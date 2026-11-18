#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# See "Linux XFCE" in libs/cua-driver-bench/README.md for provisioning the desktop and the exact agent invocation.
#
# Usage: xfce_session.py --task ID --run-index N --condition foreground|background --out DIR
#                        [--timeout S] -- COMMAND [ARG...]
#
# One Bench v2 trial on an existing X11 desktop with an EWMH window manager (XFCE/xfwm4 here), run as the desktop
# user with the session's environment. The X11 counterpart of windows_session.py for the ten public tasks:
#   BenchLab tasks (MB-09..MB-11, IR-01..IR-04): linux/benchlab.py + linux/sentinel.py;
#   pack tasks (CDB-G02..CDB-G04): reset the workspace and start the apps of the task's own
#   platform/launch.linux.json (LinuxCdbTask below, the Linux port of windows/cdb.py), place their windows.
# Then: park the pointer (Amendment 24), arm the sentinel, run COMMAND with the GUI-only guard (hyprland/guard.py,
# compositor-independent) watching, stop it at the limit or on a violation, disarm, run the checker, summarise the
# sentinel, write DIR/row.json.
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
import zipfile
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import hyprland_session as hs  # noqa: E402

v2, cdb_fairness, summarize_sentinel = hs.v2, hs.cdb_fairness, hs.summarize_sentinel
BENCH = hs.BENCH
LAB_TASKS = ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04")
CDB_TASKS = ("CDB-G02", "CDB-G03", "CDB-G04")
TASKS = CDB_TASKS + LAB_TASKS
LAB_FRAME = hs.LAB_FRAME
WITNESS_FRAME = hs.WITNESS_FRAME
SENTINEL_ID = "ai.cua.benchsentinel"
TASKPACK = Path(os.environ.get("CDB_TASKPACK") or BENCH.parents[1] / "taskpack")
SKIP_KINDS = {"terminal", "editor"}


def _out(argv: list[str], timeout: float = 5.0) -> str:
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=timeout).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


class X:
    @staticmethod
    def screen() -> tuple[int, int]:
        parts = _out(["xdotool", "getdisplaygeometry"]).split()
        return (int(parts[0]), int(parts[1])) if len(parts) == 2 else (0, 0)

    @staticmethod
    def windows_of(pid: int) -> list[int]:
        return [
            int(w) for w in _out(["xdotool", "search", "--onlyvisible", "--pid", str(pid)]).split()
        ]

    @staticmethod
    def visible() -> list[int]:
        return [int(w) for w in _out(["xdotool", "search", "--onlyvisible", "--name", "."]).split()]

    @staticmethod
    def wm_class(window: int) -> str:
        names = re.findall(r'"([^"]+)"', _out(["xprop", "-id", str(window), "WM_CLASS"]))
        return names[-1] if names else ""

    @staticmethod
    def active() -> int:
        text = _out(["xdotool", "getactivewindow"]).strip()
        return int(text) if text.isdigit() else 0

    @classmethod
    def active_class(cls) -> str:
        w = cls.active()
        return cls.wm_class(w) if w else ""

    @staticmethod
    def stacking() -> list[int]:
        return [
            int(v, 16)
            for v in re.findall(
                r"0x[0-9a-fA-F]+", _out(["xprop", "-root", "_NET_CLIENT_LIST_STACKING"])
            )
        ]

    @classmethod
    def normal_stack(cls) -> list[int]:
        """Stacking order without dock and desktop windows (the XFCE panels stay above every client)."""
        out = []
        for w in cls.stacking():
            kind = _out(["xprop", "-id", str(w), "_NET_WM_WINDOW_TYPE"])
            if "_NET_WM_WINDOW_TYPE_DOCK" in kind or "_NET_WM_WINDOW_TYPE_DESKTOP" in kind:
                continue
            out.append(w)
        return out

    @staticmethod
    def geometry(window: int) -> list[int]:
        shell = dict(
            ln.split("=", 1)
            for ln in _out(["xdotool", "getwindowgeometry", "--shell", str(window)]).split()
            if "=" in ln
        )
        try:
            return [int(shell[k]) for k in ("X", "Y", "WIDTH", "HEIGHT")]
        except (KeyError, ValueError):
            return []

    @staticmethod
    def title(window: int) -> str:
        return _out(["xdotool", "getwindowname", str(window)]).strip()

    @staticmethod
    def activate(window: int) -> None:
        _out(["xdotool", "windowactivate", "--sync", str(window)])

    @staticmethod
    def place(window: int, b: dict) -> None:
        _out(["wmctrl", "-i", "-r", hex(window), "-b", "remove,maximized_vert,maximized_horz"])
        _out(
            [
                "xdotool",
                "windowmove",
                str(window),
                str(b["x"]),
                str(b["y"]),
                "windowsize",
                str(window),
                str(b["width"]),
                str(b["height"]),
            ],
            10,
        )

    @classmethod
    def frames(cls) -> dict[str, list[int]]:
        return {
            (cls.title(w) or cls.wm_class(w) or hex(w)): cls.geometry(w) for w in cls.stacking()
        }

    @staticmethod
    def cursor() -> tuple[float, float] | None:
        m = re.search(r"x:(\d+) y:(\d+)", _out(["xdotool", "getmouselocation"]))
        return (float(m.group(1)), float(m.group(2))) if m else None


def park_pointer() -> dict:
    """Amendment 24 (#6660): move the real pointer to the park point (XTest) and read it back."""
    x, y = v2.PARK_POINT
    _out(["xdotool", "mousemove", "--sync", str(x), str(y)])
    return v2.park_record(X.cursor(), "xdotool mousemove")


# ---------------------------------------------------------------- pack tasks on Linux (port of windows/cdb.py)
def substitute(text: str, values: dict[str, str]) -> str:
    return re.sub(r"\$\{(\w+)\}", lambda m: values.get(m.group(1), m.group(0)), text)


def title_matches(win: dict[str, Any], title: str) -> bool:
    if win.get("title"):
        return title == win["title"] or title.startswith(win["title"] + " ")
    if win.get("title_contains"):
        return win["title_contains"].lower() in title.lower()
    return False


class LinuxCdbTask:
    """One pack task, driven by its own platform/launch.linux.json. Electron apps run as the descriptor says
    (`npx electron .`), as run_bench.py does on Linux."""

    KILL_NAMES = (
        "electron",
        "soffice.bin",
        "soffice",
        "oosplash",
        "gnucash",
        "chrome",
        "google-chrome",
    )

    def __init__(self, spec: dict[str, Any], artifacts: Path, seed: int | None = None) -> None:
        self.spec = spec
        self.seed = seed
        self.bundle = TASKPACK / "tasks" / spec["pack_task"]
        self.descriptor = json.loads(
            (self.bundle / "platform" / "launch.linux.json").read_text("utf-8")
        )
        self.artifacts = artifacts
        self.workspace = Path(
            self.descriptor["workspace_root"].replace("${HOME}", str(Path.home()))
        )
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
            "HOME": str(Path.home()),
            "artifacts": str(self.artifacts),
            **extra,
        }

    def sub(self, text: str, **extra: str) -> str:
        return substitute(text, self.values(**extra))

    def _run(
        self, command: list[str], timeout: int = 300, **extra: str
    ) -> subprocess.CompletedProcess:
        return subprocess.run(
            [self.sub(p, **extra) for p in command],
            cwd=str(self.bundle),
            capture_output=True,
            text=True,
            errors="replace",
            timeout=timeout,
        )

    def ports(self) -> list[int]:
        text = json.dumps(self.descriptor)
        found = {int(p) for p in re.findall(r"127\.0\.0\.1:(\d+)", text)}
        for app in self.descriptor["apps"]:
            port = (app.get("env") or {}).get("CDB_S04_PORT")
            if port:
                found.add(int(port))
        return sorted(found)

    def clear_leftovers(self) -> None:
        """Disposable desktop: the pack apps by name, and whatever listens on the task's loopback ports."""
        for name in self.KILL_NAMES:
            subprocess.run(["pkill", "-9", "-x", name], capture_output=True)
        subprocess.run(
            ["pkill", "-9", "-f", "node_modules/electron/dist/electron"], capture_output=True
        )
        subprocess.run(["pkill", "-9", "-f", "/opt/google/chrome/chrome"], capture_output=True)
        for port in self.ports():
            subprocess.run(["fuser", "-k", "-9", f"{port}/tcp"], capture_output=True)
        time.sleep(0.5)

    def reset(self) -> None:
        self.clear_leftovers()
        shutil.rmtree(self.workspace, ignore_errors=True)
        sem = self.descriptor["semantics"]["reset"]
        for step in ("setup", "verify"):
            done = self._run(sem[step])
            self.log.append(f"reset.{step} rc={done.returncode}")
            if done.returncode != 0:
                raise RuntimeError(f"reset {step} failed: {(done.stderr or done.stdout)[-400:]}")

    def health_check(self) -> list[dict[str, Any]]:
        checks = []
        for name in ("python", "node"):
            path = sys.executable if name == "python" else shutil.which(name)
            checks.append({"name": f"tool:{name}", "ok": bool(path), "path": path})
        bad = [c["name"] for c in checks if not c["ok"]]
        if bad:
            raise RuntimeError("harness health: missing " + ", ".join(bad))
        return checks

    def brief(self) -> str:
        text = (self.bundle / self.descriptor["semantics"].get("brief", "brief.md")).read_text(
            "utf-8"
        )
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
            Path(cwd).mkdir(parents=True, exist_ok=True)
            argv = [self.sub(p) for p in app["command"]]
            for p in argv:  # runtime dirs named in the arguments (GnuCash --logto)
                if p.startswith("--logto="):
                    Path(p.split("=", 1)[1]).parent.mkdir(parents=True, exist_ok=True)
            out = (self.artifacts / f"app-{app['id']}.log").open("ab")
            proc = subprocess.Popen(
                argv,
                cwd=cwd,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=out,
                stderr=out,
                start_new_session=True,
            )
            self.procs.append((app, proc))
            ready = (app.get("ready") or {}).get("url")
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

    def place_windows(self, limit: float = 120.0) -> None:
        pending = [a for a, _ in self.procs if (a.get("window") or {}).get("bounds")]
        deadline = time.monotonic() + limit
        while pending and time.monotonic() < deadline:
            for window in X.visible():
                if window in self.windows.values():
                    continue
                title = X.title(window)
                for app in list(pending):
                    if title_matches(app["window"], title):
                        X.place(window, app["window"]["bounds"])
                        self.windows[app["id"]] = window
                        self.log.append(f"window {app['id']} id={window} title={title!r}")
                        pending.remove(app)
                        break
            time.sleep(0.5)
        if pending:
            raise RuntimeError(
                "windows did not appear: "
                + ", ".join(a["id"] for a in pending)
                + f" (visible: {[X.title(w) for w in X.visible()][:20]})"
            )
        time.sleep(1.5)
        for app_id, window in self.windows.items():  # re-apply: some apps resize after mapping
            app = next(a for a in self.apps() if a["id"] == app_id)
            X.place(window, app["window"]["bounds"])
        time.sleep(0.5)

    def evaluate(self, agent_exit_code: int) -> dict[str, Any]:
        result = self.artifacts / "cdb-result.json"
        result.unlink(missing_ok=True)
        try:
            done = self._run(
                self.descriptor["semantics"]["evaluate"],
                timeout=300,
                result=str(result),
                agent_exit_code=str(agent_exit_code),
            )
            (self.artifacts / "cdb-evaluator.stderr").write_text(
                (done.stderr or "")[-4000:], "utf-8"
            )
        except subprocess.TimeoutExpired:
            pass
        if not result.is_file():
            return {
                "passed": False,
                "score": None,
                "error": "evaluator wrote no result",
                "harness_error": "evaluator_no_result",
            }
        data = json.loads(result.read_text("utf-8"))
        checks_raw = (data.get("detail") or {}).get("checks") or data.get("checks") or {}
        checks = {
            n: bool(c.get("passed")) if isinstance(c, dict) else bool(c)
            for n, c in checks_raw.items()
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
                hs.kill_group(proc)
        self.procs.clear()
        self.clear_leftovers()

    def clean_workspace(self) -> None:
        shutil.rmtree(self.workspace, ignore_errors=True)


# ---------------------------------------------------------------- trial
def start_sentinel(env: dict, out: Path, log: Path, frame, extra: list[str]):
    proc = subprocess.Popen(
        [
            sys.executable,
            str(BENCH / "linux" / "sentinel.py"),
            "--log",
            str(log),
            "--frame",
            ",".join(map(str, frame)),
            *extra,
        ],
        env=env,
        start_new_session=True,
        stdout=(out / "sentinel.log").open("w"),
        stderr=subprocess.STDOUT,
    )
    if not hs.wait_for(lambda: any(ln.get("ev") == "ready" for ln in hs.sentinel_lines(log)), 30):
        return proc, None
    if not hs.wait_for(lambda: X.windows_of(proc.pid), 10):
        return proc, None
    return proc, X.windows_of(proc.pid)[0]


def bring_sentinel_front(proc: subprocess.Popen, own: int) -> bool:
    def in_front() -> bool:
        stack = X.normal_stack()
        return X.active() == own and bool(stack) and stack[-1] == own

    os.kill(proc.pid, signal.SIGUSR1)
    if hs.wait_for(in_front, 5):
        return True
    X.activate(own)
    os.kill(proc.pid, signal.SIGUSR1)
    return hs.wait_for(in_front, 5)


def toggle(proc: subprocess.Popen, log: Path, event: str) -> bool:
    os.kill(proc.pid, signal.SIGUSR2)
    return hs.wait_for(lambda: any(ln.get("ev") == event for ln in hs.sentinel_lines(log)), 5)


def guard_loop(
    G, agent: subprocess.Popen, t0: float, timeout: float, enforce: bool, baseline: set[int]
):
    own_tree: set[int] = set()
    front = G.FrontWatch()
    violations: list[dict] = []
    timed_out = False
    while agent.poll() is None:
        rows = G.process_rows()
        own_tree |= G.descendants(rows, os.getpid())
        for pid, label in G.new_side_doors(rows, baseline, os.getpid(), own_tree):
            violations.append(
                {
                    "t_s": round(time.monotonic() - t0, 1),
                    "kind": "process",
                    "detail": label,
                    "pid": pid,
                }
            )
            hs.kill_pid(pid)
        hit = front.sample(X.active_class())
        if hit:
            violations.append(
                {"t_s": round(time.monotonic() - t0, 1), "kind": "front", "detail": hit}
            )
        if violations and enforce:
            hs.kill_group(agent)
            break
        if time.monotonic() - t0 > timeout:
            timed_out = True
            hs.kill_group(agent)
            break
        time.sleep(0.5)
    try:
        agent.wait(timeout=30)
    except subprocess.TimeoutExpired:
        hs.kill_group(agent)
    return violations, front, timed_out


def read_stream(path: Path) -> list[dict]:
    events = []
    if path.exists():
        for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                events.append(json.loads(raw))
            except json.JSONDecodeError:
                pass
    return events


def run(args: argparse.Namespace, command: list[str]) -> dict:
    G = hs.hyprland_module("guard")
    spec = hs.task_spec(args.task)
    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    background = args.condition == "background"
    cdb = LinuxCdbTask(spec, out, seed=seed) if spec.get("kind") == "cdb" else None
    if cdb is not None:
        mode = spec["pack_task"]
        brief = v2.brief_for(cdb.brief().strip(), args.condition)
    else:
        mode = spec["app_args"][spec["app_args"].index("--mode") + 1]
        brief = hs.render_brief(args.task, seed, args.condition)
    (out / "brief.md").write_text(brief, encoding="utf-8")
    row: dict = {
        "task": args.task,
        "run_index": args.run_index,
        "seed": seed,
        "condition": args.condition,
        "mode": mode,
        "platform": "linux-xfce",
        "label": args.label,
        "started": time.time(),
    }
    state, events, log = out / "state.json", out / "events.jsonl", out / "sentinel.jsonl"
    for stale in (state, events, log):
        stale.unlink(missing_ok=True)
    procs: list[subprocess.Popen] = []
    env = {**os.environ, "GDK_BACKEND": "x11", "NO_AT_BRIDGE": "0"}
    try:
        w, h = X.screen()
        row["screen"] = [w, h]
        if w < 1320 or h < 860:
            row["harness_error"] = f"screen too small: {w}x{h}"
            return row
        row["pointer_parked"] = park_pointer()
        if not row["pointer_parked"]["ok"]:
            row["harness_error"] = f"pointer park failed: {row['pointer_parked']}"
            return row
        first_window = None
        if cdb is not None:
            row["workspace"] = str(cdb.workspace)
            try:
                cdb.reset()
                cdb.start_apps()
                cdb.place_windows()
                row["health"] = cdb.health_check()
            except Exception as error:  # noqa: BLE001 - setup failure = harness error, never scored
                row["harness_error"] = f"setup: {type(error).__name__}: {str(error)[:600]}"
                return row
            finally:
                row["setup_log"] = list(cdb.log)
            first_window = next(iter(cdb.windows.values()), None)
        else:
            lab = subprocess.Popen(
                [
                    sys.executable,
                    str(BENCH / "linux" / "benchlab.py"),
                    "--mode",
                    mode,
                    "--seed",
                    str(seed),
                    "--state",
                    str(state),
                    "--events",
                    str(events),
                ],
                env=env,
                start_new_session=True,
                stdout=(out / "benchlab.log").open("w"),
                stderr=subprocess.STDOUT,
            )
            procs.append(lab)
            if not hs.wait_for(lambda: state.exists() and X.windows_of(lab.pid), 30):
                row["harness_error"] = "BenchLab did not start"
                return row
            first_window = X.windows_of(lab.pid)[0]
            X.place(first_window, dict(zip(("x", "y", "width", "height"), LAB_FRAME)))
        if not background and first_window:
            X.activate(first_window)
        frame = v2.USER_WINDOW_FRAME if background else WITNESS_FRAME
        sentinel, own = start_sentinel(
            env, out, log, frame, ["--user-activity"] if background else []
        )
        procs.append(sentinel)
        if own is None:
            row["harness_error"] = "sentinel did not start"
            return row
        row["sentinel_front_at_start"] = bring_sentinel_front(sentinel, own) if background else None
        if background and not row["sentinel_front_at_start"]:
            st = X.normal_stack()
            row["harness_error"] = (
                f"sentinel could not be brought to the front: own={own} active={X.active()} "
                f"({X.title(X.active())!r}) top={[X.title(w) for w in st[-3:]]} own_in_stack={own in st}"
            )
            return row
        park_pointer()  # the pointer may have moved while windows were placed
        row["frames"] = X.frames()
        time.sleep(1.0)
        toggle(sentinel, log, "armed")
        baseline = {pid for pid, _, _ in G.process_rows()}
        t0 = time.monotonic()
        argv = [
            a.replace("{brief}", str(out / "brief.md"))
            .replace("{out}", str(out))
            .replace("{seed}", str(seed))
            .replace("{mode}", mode)
            for a in command
        ]
        agent = subprocess.Popen(
            argv,
            cwd=str(out),
            env={**os.environ, "CDB_BRIEF_FILE": str(out / "brief.md")},
            start_new_session=True,
        )
        procs.append(agent)
        violations, front, timed_out = guard_loop(
            G, agent, t0, args.timeout, args.enforce, baseline
        )
        row["agent_exit"] = agent.returncode
        row["agent_wall_s"] = round(time.monotonic() - t0, 1)
        row["timed_out"] = timed_out
        toggle(sentinel, log, "disarmed")
        time.sleep(0.3)
        result = out / "result.json"
        if cdb is not None:
            evaluation = cdb.evaluate(124 if timed_out else int(agent.returncode or 0))
            result.write_text(json.dumps(evaluation, indent=1), encoding="utf-8")
            try:
                cdb.snapshot_workspace(out / "workspace-final.zip")
            except OSError:
                pass
            if evaluation.get("harness_error"):
                row["harness_error"] = f"evaluate: {evaluation['harness_error']}"
        else:
            subprocess.run(
                [
                    sys.executable,
                    str(BENCH / "probes" / args.task / "evaluate.py"),
                    "--seed",
                    str(seed),
                    "--state",
                    str(state),
                    "--events",
                    str(events),
                    "--result",
                    str(result),
                ],
                capture_output=True,
                timeout=60,
            )
            evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        soft: list = []
        hard, mentions = G.scan_events(read_stream(out / "agent-transcript.jsonl"))
        soft += mentions
        violations += [{"t_s": None, "kind": "tool_input", "detail": x} for x in hard]
        disturbance = summarize_sentinel.summarize(str(log))
        armed = next((ln for ln in hs.sentinel_lines(log) if ln.get("ev") == "armed"), {})
        frontmost = (armed.get("front") or {}).get("bid") == SENTINEL_ID
        passed = bool(evaluation.get("passed")) and not violations
        bg = v2.background_score(disturbance, frontmost)
        row.update(
            evaluation={k: evaluation.get(k) for k in ("passed", "score", "checks", "diagnostics")},
            checker_passed=bool(evaluation.get("passed")),
            gui_only={
                "violation": bool(violations),
                "violations": violations[:20],
                "mentions": sorted(set(map(str, soft)))[:20],
                "frontmost_seen": sorted(front.seen),
            },
            passed=passed,
            disturbance=disturbance,
            background=bg,
            a15=v2.background_condition_score(bg, disturbance, passed, args.condition),
            a22=v2.multiplayer_score(disturbance, bg["measured"], passed, args.condition),
        )
        return row
    finally:
        for proc in procs:
            if proc.poll() is None:
                hs.kill_group(proc)
        if cdb is not None:
            cdb.stop_apps()
            cdb.clean_workspace()
        row["finished"] = time.time()
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")


def main(argv: list[str]) -> int:
    if "--" not in argv:
        print(
            "usage: xfce_session.py --task ID --run-index N --condition C --out DIR -- COMMAND...",
            file=sys.stderr,
        )
        return 2
    split = argv.index("--")
    parser = argparse.ArgumentParser()
    parser.add_argument("--task", choices=TASKS, required=True)
    parser.add_argument("--run-index", type=int, required=True)
    parser.add_argument("--condition", choices=v2.CONDITIONS, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--label", default="")
    parser.add_argument("--timeout", type=float, default=360.0)
    parser.add_argument("--no-enforce", dest="enforce", action="store_false")
    args = parser.parse_args(argv[:split])
    command = argv[split + 1 :]
    if not os.environ.get("DISPLAY") or not command:
        print("xfce_session.py needs an X11 session (DISPLAY) and a command", file=sys.stderr)
        return 2
    row = run(args, command)
    print(
        json.dumps(
            {k: row.get(k) for k in ("task", "seed", "condition", "passed", "harness_error")}
        )
    )
    return 0 if not row.get("harness_error") else 3


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
