# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""A task of the CDB task pack (CDB-G02 to CDB-G04) on a Windows desktop.

The Windows counterpart of `cdb_adapter.CdbTask`. It reads the task's own `platform/launch.windows.json` and
does what it says: reset the workspace (setup, then the byte-level verify), start the apps, place their
windows, run the evaluator, stop the apps. No task content lives here.

Mechanical adaptations, the same as on macOS where one applies:
* `terminal` and `editor` apps are not started (every setup gets the same tools; GUI-only on Windows).
* The brief is the pack's `brief.md` with the MCP server name made neutral plus one line giving the
  workspace path (as `cdb_adapter.CdbTask.brief`).
* `electron` apps (macOS Amendment 4): `npx electron .` would make every app a process called
  `electron.exe`, so a setup that names apps by process cannot tell them apart. Each app instead runs from
  a copy of the pack's own `electron.exe` named after its window title (`Copperfield Mail.exe`), in the same
  `dist` folder, so the copy finds the same Electron runtime files. Same binary, code and arguments.
* GnuCash keeps its preferences (GSettings) in the registry on Windows, outside the task workspace, so the
  reset deletes `HKCU\\Software\\GSettings\\org\\gnucash` before each trial: every trial starts GnuCash with
  default preferences, as the descriptor's task-scoped data and config folders intend.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.request
import zipfile
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
TASKPACK = Path(os.environ.get("CDB_TASKPACK") or HERE.parents[2] / "taskpack")
SKIP_KINDS = {"terminal", "editor"}
GNUCASH_PREFS = r"HKCU\Software\GSettings\org\gnucash"
CREATE_NEW_PROCESS_GROUP = 0x00000200


def descriptor_path(pack_task: str, root: Path | None = None) -> Path:
    return (root or TASKPACK) / "tasks" / pack_task / "platform" / "launch.windows.json"


def substitute(text: str, values: dict[str, str]) -> str:
    return re.sub(r"\$\{(\w+)\}", lambda m: values.get(m.group(1), m.group(0)), text)


def app_display_name(app: dict[str, Any]) -> str:
    """The name an Electron app runs under: its window title, else its role, else its id (as on macOS)."""
    win = app.get("window") or {}
    return str(win.get("title") or app.get("role") or app["id"]).strip()


def exe_name(name: str) -> str:
    return re.sub(r'[<>:"/\\|?*]+', "-", name).strip(" .") + ".exe"


def electron_argv(argv: list[str], cwd: str, name: str, *, copy: bool = True) -> list[str]:
    """Rewrite `[npx|npx.cmd, electron, <args>]` to `[<cwd>\\node_modules\\electron\\dist\\<Name>.exe, <args>]`.

    `npx electron` resolves `node_modules/electron` of `cwd` and runs its `dist/electron.exe` with the remaining
    arguments; this runs a copy of that executable named NAME (made once, refreshed when the original changes)."""
    try:
        at = next(
            i for i in range(len(argv) - 1)
            if Path(argv[i]).name.lower() in ("npx", "npx.cmd") and argv[i + 1] == "electron"
        )  # fmt: skip
    except StopIteration:
        return argv
    dist = Path(cwd) / "node_modules" / "electron" / "dist"
    source = dist / "electron.exe"
    target = dist / exe_name(name)
    if copy:
        if not source.is_file():
            raise RuntimeError(f"electron.exe missing for {name}: {source} (run `npm ci` in {cwd})")
        if not target.is_file() or target.stat().st_size != source.stat().st_size:
            shutil.copy2(source, target)
    return argv[:at] + [str(target)] + argv[at + 2 :]


def process_names(descriptor: dict[str, Any]) -> set[str]:
    """Executable names (lower case) the task's apps run as: cleared before and after a trial."""
    names: set[str] = set()
    for app in descriptor["apps"]:
        if app["kind"] in SKIP_KINDS:
            continue
        if app["kind"] == "electron":
            names.update({"electron.exe", exe_name(app_display_name(app)).lower()})
            continue
        exe = Path(app["command"][0].replace("\\", "/")).name.lower()
        if exe in ("soffice.exe", "scalc.exe", "swriter.exe"):
            names.update({"soffice.exe", "soffice.bin", "scalc.exe"})
        elif (
            exe != "node" and exe != "node.exe"
        ):  # the archive service: cleared by its port, not by name
            names.add(exe if exe.endswith(".exe") else exe + ".exe")
    if "gnucash.exe" in names:
        names.add("gnucash-cli.exe")
    return names


def ports(descriptor: dict[str, Any]) -> list[int]:
    text = json.dumps(descriptor)
    found = {int(p) for p in re.findall(r"127\.0\.0\.1:(\d+)", text)}
    for app in descriptor["apps"]:
        port = (app.get("env") or {}).get("CDB_S04_PORT")
        if port:
            found.add(int(port))
    return sorted(found)


def title_matches(win: dict[str, Any], title: str) -> bool:
    if win.get("title"):
        return title == win["title"] or title.startswith(win["title"] + " ")
    if win.get("title_contains"):
        return win["title_contains"].lower() in title.lower()
    return False


class CdbTask:
    """One task of the pack, driven by its Windows launch descriptor."""

    def __init__(self, spec: dict[str, Any], artifacts: Path, seed: int | None = None) -> None:
        self.spec = spec
        self.seed = seed
        self.bundle = TASKPACK / "tasks" / spec["pack_task"]
        self.descriptor = json.loads(descriptor_path(spec["pack_task"]).read_text("utf-8"))
        self.artifacts = artifacts
        home = os.environ.get("USERPROFILE") or str(Path.home())
        self.workspace = Path(self.descriptor["workspace_root"].replace("${USERPROFILE}", home))
        self.procs: list[tuple[dict[str, Any], subprocess.Popen]] = []
        self.windows: dict[str, int] = {}
        self.log: list[str] = []

    def values(self, **extra: str) -> dict[str, str]:
        return {
            "python": sys.executable,
            "seed": str(self.seed),
            "workspace": str(self.workspace),
            "workspace_uri": self.workspace.as_uri(),
            "bundle": str(self.bundle),
            "USERPROFILE": os.environ.get("USERPROFILE", str(Path.home())),
            "artifacts": str(self.artifacts),
            **extra,
        }

    def sub(self, text: str, **extra: str) -> str:
        return substitute(text, self.values(**extra))

    def _run(
        self, command: list[str], timeout: int = 300, **extra: str
    ) -> subprocess.CompletedProcess:
        argv = [self.sub(part, **extra) for part in command]
        return subprocess.run(
            argv, cwd=str(self.bundle), capture_output=True, text=True, encoding="utf-8",
            errors="replace", timeout=timeout,
        )  # fmt: skip

    # ---- lifecycle
    def clear_leftovers(self) -> None:
        """Stop leftovers of an earlier trial in this desktop session only: the task's app executables by name,
        and whatever listens on the task's loopback ports."""
        import winenv as W

        here = W.session_of(os.getpid())
        names = process_names(self.descriptor)
        for pid, _, exe in W.processes():
            if exe.lower() in names and pid != os.getpid() and W.session_of(pid) == here:
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(pid)], capture_output=True)
        for port in ports(self.descriptor):
            out = subprocess.run(
                ["netstat", "-ano", "-p", "TCP"], capture_output=True, text=True
            ).stdout
            for line in out.splitlines():
                parts = line.split()
                if len(parts) >= 5 and parts[1].endswith(f":{port}") and parts[3] == "LISTENING":
                    subprocess.run(["taskkill", "/T", "/F", "/PID", parts[4]], capture_output=True)
        time.sleep(0.5)

    def reset(self) -> None:
        """Descriptor setup, then the pack's byte-level reset verification. Raises on failure."""
        self.clear_leftovers()
        if any("gnucash" in json.dumps(a["command"]).lower() for a in self.descriptor["apps"]):
            subprocess.run(["reg", "delete", GNUCASH_PREFS, "/f"], capture_output=True)
            self.log.append("gnucash preferences reset")
        sem = self.descriptor["semantics"]["reset"]
        for step in ("setup", "verify"):
            done = self._run(sem[step])
            self.log.append(f"reset.{step} rc={done.returncode}")
            if done.returncode != 0:
                raise RuntimeError(f"reset {step} failed: {(done.stderr or done.stdout)[-400:]}")

    def health_check(self) -> list[dict[str, Any]]:
        """The evaluator's tools are on PATH (the s04 evaluator starts `node`). A failure is a harness error."""
        checks = []
        for name in (
            ("python", "node") if "node" in json.dumps(self.descriptor["apps"]) else ("python",)
        ):
            path = sys.executable if name == "python" else shutil.which(name)
            checks.append({"name": f"tool:{name}", "ok": bool(path), "path": path})
        bad = [c["name"] for c in checks if not c["ok"]]
        if bad:
            raise RuntimeError("harness health: missing " + ", ".join(bad))
        return checks

    def brief(self) -> str:
        brief = self.sub(self.descriptor["semantics"].get("brief", "brief.md"))
        text = (self.bundle / brief).read_text("utf-8")
        text = text.replace("configured `cua` MCP server", "configured computer-use MCP server")
        return text.strip() + f"\n\nYour workspace directory is `{self.workspace}`.\n"

    def apps(self) -> list[dict[str, Any]]:
        return [a for a in self.descriptor["apps"] if a["kind"] not in SKIP_KINDS]

    def start_apps(self) -> None:
        for app in self.apps():
            env = dict(os.environ)
            env.update({k: self.sub(v) for k, v in (app.get("env") or {}).items()})
            for key in app.get("unset_env", []):
                env.pop(key, None)
            cwd = self.sub(app["cwd"]) if app.get("cwd") else str(self.bundle)
            argv = [self.sub(p) for p in app["command"]]
            if app["kind"] == "electron":
                argv = electron_argv(argv, cwd, app_display_name(app))
                self.log.append(f"electron {app['id']} as {Path(argv[0]).name}")
            elif Path(argv[0]).name.lower() == "node":
                argv[0] = shutil.which("node") or argv[0]
            out = (self.artifacts / f"app-{app['id']}.log").open("ab")
            proc = subprocess.Popen(
                argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=out,
                creationflags=CREATE_NEW_PROCESS_GROUP,
            )  # fmt: skip
            self.procs.append((app, proc))
            ready = (app.get("ready") or {}).get("url") or (app.get("ready") or {}).get("http")
            if ready:
                self._wait_http(self.sub(ready), float(app["ready"].get("timeout_seconds", 20)))
            self.log.append(f"started {app['id']} pid={proc.pid}")
            time.sleep(1.0)

    @staticmethod
    def _wait_http(url: str, timeout: float) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(url, timeout=2) as r:
                    if r.status == 200:
                        return
            except OSError:
                pass
            time.sleep(0.4)
        raise RuntimeError(f"app not ready: {url}")

    def place_windows(self, limit: float = 90.0) -> None:
        """Wait for each app's window (a shown top-level window of the app's own processes whose title matches
        the descriptor) and give it the descriptor's bounds. Raises when a window does not appear."""
        import guard as G
        import winenv as W

        pending = [(a, p) for a, p in self.procs if (a.get("window") or {}).get("bounds")]
        deadline = time.monotonic() + limit
        while pending and time.monotonic() < deadline:
            rows = W.processes()
            for app, proc in list(pending):
                tree = G.descendants(rows, proc.pid)
                exe = Path(proc.args[0]).name.lower() if isinstance(proc.args, list) else ""
                for hwnd in W.top_level_windows():
                    if not W.shown(hwnd) or hwnd in self.windows.values():
                        continue
                    pid = W.window_pid(hwnd)
                    mine = (
                        pid in tree
                        or Path(W.process_path(pid)).name.lower() == exe
                        or (
                            exe in ("soffice.exe", "scalc.exe")
                            and W.stem(W.process_path(pid)).lower() == "soffice"
                        )
                    )  # LibreOffice hands the document to its running soffice.bin
                    if mine and title_matches(app["window"], W.text(hwnd)):
                        b = app["window"]["bounds"]
                        if W.user32.IsIconic(hwnd) or W.user32.IsZoomed(hwnd):
                            W.user32.ShowWindow(hwnd, W.SW_RESTORE)
                        W.move_window(hwnd, b["x"], b["y"], b["width"], b["height"])
                        self.windows[app["id"]] = hwnd
                        self.log.append(f"window {app['id']} hwnd={hwnd} title={W.text(hwnd)!r}")
                        pending.remove((app, proc))
                        break
            time.sleep(0.5)
        if pending:
            raise RuntimeError("windows did not appear: " + ", ".join(a["id"] for a, _ in pending))
        time.sleep(1.5)

    def evaluate(self, agent_exit_code: int) -> dict[str, Any]:
        """The pack's evaluator. A result with `harness_error` set means the harness, not the agent, failed."""
        result = self.artifacts / "cdb-result.json"
        result.unlink(missing_ok=True)
        try:
            done = self._run(
                self.descriptor["semantics"]["evaluate"], timeout=300, result=str(result),
                agent_exit_code=str(agent_exit_code),
            )  # fmt: skip
            (self.artifacts / "cdb-evaluator.stderr").write_text(
                (done.stderr or "")[-4000:], "utf-8"
            )
        except subprocess.TimeoutExpired:
            pass
        if not result.is_file():
            return {"passed": False, "score": None, "error": "evaluator wrote no result",
                    "harness_error": "evaluator_no_result"}  # fmt: skip
        data = json.loads(result.read_text("utf-8"))
        checks_raw = (data.get("detail") or {}).get("checks") or data.get("checks") or {}
        checks = {
            name: bool(c.get("passed")) if isinstance(c, dict) else bool(c)
            for name, c in checks_raw.items()
        }
        return {
            "seed": data.get("seed"),
            "passed": bool(data.get("passed")),
            "score": data.get("score"),
            "checks": checks,
            "diagnostics": {
                "raw_keys": sorted(data)[:12],
                **(data.get("diagnostics") if isinstance(data.get("diagnostics"), dict) else {}),
            },
        }

    def snapshot_workspace(self, target: Path) -> None:
        """The final workspace (without runtime profiles and node_modules) as evidence."""
        with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as z:
            for path in sorted(self.workspace.rglob("*")):
                rel = path.relative_to(self.workspace)
                if path.is_file() and not ({".runtime", "node_modules"} & set(rel.parts)):
                    try:
                        z.write(path, str(rel))
                    except OSError:
                        pass

    def stop_apps(self) -> None:
        for _, proc in reversed(self.procs):
            if proc.poll() is None:
                subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                pass
        self.procs.clear()
        self.clear_leftovers()

    def clean_workspace(self) -> None:
        """Leave no workspace behind (the pack's setup recreates it next trial)."""
        for _ in range(5):
            shutil.rmtree(self.workspace, ignore_errors=True)
            if not self.workspace.exists():
                return
            time.sleep(1.0)
