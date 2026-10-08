"""Run a task of the CDB task pack in this runner.

The pack is private (proprietary benchmark material, see ../../PROVENANCE.md) and is not in this repository.
This module is generic: it contains no task content. It reads a task's own launch descriptor
(`platform/launch.macos.json`) from the pack and does what the descriptor says: reset the workspace, start the
apps, run the evaluator, stop the apps. The pack location comes from `CDB_TASKPACK` (default
`$CDB_BENCH_WORK/taskpack`); `task.json` of a task names the pack task (`pack_task`) and the pack's git revision and
tree digest (`pack_revision`, `pack_tree_sha256`), which the preflight checks.

Mechanical adaptations, all recorded in TASKS.md:
* `terminal` and `editor` apps of the descriptor are not started: both arms get the same Bash, Edit and Write tools.
* The brief is the pack's `brief.md` with the MCP server name made neutral (`cua` -> computer-use) plus one
  line giving the workspace path, because the runner's working directory is not the workspace.
* `electron` apps (Amendment 4): the descriptor starts them with `npx electron .`, so every app is a process of
  the one `Electron.app` (bundle id `com.github.Electron`, name "Electron") and Codex computer use cannot tell
  them apart. Each app instead runs from its own copy of the pack's own `Electron.app`, with the app's window
  title as its name and its own bundle id (`com.trycua.cdbbench.<name>`). Same Electron binary, same app code,
  same arguments; only the bundle's name and id differ.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
from pathlib import Path
from typing import Any

import claude_arms as ca

SKIP_KINDS = {"terminal", "editor"}
EVAL_ENTRY = "/usr/local/libexec/cdb-eval-run"
APP_PATH = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"


def pack_tasks_root() -> Path:
    base = Path(os.environ.get("CDB_TASKPACK") or (ca.WORK / "taskpack"))
    return base / "tasks"


def tree_sha256(path: Path) -> str:
    """Digest of a directory tree: relative paths and file bytes, skipping node_modules and caches."""
    digest = hashlib.sha256()
    for item in sorted(p for p in path.rglob("*") if p.is_file()):
        rel = item.relative_to(path)
        if any(part in ("node_modules", "__pycache__", ".git") for part in rel.parts):
            continue
        digest.update(str(rel).encode())
        digest.update(hashlib.sha256(item.read_bytes()).digest())
    return digest.hexdigest()


def node_facts(node: str = "/usr/local/bin/node") -> dict[str, str]:
    path = Path(node)
    if not path.exists():
        path = Path(shutil.which("node") or "node")
    version = subprocess.run([str(path), "--version"], capture_output=True, text=True).stdout.strip()
    sha = hashlib.sha256(path.resolve().read_bytes()).hexdigest()
    return {"path": str(path.resolve()), "sha256": sha, "version": version}


def _descriptor(spec: dict[str, Any]) -> dict[str, Any]:
    path = pack_tasks_root() / spec["pack_task"] / "platform/launch.macos.json"
    return json.loads(path.read_text("utf-8"))


def allowed_app_names(spec: dict[str, Any]) -> set[str]:
    """Names the host loop may approve for Codex computer use: window titles and app bundle names of the
    descriptor, plus the process names of Electron and LibreOffice. Anything else is declined."""
    names: set[str] = {"electron"}
    for app in _descriptor(spec)["apps"]:
        if app["kind"] in SKIP_KINDS:
            continue
        win = app.get("window") or {}
        for key in ("title", "title_contains"):
            if win.get(key):
                names.add(str(win[key]).lower())
        for part in app["command"]:
            match = re.search(r"/([^/]+)\.app/", part)
            if match:
                names.add(match.group(1).lower())
        if any("soffice" in part for part in app["command"]):
            names.update({"libreoffice", "soffice"})
        if any("gnucash" in part.lower() for part in app["command"]):
            names.add("gnucash")
    return names


ELECTRON_ID_PREFIX = "com.trycua.cdbbench."
ELECTRON_BASE_ID = "com.github.Electron"
LSREGISTER = Path(
    "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework"
    "/Support/lsregister"
)


def electron_apps_root() -> Path:
    return Path(os.environ.get("CDB_ELECTRON_APPS") or (ca.WORK / "electron-apps"))


def app_display_name(app: dict[str, Any]) -> str:
    """The name an Electron app gets as its own bundle: its window title, else its role, else its id."""
    win = app.get("window") or {}
    return str(win.get("title") or app.get("role") or app["id"]).strip()


def slug(name: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")


def electron_bundle_id(name: str) -> str:
    return ELECTRON_ID_PREFIX + slug(name)


def _sign(bundle: Path) -> None:
    """Ad-hoc re-sign after the Info.plist edits (the copy's seal no longer matches)."""
    done = subprocess.run(
        ["codesign", "--force", "--deep", "--sign", "-", str(bundle)], capture_output=True, text=True
    )
    if done.returncode != 0:
        raise RuntimeError(f"codesign {bundle.name} failed: {done.stderr[-300:]}")


def named_electron_app(source: Path, name: str, root: Path | None = None) -> Path:
    """A copy of ``source`` (an Electron.app) named ``name`` with its own bundle id, made once and reused.

    The main bundle gets CFBundleName/CFBundleDisplayName ``name`` and CFBundleIdentifier
    ``com.trycua.cdbbench.<slug>``; its helper apps get the same id plus their original suffix
    (``.helper``, ``.helper.Renderer`` ...), as electron-packager does. CFBundleExecutable stays ``Electron``,
    so the helpers are found as before and the runner's ``pkill -x Electron`` still clears leftovers."""
    import plistlib

    bundle_id = electron_bundle_id(name)
    target = (root or electron_apps_root()) / slug(name) / f"{name}.app"
    stamp = target.parent / "source.json"
    source_plist = source / "Contents/Info.plist"
    want = {
        "source": str(source.resolve()),
        "source_info_sha256": hashlib.sha256(source_plist.read_bytes()).hexdigest(),
        "name": name,
        "bundle_id": bundle_id,
    }
    if target.is_dir() and stamp.is_file():
        try:
            if json.loads(stamp.read_text("utf-8")) == want:
                return target
        except ValueError:
            pass
    shutil.rmtree(target.parent, ignore_errors=True)
    target.parent.mkdir(parents=True)
    done = subprocess.run(["cp", "-Rc", str(source), str(target)], capture_output=True, text=True)
    if done.returncode != 0:  # no APFS clone (other filesystem): plain copy
        shutil.copytree(source, target, symlinks=True)
    plists = [target / "Contents/Info.plist"] + sorted(
        (target / "Contents/Frameworks").glob("*.app/Contents/Info.plist")
    )
    for path in plists:
        data = plistlib.loads(path.read_bytes())
        old = str(data.get("CFBundleIdentifier", ""))
        suffix = old[len(ELECTRON_BASE_ID) :] if old.startswith(ELECTRON_BASE_ID) else ""
        data["CFBundleIdentifier"] = bundle_id + suffix
        if path.parent.parent == target:
            data["CFBundleName"] = name
            data["CFBundleDisplayName"] = name
        path.write_bytes(plistlib.dumps(data))
    _sign(target)
    if LSREGISTER.exists():
        subprocess.run([str(LSREGISTER), "-f", str(target)], capture_output=True, timeout=60)
    stamp.write_text(json.dumps(want, indent=2) + "\n", "utf-8")
    return target


def electron_argv(argv: list[str], cwd: str, name: str) -> list[str]:
    """Rewrite ``[... npx electron <args>]`` to ``[... <Name>.app/Contents/MacOS/Electron <args>]``.

    ``npx electron`` resolves ``node_modules/electron`` of ``cwd`` and execs its ``dist/Electron.app`` with the
    remaining arguments; this does the same with a named copy of that bundle. Anything before ``npx`` (such as
    ``env -u ELECTRON_RUN_AS_NODE``) is kept."""
    try:
        at = next(i for i in range(len(argv) - 1) if argv[i] == "npx" and argv[i + 1] == "electron")
    except StopIteration:
        return argv
    source = Path(cwd) / "node_modules/electron/dist/Electron.app"
    if not source.is_dir():
        raise RuntimeError(f"Electron.app missing for {name}: {source}")
    bundle = named_electron_app(source, name)
    return argv[:at] + [str(bundle / "Contents/MacOS/Electron")] + argv[at + 2 :]


CHROME_EXE = "/Contents/MacOS/Google Chrome"


def with_force_accessibility(argv: list[str], kind: str, flag: str = ca.FORCE_ACCESSIBILITY_FLAG) -> list[str]:
    """Amendment 9 (arc arm only): start Chromium with its renderer accessibility switched on. Chrome gets the
    switch right after its executable, before the start URL. An Electron app gets it at the end, after the app's
    own arguments (Chromium reads switches anywhere on the line; the pack's apps find theirs by prefix). Other
    apps are unchanged, and so is an argv that already carries the switch."""
    if flag in argv:
        return argv
    if kind == "electron":
        return [*argv, flag]
    if kind == "browser":
        for i, part in enumerate(argv):
            if part.endswith(CHROME_EXE):
                return [*argv[: i + 1], flag, *argv[i + 1 :]]
    return argv


def kill_policy(spec: dict[str, Any]) -> tuple[list[str], list[int]]:
    """Process names and listening ports to clear before and after a trial (VM only)."""
    text = json.dumps(_descriptor(spec))
    names: list[str] = []
    if "electron" in text:
        names.append("Electron")
    if "Google Chrome" in text:
        names.append("Google Chrome")
    if "soffice" in text:
        names += ["soffice", "soffice.bin"]
    if "gnucash" in text.lower():
        names += ["gnucash", "Gnucash"]
    ports = sorted({int(p) for p in re.findall(r"127\.0\.0\.1:(\d+)", text)})
    return names, ports


def clear_leftovers(spec: dict[str, Any]) -> None:
    """Kill leftovers of an earlier trial. Only in a disposable VM (CDB_BENCH_DISPOSABLE=1): by process
    name this would also kill the user's own Chrome."""
    if os.environ.get("CDB_BENCH_DISPOSABLE") != "1":
        return
    names, ports = kill_policy(spec)
    for name in names:
        subprocess.run(["pkill", "-x", name], capture_output=True)
    for port in ports:
        pids = subprocess.run(
            ["/usr/sbin/lsof", "-ti", f"tcp:{port}", "-sTCP:LISTEN"], capture_output=True, text=True
        ).stdout.split()
        for pid in pids:
            subprocess.run(["kill", "-9", pid], capture_output=True)
    time.sleep(0.5)


class CdbTask:
    """One task of the pack, driven by its launch descriptor."""

    def __init__(self, spec: dict[str, Any], artifacts: Path) -> None:
        self.spec = spec
        self.bundle = pack_tasks_root() / spec["pack_task"]
        self.descriptor = json.loads((self.bundle / "platform/launch.macos.json").read_text("utf-8"))
        self.artifacts = artifacts
        self.workspace = Path(
            self.descriptor["workspace_root"].replace("${HOME}", str(Path.home()))
        )
        self.node = node_facts()
        self.procs: list[subprocess.Popen[bytes]] = []
        self.log: list[str] = []

    # ---- substitution
    def sub(self, text: str, **extra: str) -> str:
        values = {
            "python": sys.executable,
            "workspace": str(self.workspace),
            "workspace_uri": self.workspace.as_uri(),
            "bundle": str(self.bundle),
            "HOME": str(Path.home()),
            "artifacts": str(self.artifacts),
            "evaluator_node": self.node["path"],
            "evaluator_node_sha256": self.node["sha256"],
            "evaluator_node_version": self.node["version"],
            **extra,
        }
        return re.sub(r"\$\{(\w+)\}", lambda m: values.get(m.group(1), m.group(0)), text)

    def _run(self, command: list[str], timeout: int = 300, **extra: str) -> subprocess.CompletedProcess[str]:
        argv = [self.sub(part, **extra) for part in command]
        env = {"HOME": str(Path.home()), "PATH": APP_PATH, "LANG": "en_US.UTF-8"}
        return subprocess.run(
            argv, cwd=str(self.bundle), env=env, capture_output=True, text=True, timeout=timeout
        )

    # ---- lifecycle
    def reset(self) -> None:
        """Descriptor setup, then the pack's byte-level reset verification. Raises on failure."""
        clear_leftovers(self.spec)
        sem = self.descriptor["semantics"]["reset"]
        for step in ("setup", "verify"):
            done = self._run(sem[step])
            self.log.append(f"reset.{step} rc={done.returncode}")
            if done.returncode != 0:
                raise RuntimeError(f"reset {step} failed: {(done.stderr or done.stdout)[-400:]}")

    def brief(self) -> str:
        text = (self.bundle / "brief.md").read_text("utf-8")
        text = text.replace("configured `cua` MCP server", "configured computer-use MCP server")
        return text.strip() + f"\n\nYour workspace directory is `{self.workspace}`.\n"

    def apps(self) -> list[dict[str, Any]]:
        return [a for a in self.descriptor["apps"] if a["kind"] not in SKIP_KINDS]

    def start_apps(self, windows: Any = None, force_accessibility: bool = False) -> None:
        for app in self.apps():
            env = {"HOME": str(Path.home()), "PATH": APP_PATH, "LANG": "en_US.UTF-8"}
            env.update({k: self.sub(v) for k, v in (app.get("env") or {}).items()})
            cwd = self.sub(app["cwd"]) if app.get("cwd") else str(self.bundle)
            argv = [self.sub(p) for p in app["command"]]
            if app.get("kind") == "electron" and os.environ.get("CDB_ELECTRON_SHARED") != "1":
                argv = electron_argv(argv, cwd, app_display_name(app))
                self.log.append(f"electron {app['id']} as {electron_bundle_id(app_display_name(app))}")
            if force_accessibility:
                flagged = with_force_accessibility(argv, app.get("kind", ""))
                if flagged != argv:
                    self.log.append(f"{app['id']}: {ca.FORCE_ACCESSIBILITY_FLAG}")
                argv = flagged
            out =(self.artifacts / f"app-{app['id']}.log").open("ab")
            proc = subprocess.Popen(
                argv,
                cwd=cwd,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=out,
                stderr=out,
                start_new_session=True,
            )
            self.procs.append(proc)
            ready = (app.get("ready") or {}).get("http")
            if ready:
                self._wait_http(self.sub(ready), float(app["ready"].get("timeout_seconds", 20)))
            self.log.append(f"started {app['id']} pid={proc.pid}")
            time.sleep(1.0)
        if windows is not None:
            for app in self.apps():
                win = app.get("window")
                if win and win.get("bounds") and windows(win) is False:
                    raise RuntimeError(f"window of app {app['id']} did not appear")
        time.sleep(1.5)

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

    def _digest_hidden(self) -> str | None:
        """Digest of the hidden pack tree through the evaluator entry point (VM isolation mode)."""
        if os.environ.get("CDB_EVAL_SUDO") != "1":
            return None
        done = subprocess.run(
            ["sudo", "-n", "-u", "cdbeval", EVAL_ENTRY, "--digest", self.spec["pack_task"]],
            capture_output=True,
            text=True,
            timeout=120,
        )
        return done.stdout.strip() or None

    def evaluate(self, agent_exit_code: int) -> dict[str, Any]:
        result = self.artifacts / "cdb-result.json"
        if os.environ.get("CDB_EVAL_SUDO") == "1":
            # The pack's evaluator, oracle and hidden tests live in a home the agent's user cannot read.
            subprocess.run(["chmod", "-R", "a+rwX", str(self.workspace)], capture_output=True)
            done = subprocess.run(
                [
                    "sudo", "-n", "-u", "cdbeval", EVAL_ENTRY, self.spec["pack_task"],
                    str(self.workspace), str(agent_exit_code), self.node["path"],
                    self.node["sha256"], self.node["version"],
                ],
                capture_output=True,
                text=True,
                timeout=300,
            )
            if done.stdout.strip().startswith("{"):
                result.write_text(done.stdout, "utf-8")
        else:
            command = self.descriptor["semantics"]["evaluate"]
            done = self._run(
                command,
                timeout=300,
                result=str(result),
                agent_exit_code=str(agent_exit_code),
            )
        (self.artifacts / "cdb-evaluator.stderr").write_text(
            (done.stderr or "")[-4000:], "utf-8"
        )
        if not result.is_file():
            return {"passed": False, "score": None, "error": "evaluator wrote no result"}
        data = json.loads(result.read_text("utf-8"))
        checks_raw = (data.get("detail") or {}).get("checks") or data.get("checks") or {}
        checks = {
            name: bool(c.get("passed")) if isinstance(c, dict) else bool(c)
            for name, c in checks_raw.items()
        }
        passed = data.get("passed")
        return {
            "passed": bool(passed),
            "score": data.get("score"),
            "checks": checks,
            "diagnostics": {"raw_keys": sorted(data)[:12]},
        }

    def stop_apps(self) -> None:
        for proc in reversed(self.procs):
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    os.killpg(proc.pid, sig)
                except (ProcessLookupError, PermissionError):
                    break
                time.sleep(0.4)
        self.procs.clear()
        clear_leftovers(self.spec)

    def clean_workspace(self) -> None:
        """Leave no workspace behind (the pack's setup recreates it next trial)."""
        shutil.rmtree(self.workspace, ignore_errors=True)
