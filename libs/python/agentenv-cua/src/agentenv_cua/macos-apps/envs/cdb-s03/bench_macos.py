"""What every bench task's app shares on macOS, the same file in each task app's folder.

The install brings the task's own files (``files.json``: each at the bench's pinned commit in trycua/cua, by URL and
sha256, under ``task/``) and the runtimes they run on: Electron, and Node and Chrome for Testing where the task needs
them. It builds the task's workspace in the session user's home with the task's ``reset/setup.py``, checked by its
``reset/verify.py``. Run as a script, it is the app's launch: it opens the task's windows as the task's
``platform/launch.macos.json`` says, on that workspace, places each at the bench's bounds, and stays while they run, so
stopping the app stops them all.

Where the macOS bench's own runner moved windows through its recorder daemon, this places them through Cua Driver's
``set_window_frame`` in the image's CuaDriver.app: the same call, before the agent starts, whichever arm it runs.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import time
import urllib.request
from pathlib import Path
from typing import Any

from agentenv_protocol.computer import (
    ComputerAppEnvironment,
    Download,
    Launch,
    Ready,
    RunScript,
    install,
    install_step,
    ready_check,
)
from pydantic import BaseModel, ConfigDict, Field

HERE = Path(__file__).resolve().parent
TASK = HERE / "task"
RUNTIME = HERE / "runtime"
SUPPORTS = [{"os": {"type": "macos"}, "arch": ["arm64"]}]

ELECTRON = "43.8.1"
ELECTRON_SHA256 = "01bb5d8756d220002ed5f8dccfb5a01391c4b919667af7156257cf5252e3e8ae"
NODE = "22.23.4"
NODE_SHA256 = "037650d0839f457747396f052073fb48333d31bf5abd3c98b74da20d89ebb1ad"
#: Chrome for Testing, for a task's browser in place of the Chrome the launch descriptor names: pinned, and with managed
#: policies that keep a fresh profile free of first-run tabs, sign-in, prompts and DevTools.
CHROME = "156.0.8078.4"
CHROME_SHA256 = "987d2f8499c87d21676f78178550cfcbfb1bb53786821e2cff46c5a7e99b58be"
CHROME_APP = "Google Chrome for Testing.app"
CHROME_BUNDLE_ID = "com.google.chrome.for.testing"
#: Chrome reads managed policies from the system's managed preferences, one plist by bundle id.
POLICY_PLIST = Path(f"/Library/Managed Preferences/{CHROME_BUNDLE_ID}.plist")
POLICIES = {
    "BrowserSignin": 0,
    "SyncDisabled": True,
    "PromotionalTabsEnabled": False,
    "DefaultBrowserSettingEnabled": False,
    "MetricsReportingEnabled": False,
    "PasswordManagerEnabled": False,
    "TranslateEnabled": False,
    "ComponentUpdatesEnabled": False,
    "DefaultGeolocationSetting": 2,
    "DefaultNotificationsSetting": 2,
    "CommandLineFlagSecurityWarningsEnabled": False,
    "DeveloperToolsAvailability": 2,
}
#: The apps the launch descriptors name by path, which the image installs; the app's ready check says which is missing.
LIBREOFFICE = Path("/Applications/LibreOffice.app/Contents/MacOS/soffice")
GNUCASH = Path("/Applications/Gnucash.app/Contents/MacOS/gnucash")
CUA_DRIVER = Path("/Applications/CuaDriver.app/Contents/MacOS/cua-driver")
QUIET = {
    "DO_NOT_TRACK": "1",
    "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
    "CUA_DRIVER_RS_UPDATE_CHECK": "false",
}
_LAUNCH_TIMEOUT_S = 180
_PLACE_TIMEOUT_S = 90


class MacOS(BaseModel):
    """How a macOS computer installs the app, as ``Linux`` is for Linux: ``downloads``, files fetched by URL and
    checked by sha256, put under the app's prefix; and ``run``, scripts run once they're in."""

    model_config = ConfigDict(extra="forbid")

    files: str = "."
    downloads: list[Download] = Field(default_factory=list)
    run: list[RunScript] = Field(default_factory=list)


def macos(
    *,
    downloads: list[Download],
    run: list[RunScript] | None = None,
    launch: Launch | None = None,
    paths: list[str] | None = None,
    settings: type[BaseModel] | None = None,
):
    """``@install`` for macOS."""
    return install(
        "macos",
        MacOS(downloads=downloads, run=run or []),
        launch=launch,
        paths=paths,
        settings=settings,
    )


def pinned_downloads(files_json: Path, prefix: str = "task") -> list[Download]:
    files = json.loads(files_json.read_text(encoding="utf-8"))["files"]
    return [Download(url=f["url"], sha256=f["sha256"], to=f"{prefix}/{f['path']}") for f in files]


def task_install(
    *, workspace: str, first_window: str, node: bool = False, browser: bool = False
) -> dict:
    """The ``@macos(...)`` of a task's app: the task's files, Electron (and Node and Chrome where the task needs them),
    its ``workspace`` folder of the home as its data, which a reset puts back as ``setup.py`` built it, and a launch
    that's ready once ``first_window`` shows, after which its ``windows`` check waits for the rest."""
    downloads = pinned_downloads(HERE / "files.json")
    downloads.append(
        Download(
            url=f"https://github.com/electron/electron/releases/download/v{ELECTRON}/electron-v{ELECTRON}-darwin-arm64.zip",
            sha256=ELECTRON_SHA256,
            to="runtime/electron-arm64.zip",
        )
    )
    if node:
        downloads.append(
            Download(
                url=f"https://nodejs.org/dist/v{NODE}/node-v{NODE}-darwin-arm64.tar.xz",
                sha256=NODE_SHA256,
                to="runtime/node-arm64.tar.xz",
            )
        )
    if browser:
        downloads.append(
            Download(
                url=f"https://storage.googleapis.com/chrome-for-testing-public/{CHROME}/mac-arm64/chrome-mac-arm64.zip",
                sha256=CHROME_SHA256,
                to="runtime/chrome-arm64.zip",
            )
        )
    return dict(
        downloads=downloads,
        run=[RunScript(script="install.sh", scope="session")],
        paths=[workspace],
        launch=Launch(
            at="boot",
            command=["venv/bin/python", "bench_macos.py"],
            window={"title": first_window},
            ready=Ready(window=True, timeout_s=_LAUNCH_TIMEOUT_S),
        ),
    )


class BenchTaskApp(ComputerAppEnvironment):
    """A bench task's app: its install builds the workspace, which must check out byte for byte, and it's ready once
    each of the task's windows shows and the apps it needs are installed."""

    @install_step(scope="session", os="macos")
    def workspace(self) -> None:
        spec = _spec()
        values = _values(spec)
        for step in ("setup", "verify"):
            subprocess.run(
                [_sub(arg, values) for arg in spec["semantics"]["reset"][step]],
                cwd=TASK,
                check=True,
            )

    @install_step(scope="system", os="macos")
    def browser(self) -> None:
        install_browser(RUNTIME, POLICY_PLIST)

    @ready_check(os="macos")
    def apps_installed(self) -> bool:
        return not missing_apps(_spec())

    @ready_check(os="macos")
    def windows(self) -> bool:
        specs = [app["window"] for app in _spec()["apps"] if "window" in app]
        return all(claimed(specs, list_windows()))


def missing_apps(spec: dict) -> list[str]:
    """The apps the descriptor names by path that the image doesn't have: LibreOffice and GnuCash."""
    needed = {
        str(path)
        for app in spec["apps"]
        for path in (LIBREOFFICE, GNUCASH)
        if app["command"][0] == str(path)
    }
    return sorted(p for p in needed if not Path(p).exists())


def install_browser(runtime: Path, plist: Path) -> None:
    """Where the task has a browser, Chrome for Testing unpacked to ``runtime/chrome``, and ``POLICIES`` written
    to the managed preferences."""
    for archive in runtime.glob("chrome-*.zip"):
        target = runtime / "chrome"
        subprocess.run(
            ["ditto", "-x", "-k", str(archive), str(runtime / "chrome-unpacked")], check=True
        )
        (runtime / "chrome-unpacked" / "chrome-mac-arm64").rename(target)
        (runtime / "chrome-unpacked").rmdir()
        archive.unlink()
        import plistlib

        plist.parent.mkdir(parents=True, exist_ok=True)
        plist.write_bytes(plistlib.dumps(POLICIES))


def launch() -> None:
    """Open the task's windows on its workspace, then place each at the bench's bounds, and stay while any runs."""
    spec = _spec()
    values = _values(spec)
    workspace = Path(values["${workspace}"])
    _unlock(workspace)
    logs = workspace / ".runtime" / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    running = []
    for app in spec["apps"]:
        env = {
            **{k: v for k, v in os.environ.items() if k not in app.get("unset_env", [])},
            **{k: _sub(v, values) for k, v in app.get("env", {}).items()},
        }
        command = _command(app, values)
        _quiet(command, env)
        with open(logs / f"{app['id']}.log", "wb") as log:
            running.append(
                subprocess.Popen(
                    command,
                    cwd=_sub(app.get("cwd", "${bundle}"), values),
                    env=env,
                    stdin=subprocess.DEVNULL,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                )
            )
        if ready := app.get("ready"):
            _wait_for(ready["url"], ready["timeout_seconds"])
    _place([app["window"] for app in spec["apps"] if "window" in app])
    while any(process.poll() is None for process in running):
        time.sleep(1)


def _unlock(workspace: Path) -> None:
    """Remove the locks a stopped app left, LibreOffice's and GnuCash's, which would keep it from opening its file
    again: a launch only runs with every one of the task's apps stopped."""
    for path in workspace.rglob("*"):
        if path.is_file() and (
            re.fullmatch(r"\.~lock\..*#", path.name) or path.suffix in (".LCK", ".LNK")
        ):
            path.unlink(missing_ok=True)


def _spec() -> dict:
    return json.loads((TASK / "platform" / "launch.macos.json").read_text(encoding="utf-8"))


def _values(spec: dict) -> dict[str, str]:
    workspace = Path(spec["workspace_root"].replace("${HOME}", str(Path.home())))
    return {
        "${bundle}": str(TASK),
        "${workspace}": str(workspace),
        "${workspace_uri}": workspace.as_uri(),
        "${python}": sys.executable,
    }


def _sub(text: str, values: dict[str, str]) -> str:
    return re.sub(r"\$\{\w+\}", lambda match: values.get(match.group(0), match.group(0)), text)


def _command(app: dict, values: dict[str, str]) -> list[str]:
    """The app's command as the task gives it, with Electron, Node and Chrome for Testing this app installed in place
    of ``npx electron``, ``node`` and Google Chrome. Electron runs with its own profile under the workspace's runtime
    folder, and Chrome with the launch's own flags."""
    argv = [_sub(arg, values) for arg in app["command"]]
    if argv[:2] == ["npx", "electron"]:
        profile = Path(values["${workspace}"]) / ".runtime" / f"electron-{app['id']}"
        return [
            str(RUNTIME / "electron" / "Electron.app" / "Contents" / "MacOS" / "Electron"),
            *argv[2:],
            f"--user-data-dir={profile}",
        ]
    if argv[0] == "node":
        return [str(RUNTIME / "node" / "bin" / "node"), *argv[1:]]
    if argv[0].endswith("Google Chrome.app/Contents/MacOS/Google Chrome"):
        return [
            str(
                RUNTIME / "chrome" / CHROME_APP / "Contents" / "MacOS" / "Google Chrome for Testing"
            ),
            *argv[1:],
        ]
    return argv


def _quiet(command: list[str], env: dict[str, str]) -> None:
    """A first run's tip of the day off, GnuCash's and LibreOffice's, in the settings each keeps in the runtime folder
    the task gives it, where a reset leaves none."""
    if command[0] == str(GNUCASH) and "GNC_CONFIG_HOME" in env:
        env.setdefault("GSETTINGS_BACKEND", "keyfile")
        env.setdefault("XDG_CONFIG_HOME", env["GNC_CONFIG_HOME"])
        _setting(
            Path(env["GNC_CONFIG_HOME"]) / "glib-2.0" / "settings" / "keyfile",
            "[org/gnucash/GnuCash/dialogs/tip-of-the-day]",
            "show-at-startup=false",
        )
    profile = next(
        (arg.split("=", 1)[1] for arg in command if arg.startswith("-env:UserInstallation=")), None
    )
    if command[0] == str(LIBREOFFICE) and profile:
        path = (
            Path(urllib.request.url2pathname(profile.removeprefix("file://")))
            / "user"
            / "registrymodifications.xcu"
        )
        tip = (
            '<item oor:path="/org.openoffice.Office.Common/Misc"><prop oor:name="ShowTipOfTheDay" '
            'oor:op="fuse"><value>false</value></prop></item>'
        )
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(
                '<?xml version="1.0" encoding="UTF-8"?>\n<oor:items xmlns:oor="http://openoffice.org/2001/'
                'registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/'
                f'XMLSchema-instance">\n{tip}\n</oor:items>\n',
                encoding="utf-8",
            )
        elif "ShowTipOfTheDay" not in (text := path.read_text(encoding="utf-8")):
            path.write_text(text.replace("</oor:items>", f"{tip}\n</oor:items>"), encoding="utf-8")


def _setting(keyfile: Path, section: str, line: str) -> None:
    """``line`` in ``section`` of a GLib keyfile, in place of the same key's there."""
    keyfile.parent.mkdir(parents=True, exist_ok=True)
    text = keyfile.read_text(encoding="utf-8") if keyfile.exists() else ""
    key = line.split("=", 1)[0]
    if section not in text:
        text = f"{text.rstrip()}\n\n{section}\n{line}\n".lstrip()
    else:
        head, _, rest = text.partition(section)
        body, sep, tail = rest.partition("\n[")
        body = "\n".join(entry for entry in body.split("\n") if not entry.startswith(f"{key}="))
        text = f"{head}{section}\n{line}{body}{sep}{tail}"
    keyfile.write_text(text, encoding="utf-8")


def _wait_for(url: str, timeout_s: float) -> None:
    deadline = time.monotonic() + timeout_s
    while True:
        try:
            with urllib.request.urlopen(url, timeout=2):
                return
        except OSError:
            if time.monotonic() > deadline:
                raise SystemExit(f"{url} didn't answer within {timeout_s} s") from None
            time.sleep(0.5)


# --- windows, through Cua Driver's CLI in the image's CuaDriver.app (its Accessibility grant is the image's) ---------


def cua_driver(*args: str, timeout: float = 20) -> str:
    done = subprocess.run(
        [str(CUA_DRIVER), *args],
        capture_output=True,
        text=True,
        timeout=timeout,
        env={**os.environ, **QUIET},
    )
    return done.stdout


def _walk(node: Any):
    if isinstance(node, dict):
        yield node
        for value in node.values():
            yield from _walk(value)
    elif isinstance(node, list):
        for item in node:
            yield from _walk(item)


def list_windows() -> list[dict]:
    """Each window Cua Driver lists, as ``{title, window_id, pid}``."""
    out = cua_driver("call", "list_windows", "{}")
    try:
        data = json.loads(out[out.index("{") :])
    except ValueError:
        return []
    windows = []
    for item in _walk(data):
        title = str(item.get("title") or item.get("window_title") or "")
        wid, pid = item.get("window_id", item.get("id")), item.get("pid", item.get("owner_pid"))
        if title and wid is not None and pid is not None:
            windows.append(
                {"title": title, "window_id": wid, "pid": pid, "bounds": item.get("bounds")}
            )
    return windows


def _match(window: dict, listed: dict) -> bool:
    if "title" in window:
        return listed["title"] == window["title"]
    return window["title_contains"].lower() in listed["title"].lower()


def claimed(windows: list[dict], listed: list[dict]) -> list[list[dict]]:
    """The listed windows each of ``windows`` matches, a window matched by more than one belonging to the most
    specific: a whole title first, then the longest part of one. GnuCash's "kestrel-wharf" also matches Calc's
    "kestrel-wharf-reconciliation.ods" window, which Calc's longer match holds."""
    order = sorted(
        range(len(windows)),
        key=lambda i: ("title" not in windows[i], -len(windows[i].get("title_contains", ""))),
    )
    taken: set[Any] = set()
    found: list[list[dict]] = [[] for _ in windows]
    for i in order:
        found[i] = [w for w in listed if _match(windows[i], w) and w["window_id"] not in taken]
        taken.update(w["window_id"] for w in found[i])
    return found


def _at(listed: dict, bounds: dict) -> bool:
    current = listed.get("bounds") or {}
    try:
        return all(abs(current[k] - bounds[k]) <= 4 for k in ("x", "y", "width", "height"))
    except (KeyError, TypeError):
        return False


def _place(
    windows: list[dict], *, listing=list_windows, move=None, now=time.monotonic, sleep=time.sleep
) -> bool:
    """Move each window to the bench's bounds, again until it stays there three looks in a row, since an app may
    resize itself once it's shown, as LibreOffice does; False when a window never showed or never settled."""
    move = move or _set_frame
    deadline, held = now() + _PLACE_TIMEOUT_S, [0] * len(windows)
    wanted = [i for i, w in enumerate(windows) if w.get("bounds")]
    while now() < deadline:
        for i, ids in enumerate(claimed(windows, listing())):
            if i not in wanted or not ids:
                continue
            if _at(ids[0], windows[i]["bounds"]):
                held[i] += 1
            else:
                held[i] = 0
                move(ids[0], windows[i]["bounds"])
        if all(held[i] >= 3 for i in wanted):
            return True
        sleep(1)
    return False


def _set_frame(listed: dict, bounds: dict) -> None:
    args = {
        "pid": listed["pid"],
        "window_id": listed["window_id"],
        **{k: bounds[k] for k in ("x", "y", "width", "height")},
    }
    cua_driver("call", "set_window_frame", json.dumps(args))


if __name__ == "__main__":
    launch()
