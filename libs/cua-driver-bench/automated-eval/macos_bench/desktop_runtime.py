# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

from __future__ import annotations

import json
import os
import platform
import re
import shutil
import signal
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Mapping, Sequence

Platform = Literal["macos", "linux"]
HERE = Path(__file__).resolve().parent
SESSION_KEYS = (
    "DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "XDG_DATA_DIRS",
    "AT_SPI_BUS_ADDRESS",
    "GTK_MODULES",
    "QT_ACCESSIBILITY",
    "QT_LINUX_ACCESSIBILITY_ALWAYS_ON",
    "NO_AT_BRIDGE",
)


@dataclass(frozen=True)
class Check:
    name: str
    status: Literal["pass", "fail", "skip"]
    detail: str


def detect_platform(system: str | None = None) -> Platform:
    name = system if system is not None else platform.system()
    if name == "Darwin":
        return "macos"
    if name == "Linux":
        return "linux"
    raise ValueError(f"unsupported-platform: {name}")


def tool(name: str) -> str:
    found = shutil.which(name)
    if found:
        return found
    if detect_platform() == "macos":
        fallback = Path("/opt/homebrew/bin") / name
        if fallback.is_file():
            return str(fallback)
    raise FileNotFoundError(f"Install {name} and add it to PATH")


def desktop_env(home: Path, extra: Mapping[str, str] | None = None) -> dict[str, str]:
    linux = detect_platform() == "linux"
    env = {
        "HOME": str(home),
        "TMPDIR": str(home / "tmp")
        if (home / "tmp").is_dir()
        else os.environ.get("TMPDIR", "/tmp"),
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin")
        if linux
        else "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "USER": os.environ.get("USER", ""),
        "LANG": os.environ.get("LANG", "C.UTF-8") if linux else "en_US.UTF-8",
        "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
        "DO_NOT_TRACK": "1",
    }
    if linux:
        env.update({key: os.environ[key] for key in SESSION_KEYS if key in os.environ})
    env.update(extra or {})
    return env


def bench_app_argv(name: str, build_dir: Path | None = None) -> list[str]:
    name = name.removesuffix(".app")
    if name not in ("BenchLab", "BenchSentinel"):
        raise ValueError(f"unknown bench app: {name}")
    if detect_platform() == "linux":
        script = HERE / "linux" / ("benchlab.py" if name == "BenchLab" else "sentinel.py")
        if not script.is_file():
            raise FileNotFoundError(script)
        return ["/usr/bin/python3", str(script)]
    if build_dir is None:
        raise ValueError("--build-dir is required on macOS")
    exes = sorted((build_dir / f"{name}.app" / "Contents/MacOS").iterdir())
    return [str(next(p for p in exes if p.is_file() and os.access(p, os.X_OK)))]


def stop_group(pid: int) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pid, sig)
        except OSError:
            return
        if sig == signal.SIGTERM:
            time.sleep(0.3)


def start_app(
    argv: Sequence[str], home: Path, log: Path, *, env: Mapping[str, str] | None = None
) -> subprocess.Popen:
    log.parent.mkdir(parents=True, exist_ok=True)
    (home / "tmp").mkdir(parents=True, exist_ok=True)
    with log.open("ab") as output:
        proc = subprocess.Popen(
            list(argv),
            env=desktop_env(home, env),
            stdin=subprocess.DEVNULL,
            stdout=output,
            stderr=output,
            start_new_session=True,
        )
    time.sleep(0.15)
    if proc.poll() is not None:
        stop_group(proc.pid)
        proc.wait()
        raise RuntimeError(f"{argv[0]} exited ({proc.returncode}); see {log}")
    return proc


def start_driver(
    binary: Path,
    socket: str,
    state: Path,
    *,
    overlay: bool = True,
    log_name: str = "daemon.log",
    env_extra: Mapping[str, str] | None = None,
) -> subprocess.Popen:
    Path(socket).unlink(missing_ok=True)
    args = [str(binary), "serve", "--socket", socket, "--dangerously-bypass-approvals"]
    if not overlay:
        args.append("--no-overlay")
    proc = start_app(args, state / "home", state / log_name, env=env_extra)
    try:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                raise RuntimeError(f"driver exited ({proc.returncode}); see {state / log_name}")
            done = subprocess.run(
                [str(binary), "status", "--socket", socket],
                env=desktop_env(state / "home", env_extra),
                capture_output=True,
                timeout=5,
            )
            if done.returncode == 0:
                return proc
            time.sleep(0.3)
        raise TimeoutError(f"driver not ready; see {state / log_name}")
    except BaseException:
        stop_group(proc.pid)
        proc.wait(timeout=5)
        raise


def cua_health(binary: Path, socket: str, home: Path) -> dict:
    try:
        done = subprocess.run(
            [str(binary), "--socket", socket, "call", "health_report", "{}"],
            env=desktop_env(home),
            capture_output=True,
            text=True,
            timeout=30,
        )
        data = json.loads(done.stdout[done.stdout.index("{") :])
        return {
            "ok": done.returncode == 0,
            "build": data.get("build", {}),
            "checks": {c.get("name"): c.get("status") for c in data.get("checks", [])},
        }
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        return {"ok": False, "error": f"{type(error).__name__}: {error}"}


def capabilities_ok(health: dict) -> bool:
    keys = (
        ("ax_capability", "screen_capture_capability")
        if detect_platform() == "linux"
        else ("tcc_accessibility", "tcc_screen_recording")
    )
    return bool(health.get("ok")) and all(
        health.get("checks", {}).get(key) == "pass" for key in keys
    )


def doctor(binary: Path, socket: str, home: Path) -> list[Check]:
    checks = []
    if detect_platform() == "linux":
        probes = {
            "display": (
                ["xdpyinfo", "-display", os.environ.get("DISPLAY", "")],
                "Start Xvfb/X11 and set DISPLAY",
            ),
            "at-spi": (
                [
                    "/usr/bin/python3",
                    "-c",
                    "import gi; gi.require_version('Atspi', '2.0'); from gi.repository import Atspi, Gio; bus=Gio.bus_get_sync(Gio.BusType.SESSION, None); address=bus.call_sync('org.a11y.Bus', '/org/a11y/bus', 'org.a11y.Bus', 'GetAddress', None, None, Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]; ax=Gio.DBusConnection.new_for_address_sync(address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None); ax.call_sync('org.a11y.atspi.Registry', '/org/a11y/atspi/registry', 'org.freedesktop.DBus.Introspectable', 'Introspect', None, None, Gio.DBusCallFlags.NONE, 3000, None); d=Atspi.get_desktop(0); assert d is not None; d.get_child_count()",
                ],
                "Start dbus-run-session with at-spi2-core installed",
            ),
            "ffmpeg": (
                ["ffmpeg", "-hide_banner", "-devices"],
                "Install ffmpeg with x11grab support",
            ),
        }
        for name, (argv, remedy) in probes.items():
            try:
                done = subprocess.run(
                    argv, env=desktop_env(home), capture_output=True, text=True, timeout=10
                )
                ok = done.returncode == 0 and (
                    name != "ffmpeg" or "x11grab" in done.stdout + done.stderr
                )
                checks.append(Check(name, "pass" if ok else "fail", "usable" if ok else remedy))
            except (OSError, subprocess.TimeoutExpired) as error:
                checks.append(Check(name, "fail", f"{remedy}: {error}"))
    health = cua_health(binary, socket, home)
    checks.append(
        Check("driver", "pass" if capabilities_ok(health) else "fail", json.dumps(health))
    )
    return checks


def capture_screenshot(display: str, destination: Path) -> Path:
    destination.parent.mkdir(parents=True, exist_ok=True)
    if detect_platform() == "macos":
        subprocess.run(["/usr/sbin/screencapture", "-x", str(destination)], check=True, timeout=20)
        return destination
    info = subprocess.run(
        [tool("xdpyinfo"), "-display", display],
        capture_output=True,
        text=True,
        check=True,
        timeout=5,
    )
    dimensions = re.search(r"dimensions:\s+(\d+x\d+)", info.stdout)
    if dimensions is None:
        raise RuntimeError(f"cannot determine X11 display dimensions: {display}")
    with destination.with_suffix(".stderr").open("ab") as log:
        subprocess.run(
            [
                tool("ffmpeg"),
                "-nostdin",
                "-y",
                "-f",
                "x11grab",
                "-video_size",
                dimensions[1],
                "-i",
                display,
                "-frames:v",
                "1",
                str(destination),
            ],
            stdout=subprocess.DEVNULL,
            stderr=log,
            check=True,
            timeout=20,
        )
    return destination
