#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Usage: x11_session.py --task ID --run-index N --condition foreground|background --out DIR
#                       [--timeout S] [--instances N] -- COMMAND [ARG...]
#
# One BenchLab trial on an existing X11 desktop with an EWMH window manager (for example XFCE or openbox), run as
# the desktop user with the session's environment (DISPLAY, XAUTHORITY, DBUS_SESSION_BUS_ADDRESS). It is the X11
# counterpart of hyprland_session.py and windows_session.py, for runs where the agent and its tool server are
# started from outside the trial (COMMAND can be a stand-in that waits for an agent running elsewhere):
#   1. derive the seed (fairness rule), render the brief (+ the background paragraph), write DIR/brief.md;
#   2. start BenchLab (linux/benchlab.py, GTK3) and the sentinel (linux/sentinel.py) at their frames: in the
#      background condition the "User activity" window partly covers BenchLab, is brought to the front and types;
#   3. arm the sentinel, run COMMAND (`{brief}`, `{out}`, `{seed}`, `{mode}` in its arguments are filled in) with
#      the GUI-only guard watching, stop it at the time limit or on a violation;
#   4. disarm, run the task's checker, summarise the sentinel, write DIR/row.json.
# Shared-desktop tasks (SHARED-DESKTOP.md), with the same contract as hyprland_session.py:
#   PA-01 [--instances N]: N BenchLab copies (`--mode desk`) in the 5x2 grid, one agent process each, all started at
#     the same moment; `{brief}` and `{out}` are that agent's own (DIR/desk-L/), plus `{instance}` and `{title}`;
#     CDB_BRIEF_FILE and CDB_INSTANCE are in its environment. Any agent's GUI-only violation stops all of them.
#     Needs a screen of at least 1920x1000.
#   UM-01 (background only): one desk, and the sentinel's user moves the real pointer and clicks through XTest
#     from a seeded script (trajectory check only: X11 cannot tell XTest input apart).
# The GUI-only guard is the Linux one of hyprland/guard.py (terminal processes and windows, exec patterns in tool
# inputs); it reads only /proc, window classes and the agent's stream, so it applies to X11 unchanged.
# Needs xdotool and xprop on PATH, and libXtst for UM-01.
from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import hyprland_session as hs  # noqa: E402  (shared, compositor-independent helpers)

v2, cdb_fairness, shared_desktop, summarize_sentinel = (
    hs.v2, hs.cdb_fairness, hs.shared_desktop, hs.summarize_sentinel,
)  # fmt: skip
BENCH = hs.BENCH
TASKS = hs.TASKS
LAB_FRAME = hs.LAB_FRAME
WITNESS_FRAME = hs.WITNESS_FRAME
PA_WITNESS_FRAME = hs.PA_WITNESS_FRAME
PA_MIN_SCREEN = hs.PA_MIN_SCREEN
PLATFORM_WORDS = hs.PLATFORM_WORDS
SENTINEL_ID = "ai.cua.benchsentinel"


class X:
    """The few window-manager queries a trial needs, through xdotool and xprop (EWMH)."""

    @staticmethod
    def _out(argv: list[str], timeout: float = 5.0) -> str:
        try:
            return subprocess.run(argv, capture_output=True, text=True, timeout=timeout).stdout
        except (OSError, subprocess.SubprocessError):
            return ""

    @classmethod
    def screen(cls) -> tuple[int, int]:
        parts = cls._out(["xdotool", "getdisplaygeometry"]).split()
        return (int(parts[0]), int(parts[1])) if len(parts) == 2 else (0, 0)

    @classmethod
    def windows_of(cls, pid: int) -> list[int]:
        return [
            int(w)
            for w in cls._out(["xdotool", "search", "--onlyvisible", "--pid", str(pid)]).split()
        ]

    @classmethod
    def wm_class(cls, window: int) -> str:
        names = re.findall(r'"([^"]+)"', cls._out(["xprop", "-id", str(window), "WM_CLASS"]))
        return names[-1] if names else ""

    @classmethod
    def active(cls) -> int:
        text = cls._out(["xdotool", "getactivewindow"]).strip()
        return int(text) if text.isdigit() else 0

    @classmethod
    def active_class(cls) -> str:
        window = cls.active()
        return cls.wm_class(window) if window else ""

    @classmethod
    def stacking(cls) -> list[int]:
        text = cls._out(["xprop", "-root", "_NET_CLIENT_LIST_STACKING"])
        return [int(v, 16) for v in re.findall(r"0x[0-9a-fA-F]+", text)]

    @classmethod
    def geometry(cls, window: int) -> list[int]:
        """The window's outer frame (x, y, w, h) as the window manager reports it."""
        shell = dict(
            line.split("=", 1)
            for line in cls._out(["xdotool", "getwindowgeometry", "--shell", str(window)]).split()
            if "=" in line
        )
        try:
            return [int(shell[k]) for k in ("X", "Y", "WIDTH", "HEIGHT")]
        except (KeyError, ValueError):
            return []

    @classmethod
    def title(cls, window: int) -> str:
        return cls._out(["xdotool", "getwindowname", str(window)]).strip()

    @classmethod
    def activate(cls, window: int) -> None:
        cls._out(["xdotool", "windowactivate", "--sync", str(window)])

    @classmethod
    def frames(cls) -> dict[str, list[int]]:
        out = {}
        for window in cls.stacking():
            name = cls.title(window) or cls.wm_class(window) or hex(window)
            out[name] = cls.geometry(window)
        return out


def start_sentinel(env: dict, out: Path, log: Path, frame, extra: list[str]):
    """Start linux/sentinel.py at `frame`; returns (process, its X window or None)."""
    proc = subprocess.Popen(
        [sys.executable, str(BENCH / "linux" / "sentinel.py"), "--log", str(log),
         "--frame", ",".join(map(str, frame)), *extra],
        env=env, start_new_session=True, stdout=(out / "sentinel.log").open("w"), stderr=subprocess.STDOUT,
    )  # fmt: skip
    if not hs.wait_for(lambda: any(ln.get("ev") == "ready" for ln in hs.sentinel_lines(log)), 30):
        return proc, None
    if not hs.wait_for(lambda: X.windows_of(proc.pid), 10):
        return proc, None
    return proc, X.windows_of(proc.pid)[0]


def bring_sentinel_front(proc: subprocess.Popen, own: int) -> bool:
    def in_front() -> bool:
        stack = X.stacking()
        return X.active() == own and bool(stack) and stack[-1] == own

    os.kill(proc.pid, signal.SIGUSR1)  # the sentinel presents itself and focuses its own widget
    if hs.wait_for(in_front, 5):
        return True
    X.activate(own)  # the window manager may refuse a self-raise (focus-stealing prevention)
    os.kill(proc.pid, signal.SIGUSR1)
    return hs.wait_for(in_front, 5)


def toggle(proc: subprocess.Popen, log: Path, event: str) -> bool:
    os.kill(proc.pid, signal.SIGUSR2)
    return hs.wait_for(lambda: any(ln.get("ev") == event for ln in hs.sentinel_lines(log)), 5)


def screen_too_small(width: int, height: int) -> bool:
    w, h = X.screen()
    return w < width or h < height


def guard_loop(G, agents: list[dict], t0: float, timeout: float, enforce: bool, baseline: set[int]):
    """Watch every agent process: terminal processes or windows (G), the time limit. Returns
    (violations, front watch, timed out)."""
    own_tree: set[int] = set()
    front = G.FrontWatch()
    violations: list[dict] = []
    timed_out = False

    def running() -> list[dict]:
        live = []
        for entry in agents:
            if entry["proc"].poll() is None:
                live.append(entry)
            elif entry["wall_s"] is None:
                entry["wall_s"] = round(time.monotonic() - t0, 1)
        return live

    def kill_all(is_timeout: bool = False) -> None:
        for entry in running():
            entry["timed_out"] = is_timeout
            hs.kill_group(entry["proc"])

    while running():
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
            kill_all()
            break
        if time.monotonic() - t0 > timeout:
            timed_out = True
            kill_all(is_timeout=True)
            break
        time.sleep(0.5)
    for entry in agents:
        try:
            entry["proc"].wait(timeout=30)
        except subprocess.TimeoutExpired:
            hs.kill_group(entry["proc"])
        if entry["wall_s"] is None:
            entry["wall_s"] = round(time.monotonic() - t0, 1)
    return violations, front, timed_out


def session_env() -> dict:
    return {**os.environ, "GDK_BACKEND": "x11", "NO_AT_BRIDGE": "0"}


def score(
    row: dict, args, G, evaluation: dict, log: Path, violations: list, front, streams: list[Path]
) -> None:
    soft: list = []
    for stream in streams:
        hard, mentions = G.scan_events(hs.read_stream(stream))
        soft += mentions
        violations += [{"t_s": None, "kind": "tool_input", "detail": h} for h in hard]
    disturbance = summarize_sentinel.summarize(str(log))
    armed = next((ln for ln in hs.sentinel_lines(log) if ln.get("ev") == "armed"), {})
    frontmost = (armed.get("front") or {}).get("bid") == SENTINEL_ID
    passed = bool(evaluation.get("passed")) and not violations
    bg = v2.background_score(disturbance, frontmost)
    row.update(
        evaluation={k: evaluation.get(k) for k in ("passed", "score", "checks", "diagnostics")},
        checker_passed=bool(evaluation.get("passed")),
        gui_only={"violation": bool(violations), "violations": violations[:20], "mentions": sorted(set(soft))[:20],
                  "frontmost_seen": sorted(front.seen)},
        passed=passed,
        disturbance=disturbance,
        background=bg,
        a15=v2.background_condition_score(bg, disturbance, passed, args.condition),
        a22=v2.multiplayer_score(disturbance, bg["measured"], passed, args.condition),
    )  # fmt: skip


def run(args: argparse.Namespace, command: list[str]) -> dict:
    G = hs.hyprland_module("guard")
    spec = hs.task_spec(args.task)
    if spec.get("parallel"):
        return run_parallel(args, command, G, spec)
    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    mode = spec["app_args"][spec["app_args"].index("--mode") + 1]
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    um = bool(spec.get("mouse_user"))
    (out / "brief.md").write_text(
        hs.render_brief(args.task, seed, args.condition, mouse_user=um), encoding="utf-8"
    )
    background = args.condition == "background"
    row: dict = {
        "task": args.task, "run_index": args.run_index, "seed": seed, "condition": args.condition,
        "mode": mode, "platform": "linux-x11", "label": args.label, "started": time.time(),
    }  # fmt: skip
    state, events, log = out / "state.json", out / "events.jsonl", out / "sentinel.jsonl"
    for stale in (state, events, log):
        stale.unlink(missing_ok=True)
    procs: list[subprocess.Popen] = []
    try:
        if screen_too_small(1320, 860):
            row["harness_error"] = "screen smaller than 1320x860"
            return row
        env = session_env()
        lab_args = [
            "--mode",
            mode,
            "--seed",
            str(seed),
            "--state",
            str(state),
            "--events",
            str(events),
        ]
        sentinel_extra = ["--user-activity"] if background else []
        if um:
            if not background:
                row["harness_error"] = "UM-01 runs only in the background condition"
                return row
            script = shared_desktop.write_mouse_script(seed, out / "mouse-user.json")
            lab_args += shared_desktop.um_app_args(seed)
            sentinel_extra = v2.sentinel_args("background", mouse_script=out / "mouse-user.json")[
                :2
            ]
            row["mouse_user_script"] = {
                "dots": len(script["dots"]),
                "sequence": len(script["sequence"]),
            }
        lab = subprocess.Popen(
            [sys.executable, str(BENCH / "linux" / "benchlab.py"), *lab_args],
            env=env, start_new_session=True, stdout=(out / "benchlab.log").open("w"), stderr=subprocess.STDOUT,
        )  # fmt: skip
        procs.append(lab)
        if not hs.wait_for(lambda: state.exists() and X.windows_of(lab.pid), 30):
            row["harness_error"] = "BenchLab did not start"
            return row
        lab_window = X.windows_of(lab.pid)[0]
        if not um:  # the desk places itself (--frame); the other modes take the usual frame
            subprocess.run(["xdotool", "windowmove", str(lab_window), str(LAB_FRAME[0]), str(LAB_FRAME[1]),
                            "windowsize", str(lab_window), str(LAB_FRAME[2]), str(LAB_FRAME[3])],
                           capture_output=True, timeout=10)  # fmt: skip
        if not background:
            X.activate(lab_window)
        frame = v2.USER_WINDOW_FRAME if background else WITNESS_FRAME
        sentinel, own = start_sentinel(env, out, log, frame, sentinel_extra)
        procs.append(sentinel)
        if own is None:
            row["harness_error"] = "sentinel did not start"
            return row
        row["sentinel_front_at_start"] = bring_sentinel_front(sentinel, own) if background else None
        row["frames"] = X.frames()
        time.sleep(1.0)
        toggle(sentinel, log, "armed")
        baseline = {pid for pid, _, _ in G.process_rows()}
        t0 = time.monotonic()
        argv = hs.substitute(command, {"{brief}": str(out / "brief.md"), "{out}": str(out), "{seed}": str(seed),
                                       "{mode}": mode})  # fmt: skip
        agent = subprocess.Popen(
            argv,
            cwd=str(out),
            env={**os.environ, "CDB_BRIEF_FILE": str(out / "brief.md")},
            start_new_session=True,
        )
        procs.append(agent)
        entry = {"proc": agent, "wall_s": None, "timed_out": False}
        violations, front, timed_out = guard_loop(
            G, [entry], t0, args.timeout, args.enforce, baseline
        )
        row["agent_exit"] = agent.returncode
        row["agent_wall_s"] = round(time.monotonic() - t0, 1)
        row["timed_out"] = timed_out
        toggle(sentinel, log, "disarmed")
        time.sleep(0.3)
        result = out / "result.json"
        checker = [sys.executable, str(BENCH / "probes" / args.task / "evaluate.py"), "--seed", str(seed),
                   "--state", str(state), "--events", str(events), "--result", str(result)]  # fmt: skip
        if um:
            checker += ["--sentinel", str(log)]
        subprocess.run(checker, capture_output=True, timeout=60)
        evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        score(row, args, G, evaluation, log, violations, front, [out / "agent-transcript.jsonl"])
        if um:
            problem = v2.mouse_user_problem(row["disturbance"]) or evaluation.get("harness_error")
            if problem:
                row["harness_error"] = problem
        return row
    finally:
        for proc in procs:
            if proc.poll() is None:
                hs.kill_group(proc)
        row["finished"] = time.time()
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")


def run_parallel(args: argparse.Namespace, command: list[str], G, spec: dict) -> dict:
    """PA-01: N desks, N agents started at the same moment, one GUI-only guard over all of them."""
    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    instances = shared_desktop.instances_for(spec, args.instances)
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    background = args.condition == "background"
    row: dict = {
        "task": args.task, "run_index": args.run_index, "seed": seed, "condition": args.condition,
        "mode": "desk", "platform": "linux-x11", "label": args.label, "started": time.time(),
        "instances": instances,
    }  # fmt: skip
    log = out / "sentinel.jsonl"
    log.unlink(missing_ok=True)
    agents = shared_desktop.pa_agents(seed, instances, out, PLATFORM_WORDS)
    for entry in agents:
        entry["dir"].mkdir(parents=True, exist_ok=True)
        entry["state"].unlink(missing_ok=True)
        entry["events"].unlink(missing_ok=True)
        (entry["dir"] / "brief.md").write_text(
            v2.brief_for(entry["brief"], args.condition), encoding="utf-8"
        )
    procs: list[subprocess.Popen] = []
    try:
        if screen_too_small(*PA_MIN_SCREEN):
            row["harness_error"] = f"screen smaller than {PA_MIN_SCREEN[0]}x{PA_MIN_SCREEN[1]}"
            return row
        env = session_env()
        for entry in agents:
            entry["lab"] = subprocess.Popen(
                [sys.executable, str(BENCH / "linux" / "benchlab.py"), "--mode", "desk", *entry["app_args"],
                 *(["--no-activate"] if background else [])],
                env=env, start_new_session=True,
                stdout=(entry["dir"] / "benchlab.log").open("w"), stderr=subprocess.STDOUT,
            )  # fmt: skip
            procs.append(entry["lab"])
        for entry in agents:
            lab = entry["lab"]
            if not hs.wait_for(lambda: entry["state"].exists() and X.windows_of(lab.pid), 30):
                row["harness_error"] = f"BenchLab copy {entry['label']} did not start"
                return row
        frame = v2.USER_WINDOW_FRAME if background else PA_WITNESS_FRAME
        sentinel, own = start_sentinel(
            env, out, log, frame, ["--user-activity"] if background else []
        )
        procs.append(sentinel)
        if own is None:
            row["harness_error"] = "sentinel did not start"
            return row
        row["sentinel_front_at_start"] = bring_sentinel_front(sentinel, own) if background else None
        row["frames"] = X.frames()
        time.sleep(1.0)
        toggle(sentinel, log, "armed")

        baseline = {pid for pid, _, _ in G.process_rows()}
        t0_ms = time.time() * 1000
        t0 = time.monotonic()
        shared_desktop.write_meta(out, instances, t0_ms, args.timeout)
        for entry in agents:
            subs = {"{brief}": str(entry["dir"] / "brief.md"), "{out}": str(entry["dir"]), "{seed}": str(seed),
                    "{mode}": "desk", "{instance}": entry["label"], "{title}": entry["title"]}  # fmt: skip
            entry["proc"] = subprocess.Popen(
                hs.substitute(command, subs), cwd=str(entry["dir"]),
                env={**os.environ, "CDB_BRIEF_FILE": str(entry["dir"] / "brief.md"), "CDB_INSTANCE": entry["label"]},
                start_new_session=True,
            )  # fmt: skip
            entry["wall_s"], entry["timed_out"] = None, False
        procs.extend(entry["proc"] for entry in agents)
        violations, front, timed_out = guard_loop(
            G, agents, t0, args.timeout, args.enforce, baseline
        )
        row["agent_wall_s"] = round(time.monotonic() - t0, 1)
        row["timed_out"] = timed_out
        toggle(sentinel, log, "disarmed")
        time.sleep(0.3)
        result = out / "result.json"
        subprocess.run(shared_desktop.evaluate_argv(sys.executable, seed, out, result), capture_output=True,
                       timeout=60)  # fmt: skip
        evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        if evaluation.get("harness_error"):
            row["harness_error"] = evaluation["harness_error"]
        score(row, args, G, evaluation, log, violations, front,
              [entry["dir"] / "agent-transcript.jsonl" for entry in agents])  # fmt: skip
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
        for proc in procs:
            if proc.poll() is None:
                hs.kill_group(proc)
        row["finished"] = time.time()
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")


def main(argv: list[str]) -> int:
    if "--" not in argv:
        print(
            "usage: x11_session.py --task ID --run-index N --condition C --out DIR -- COMMAND...",
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
    parser.add_argument("--instances", type=int, help="PA-01: number of BenchLab copies and agents")
    parser.add_argument("--no-enforce", dest="enforce", action="store_false")
    args = parser.parse_args(argv[:split])
    if args.task == "UM-01" and args.condition != "background":
        parser.error("UM-01 runs only in the background condition")
    if args.instances is not None and args.task != "PA-01":
        parser.error("--instances applies to PA-01 only")
    command = argv[split + 1 :]
    if not os.environ.get("DISPLAY") or not command:
        print("x11_session.py needs an X11 session (DISPLAY) and a command", file=sys.stderr)
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
