#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Usage: macos_session.py --build DIR --task PA-01|UM-01 --run-index N --condition foreground|background --out DIR
#                         [--timeout S] [--instances N] -- COMMAND [ARG...]
#
# One shared-desktop trial (SHARED-DESKTOP.md) on a macOS desktop, run as the console user in its GUI session. It is
# the macOS counterpart of x11_session.py, hyprland_session.py and windows_session.py, for runs where the agents and
# their tool servers are started from outside the trial (COMMAND can be a stand-in that waits for an agent running
# elsewhere, for example on the host of a macOS VM):
#   1. derive the seed (fairness rule), render the brief(s) (+ the background paragraph);
#   2. start BenchLab (`--mode desk`, DIR/BenchLab.app) and BenchSentinel (DIR/BenchSentinel.app): in the
#      background condition the "User activity" window partly covers the desks, is brought to the front and types
#      (PA-01) or moves and clicks the real pointer from a seeded script (UM-01);
#   3. arm the sentinel, run COMMAND (`{brief}`, `{out}`, `{seed}`, `{mode}` in its arguments are filled in) with the
#      GUI-only guard watching (rules 1 and 2: a terminal or scripting app in front or running, input to a Terminal
#      session), stop it at the time limit or on a violation;
#   4. disarm, run the task's checker, summarise the sentinel, scan every agent stream found in the output
#      directories (rules 3 and 4), write DIR/row.json.
# PA-01 [--instances N]: N BenchLab copies in the 5x2 grid, one COMMAND per copy, all started at the same moment;
#   `{brief}` and `{out}` are that agent's own (DIR/desk-L/), plus `{instance}` and `{title}`; CDB_BRIEF_FILE and
#   CDB_INSTANCE are in its environment. Any GUI-only violation stops all of them. Needs a 1920x1080 screen.
# UM-01 (background only): one desk; the sentinel needs Accessibility (to post the user's events) and Input
#   Monitoring (for the listen-only tap that tells the user's tagged events from anyone else's).
# An agent's stream is read from `agent-transcript.jsonl` in its output directory once COMMAND has ended.
# TCC attributes a process to its responsible ancestor. When the trial is started over ssh, pass
# `--sentinel-launcher HELPER`: a helper that runs its arguments as their own responsible process, so the
# sentinel's own Accessibility and Input Monitoring grants apply (not those of sshd).
from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import signal
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "probes" / "_common"))
sys.path.insert(0, str(BENCH / "swift"))
sys.path.insert(0, str(BENCH.parents[1] / "fairness"))
import bench_v2 as v2  # noqa: E402
import cdb_fairness  # noqa: E402
import shared_desktop  # noqa: E402
import summarize_sentinel  # noqa: E402

TASKS = ("PA-01", "UM-01")
LSREGISTER = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
BENCH_APPS = ("BenchLab.app", "BenchSentinel.app")
SENTINEL_ID = "ai.cua.benchsentinel"
PA_MIN_SCREEN = (1920, 1000)


def exe(build: Path, name: str) -> str:
    return str(build / f"{name}.app" / "Contents" / "MacOS" / name)


def unregister_stale_copies(build: Path) -> list[str]:
    """As run_bench.py: a computer-use server that resolves apps by bundle id refuses an ambiguous one, so only the
    copies in the build dir may be registered with LaunchServices. Reversible (lsregister -u; files are left alone).
    """
    try:
        dump = subprocess.run(
            [LSREGISTER, "-dump"], capture_output=True, text=True, timeout=180
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return []
    found = {m.strip() for m in re.findall(r"^path:\s+(.*?\.app)\s+\(0x", dump, re.M)}
    gone = []
    for path in sorted(found):
        if (
            Path(path).name in BENCH_APPS
            and Path(path).exists()
            and Path(path).resolve().parent != build
        ):
            subprocess.run([LSREGISTER, "-u", path], capture_output=True, timeout=60)
            gone.append(path)
    return gone


def kill_group(proc: subprocess.Popen) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(proc.pid, sig)
        except OSError:
            return
        try:
            proc.wait(timeout=3)
            return
        except subprocess.TimeoutExpired:
            continue


def wait_for(predicate, limit: float, step: float = 0.1) -> bool:
    end = time.monotonic() + limit
    while time.monotonic() < end:
        if predicate():
            return True
        time.sleep(step)
    return False


def sentinel_lines(log: Path) -> list[dict]:
    if not log.exists():
        return []
    out = []
    for raw in log.read_text(encoding="utf-8").splitlines():
        try:
            out.append(json.loads(raw))
        except json.JSONDecodeError:
            pass
    return out


def substitute(command: list[str], subs: dict[str, str]) -> list[str]:
    argv = []
    for a in command:
        for key, value in subs.items():
            a = a.replace(key, value)
        argv.append(a)
    return argv


def frontmost_bundle() -> str | None:
    """Bundle id of the frontmost app through lsappinfo (no TCC permission needed)."""
    try:
        asn = subprocess.run(
            ["/usr/bin/lsappinfo", "front"], capture_output=True, text=True, timeout=3
        ).stdout.strip()
        if not asn:
            return None
        out = subprocess.run(
            ["/usr/bin/lsappinfo", "info", "-only", "bundleid", asn],
            capture_output=True,
            text=True,
            timeout=3,
        ).stdout
        match = re.search(r'"([A-Za-z0-9._-]+)"\s*$', out.strip())
        return match.group(1) if match else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def screen_size() -> tuple[int, int]:
    """The main display's size in points (CGDisplayBounds through ctypes: no extra package, no permission)."""
    import ctypes

    class CGRect(ctypes.Structure):
        _fields_ = [
            ("x", ctypes.c_double),
            ("y", ctypes.c_double),
            ("w", ctypes.c_double),
            ("h", ctypes.c_double),
        ]

    try:
        cg = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
        cg.CGMainDisplayID.restype = ctypes.c_uint32
        cg.CGDisplayBounds.restype = CGRect
        cg.CGDisplayBounds.argtypes = [ctypes.c_uint32]
        rect = cg.CGDisplayBounds(cg.CGMainDisplayID())
        return int(rect.w), int(rect.h)
    except (OSError, AttributeError):
        return (0, 0)


def app_env(home: Path) -> dict:
    """A throwaway HOME per app process, as the runner gives BenchLab and the sentinel."""
    (home / "tmp").mkdir(parents=True, exist_ok=True)
    keep = ("PATH", "USER", "LOGNAME", "LANG", "LC_ALL", "__CF_USER_TEXT_ENCODING")
    return {
        **{k: v for k, v in os.environ.items() if k in keep},
        "HOME": str(home),
        "TMPDIR": str(home / "tmp") + "/",
    }


def start_app(argv: list[str], home: Path, log: Path) -> subprocess.Popen:
    return subprocess.Popen(
        argv,
        env=app_env(home),
        start_new_session=True,
        stdin=subprocess.DEVNULL,
        stdout=log.open("w"),
        stderr=subprocess.STDOUT,
    )


class Sentinel:
    def __init__(
        self, build: Path, out: Path, log: Path, args: list[str], launcher: list[str]
    ) -> None:
        self.argv = [*launcher, exe(build, "BenchSentinel"), "--log", str(log), *args]
        self.out, self.log = out, log
        self.proc: subprocess.Popen | None = None
        self.pid: int | None = None  # the sentinel's own pid (a launcher may run it as a child)

    def start(self) -> bool:
        self.proc = start_app(self.argv, self.out / "sentinel-home", self.out / "sentinel.stderr")
        if not wait_for(
            lambda: any(ln.get("ev") == "start" for ln in sentinel_lines(self.log)), 15
        ):
            return False
        self.pid = next(
            int(ln["pid"]) for ln in sentinel_lines(self.log) if ln.get("ev") == "start"
        )
        return True

    def signal(self, sig: int) -> None:
        if self.pid is not None:
            try:
                os.kill(self.pid, sig)
            except OSError:
                pass

    def stop(self) -> None:
        if self.proc is not None and self.proc.poll() is None:
            kill_group(self.proc)
        for sig in (signal.SIGTERM, signal.SIGKILL):
            self.signal(sig)
            time.sleep(0.5)

    def bring_front(self) -> bool:
        for _ in range(3):
            self.signal(signal.SIGUSR1)
            if wait_for(lambda: frontmost_bundle() == SENTINEL_ID, 3, 0.2):
                return True
        return False

    def set_armed(self, armed: bool) -> bool:
        self.signal(signal.SIGUSR2)
        return wait_for(
            lambda: any(ln.get("armed") is armed for ln in sentinel_lines(self.log)[-5:]), 5
        )


def guard_loop(
    guard: v2.GuiOnlyGuard, agents: list[dict], t0: float, timeout: float, enforce: bool
) -> bool:
    """Run until every COMMAND ended, the time limit, or (enforcing) a GUI-only violation. Returns timed out."""

    def running() -> list[dict]:
        live = []
        for entry in agents:
            if entry["proc"].poll() is None:
                live.append(entry)
            elif entry["wall_s"] is None:
                entry["wall_s"] = round(time.monotonic() - t0, 1)
        return live

    timed_out = False
    guard.start()
    try:
        while running():
            if enforce and guard.stop.is_set():
                break
            if time.monotonic() - t0 > timeout:
                timed_out = True
                for entry in running():
                    entry["timed_out"] = True
                break
            time.sleep(0.5)
    finally:
        guard.finish()
    for entry in agents:
        if entry["proc"].poll() is None:
            kill_group(entry["proc"])
        if entry["wall_s"] is None:
            entry["wall_s"] = round(time.monotonic() - t0, 1)
    return timed_out


def read_stream(path: Path) -> list[dict]:
    events = []
    if not path.exists():
        return events
    for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            events.append(json.loads(raw))
        except json.JSONDecodeError:
            pass
    return events


def score(
    row: dict,
    condition: str,
    guard: v2.GuiOnlyGuard,
    evaluation: dict,
    log: Path,
    streams: list[Path],
    sentinel_frontmost: bool,
) -> None:
    hard: list[str] = []
    soft: list[str] = []
    for stream in streams:
        h, s = v2.scan_events(read_stream(stream))
        hard += h
        soft += s
    gui = guard.summary(sorted(set(hard)), sorted(set(soft)))
    disturbance = summarize_sentinel.summarize(str(log))
    passed = bool(evaluation.get("passed")) and not gui["violation"]
    bg = v2.background_score(disturbance, sentinel_frontmost)
    row.update(
        evaluation={k: evaluation.get(k) for k in ("passed", "score", "checks", "diagnostics")},
        checker_passed=bool(evaluation.get("passed")),
        gui_only=gui,
        passed=passed,
        disturbance=disturbance,
        background=bg,
        a15=v2.background_condition_score(bg, disturbance, passed, condition),
        a22=v2.multiplayer_score(disturbance, bg["measured"], passed, condition),
    )


def new_guard(enforce: bool) -> v2.GuiOnlyGuard:
    # The agents' streams live elsewhere while they run: rules 1-2 are watched live here, rules 3-4 are scanned
    # from the final streams (score()).
    return v2.GuiOnlyGuard(
        Path(os.devnull),
        frontmost_bundle,
        enforce=enforce,
        terminal_ttys=lambda: v2.terminal_tty_inputs(os.getpid()),
    )


def start_command(
    command: list[str], subs: dict[str, str], cwd: Path, env: dict
) -> subprocess.Popen:
    return subprocess.Popen(
        substitute(command, subs), cwd=str(cwd), env={**os.environ, **env}, start_new_session=True
    )


def run(args: argparse.Namespace, command: list[str]) -> dict:
    spec = json.loads((BENCH / "probes" / args.task / "task.json").read_text(encoding="utf-8"))
    if shared_desktop.is_parallel(spec):
        return run_parallel(args, command, spec)
    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    render = shared_desktop._renderer(args.task)
    (out / "brief.md").write_text(
        v2.brief_for(render.render(seed), args.condition, mouse_user=True), encoding="utf-8"
    )
    row: dict = {
        "task": args.task, "run_index": args.run_index, "seed": seed, "condition": args.condition,
        "mode": "desk", "platform": "macos", "label": args.label, "started": time.time(),
    }  # fmt: skip
    desk = out / "desk"
    desk.mkdir(exist_ok=True)
    state, events, log = desk / "state.json", desk / "events.jsonl", out / "sentinel.jsonl"
    for stale in (state, events, log):
        if stale.exists():
            stale.unlink()
    procs: list[subprocess.Popen] = []
    sentinel: Sentinel | None = None
    try:
        script = shared_desktop.write_mouse_script(seed, out / "mouse-user.json")
        row["mouse_user_script"] = {
            "dots": len(script["dots"]),
            "sequence": len(script["sequence"]),
        }
        lab = start_app(
            [exe(args.build, "BenchLab"), "--mode", "desk", "--seed", str(seed), "--state", str(state),
             "--events", str(events), *shared_desktop.um_app_args(seed), "--no-activate"],
            out / "apphome", desk / "benchlab.log",
        )  # fmt: skip
        procs.append(lab)
        if not wait_for(state.exists, 30):
            row["harness_error"] = "BenchLab did not start"
            return row
        sentinel = Sentinel(
            args.build,
            out,
            log,
            v2.sentinel_args("background", out / "mouse-user.json"),
            args.launcher,
        )
        ok = sentinel.start()
        procs.append(sentinel.proc)
        if not ok:
            row["harness_error"] = "sentinel did not start"
            return row
        time.sleep(1.0)
        row["sentinel_front_at_start"] = sentinel.bring_front()
        front = frontmost_bundle() == SENTINEL_ID
        if not sentinel.set_armed(True):
            row["harness_error"] = "sentinel did not arm"
            return row
        guard = new_guard(args.enforce)
        t0 = time.monotonic()
        subs = {
            "{brief}": str(out / "brief.md"),
            "{out}": str(out),
            "{seed}": str(seed),
            "{mode}": "desk",
        }
        agent = start_command(command, subs, out, {"CDB_BRIEF_FILE": str(out / "brief.md")})
        procs.append(agent)
        entry = {"proc": agent, "wall_s": None, "timed_out": False}
        row["timed_out"] = guard_loop(guard, [entry], t0, args.timeout, args.enforce)
        row["agent_exit"] = agent.returncode
        row["agent_wall_s"] = entry["wall_s"]
        sentinel.set_armed(False)
        time.sleep(0.3)
        result = out / "result.json"
        subprocess.run(
            [sys.executable, str(BENCH / "probes" / args.task / "evaluate.py"), "--seed", str(seed),
             "--state", str(state), "--events", str(events), "--result", str(result), "--sentinel", str(log)],
            capture_output=True, timeout=60,
        )  # fmt: skip
        evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        score(row, args.condition, guard, evaluation, log, [out / "agent-transcript.jsonl"], front)
        problem = v2.mouse_user_problem(row["disturbance"]) or evaluation.get("harness_error")
        if problem:
            row["harness_error"] = problem
        return row
    finally:
        if sentinel is not None:
            sentinel.stop()
        for proc in procs:
            if proc is not None and proc.poll() is None:
                kill_group(proc)
        row["finished"] = time.time()
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")


def run_parallel(args: argparse.Namespace, command: list[str], spec: dict) -> dict:
    """PA-01: N desks, N agents started at the same moment, one GUI-only guard over all of them."""
    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    instances = shared_desktop.instances_for(spec, args.instances)
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    background = args.condition == "background"
    row: dict = {
        "task": args.task, "run_index": args.run_index, "seed": seed, "condition": args.condition,
        "mode": "desk", "platform": "macos", "label": args.label, "started": time.time(),
        "instances": instances,
    }  # fmt: skip
    log = out / "sentinel.jsonl"
    if log.exists():
        log.unlink()
    agents = shared_desktop.pa_agents(seed, instances, out)
    for entry in agents:
        entry["dir"].mkdir(parents=True, exist_ok=True)
        for stale in (entry["state"], entry["events"]):
            if stale.exists():
                stale.unlink()
        (entry["dir"] / "brief.md").write_text(
            v2.brief_for(entry["brief"], args.condition), encoding="utf-8"
        )
    procs: list[subprocess.Popen] = []
    sentinel: Sentinel | None = None
    try:
        width, height = screen_size()
        if width < PA_MIN_SCREEN[0] or height < PA_MIN_SCREEN[1]:
            row["harness_error"] = (
                f"screen {width}x{height} smaller than {PA_MIN_SCREEN[0]}x{PA_MIN_SCREEN[1]}"
            )
            return row
        for entry in agents:
            entry["lab"] = start_app(
                [exe(args.build, "BenchLab"), "--mode", "desk", *entry["app_args"],
                 *(["--no-activate"] if background else [])],
                out / f"apphome-{entry['label']}", entry["dir"] / "benchlab.log",
            )  # fmt: skip
            procs.append(entry["lab"])
        if not wait_for(lambda: all(e["state"].exists() for e in agents), 30):
            missing = [e["label"] for e in agents if not e["state"].exists()]
            row["harness_error"] = f"BenchLab copies did not start: {missing}"
            return row
        time.sleep(1.5)
        sentinel_args = v2.sentinel_args(args.condition) or [
            "--frame", ",".join(map(str, shared_desktop.PA_WITNESS_FRAME)), "--no-activate"
        ]  # fmt: skip
        sentinel = Sentinel(args.build, out, log, sentinel_args, args.launcher)
        ok = sentinel.start()
        procs.append(sentinel.proc)
        if not ok:
            row["harness_error"] = "sentinel did not start"
            return row
        time.sleep(1.0)
        row["sentinel_front_at_start"] = sentinel.bring_front() if background else None
        front = frontmost_bundle() == SENTINEL_ID
        if not sentinel.set_armed(True):
            row["harness_error"] = "sentinel did not arm"
            return row
        guard = new_guard(args.enforce)
        t0_ms = time.time() * 1000
        t0 = time.monotonic()
        shared_desktop.write_meta(out, instances, t0_ms, args.timeout)
        for entry in agents:
            subs = {"{brief}": str(entry["dir"] / "brief.md"), "{out}": str(entry["dir"]), "{seed}": str(seed),
                    "{mode}": "desk", "{instance}": entry["label"], "{title}": entry["title"]}  # fmt: skip
            entry["proc"] = start_command(
                command, subs, entry["dir"],
                {"CDB_BRIEF_FILE": str(entry["dir"] / "brief.md"), "CDB_INSTANCE": entry["label"]},
            )  # fmt: skip
            entry["wall_s"], entry["timed_out"] = None, False
        procs.extend(entry["proc"] for entry in agents)
        row["timed_out"] = guard_loop(guard, agents, t0, args.timeout, args.enforce)
        row["agent_wall_s"] = round(time.monotonic() - t0, 1)
        sentinel.set_armed(False)
        time.sleep(0.3)
        result = out / "result.json"
        subprocess.run(
            shared_desktop.evaluate_argv(sys.executable, seed, out, result),
            capture_output=True,
            timeout=60,
        )
        evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        if evaluation.get("harness_error"):
            row["harness_error"] = evaluation["harness_error"]
        score(row, args.condition, guard, evaluation, log,
              [entry["dir"] / "agent-transcript.jsonl" for entry in agents], front)  # fmt: skip
        row["parallel"] = shared_desktop.parallel_of(evaluation)
        row["agents"] = shared_desktop.aggregate_agents(
            [
                {
                    "label": e["label"],
                    "wall_s": e["wall_s"],
                    "timed_out": e["timed_out"],
                    "returncode": e["proc"].returncode,
                }
                for e in agents
            ]  # fmt: skip
        )
        return row
    finally:
        if sentinel is not None:
            sentinel.stop()
        for proc in procs:
            if proc is not None and proc.poll() is None:
                kill_group(proc)
        row["finished"] = time.time()
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")


def main(argv: list[str]) -> int:
    if "--" not in argv:
        print(
            "usage: macos_session.py --build DIR --task ID --run-index N --condition C --out DIR -- COMMAND...",
            file=sys.stderr,
        )
        return 2
    split = argv.index("--")
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--build", type=Path, required=True, help="dir with BenchLab.app and BenchSentinel.app"
    )
    parser.add_argument("--task", choices=TASKS, required=True)
    parser.add_argument("--run-index", type=int, required=True)
    parser.add_argument("--condition", choices=v2.CONDITIONS, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--label", default="")
    parser.add_argument("--timeout", type=float, default=360.0)
    parser.add_argument("--instances", type=int, help="PA-01: number of BenchLab copies and agents")
    parser.add_argument("--no-enforce", dest="enforce", action="store_false")
    parser.add_argument(
        "--sentinel-launcher",
        default="",
        help="command prefix that starts BenchSentinel as its own responsible process (TCC), e.g. over ssh",
    )
    args = parser.parse_args(argv[:split])
    args.launcher = shlex.split(args.sentinel_launcher)
    if args.task == "UM-01" and args.condition != "background":
        parser.error("UM-01 runs only in the background condition")
    if args.instances is not None and args.task != "PA-01":
        parser.error("--instances applies to PA-01 only")
    command = argv[split + 1 :]
    if not command:
        print("macos_session.py needs a command", file=sys.stderr)
        return 2
    args.build = args.build.resolve()
    unregistered = unregister_stale_copies(args.build)
    row = run(args, command)
    if unregistered:
        row["unregistered_copies"] = unregistered
        (args.out.resolve() / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")
    print(
        json.dumps(
            {k: row.get(k) for k in ("task", "seed", "condition", "passed", "harness_error")}
        )
    )
    return 0 if not row.get("harness_error") else 3


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
