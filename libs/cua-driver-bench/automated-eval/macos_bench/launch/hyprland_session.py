#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Usage: hyprland_session.py --task ID --run-index N --condition foreground|background --out DIR
#                            [--timeout S] -- COMMAND [ARG...]
#
# One BenchLab trial on a Hyprland (Wayland) desktop such as Omarchy, run as the desktop user with the session's
# environment (WAYLAND_DISPLAY, HYPRLAND_INSTANCE_SIGNATURE, XDG_RUNTIME_DIR, DBUS_SESSION_BUS_ADDRESS):
#   1. derive the seed (fairness rule), render the brief (+ the background paragraph), write DIR/brief.md;
#   2. start BenchLab (linux/benchlab.py, GTK3) and the sentinel (hyprland/sentinel.py) and place both through
#      Hyprland's IPC: in the background condition the "User activity" window partly covers BenchLab, is brought
#      to the front and types;
#   3. arm the sentinel, run COMMAND (the agent; `{brief}`, `{out}`, `{seed}`, `{mode}` in its arguments are filled
#      in) with the GUI-only guard watching, stop it at the time limit or on a violation;
#   4. disarm, run the task's checker, summarise the sentinel, write DIR/row.json.
# Shared-desktop tasks (SHARED-DESKTOP.md):
#   PA-01 [--instances N]: N BenchLab copies (`--mode desk`) placed in the 5x2 grid, one agent process each, all
#     started at the same moment; `{brief}` and `{out}` are that agent's own (DIR/desk-L/), plus `{instance}` (the
#     label) and `{title}` (the window title); CDB_BRIEF_FILE and CDB_INSTANCE are in its environment. Any
#     agent's GUI-only violation stops all of them. Needs a screen of at least 1920x1000.
#   UM-01 (background only): one desk, and the sentinel's user moves the real pointer and clicks from a seeded
#     script (the pointer moves through the compositor's cursor dispatcher, clicks need wlrctl or ydotool).
# The desktop needs BenchLab and BenchSentinel to float, and BenchLab's windows not to take focus when they open
# (hyprland/bench.lua).
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

BENCH = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "probes" / "_common"))
sys.path.insert(0, str(BENCH / "swift"))
sys.path.insert(0, str(BENCH.parents[1] / "fairness"))
import bench_v2 as v2  # noqa: E402
import cdb_fairness  # noqa: E402
import shared_desktop  # noqa: E402
import summarize_sentinel  # noqa: E402

TASKS = ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04", "PA-01", "UM-01")
HARD_TASKS = (
    "PA-02",
    "UM-02",
)  # X11, Windows and macOS only: a Hyprland desktop does not run the hard variants
LAB_FRAME = (40, 40, 760, 540)
WITNESS_FRAME = (1000, 640, 600, 320)
PA_WITNESS_FRAME = shared_desktop.PA_WITNESS_FRAME  # below the PA-01 grid (which ends at y = 900)
PA_MIN_SCREEN = (1920, 1000)
PLATFORM_WORDS = ("on your Mac", "on this Linux computer")
RULES = BENCH / "hyprland" / "bench.lua"


def apply_rules(H) -> None:
    """The window rules of hyprland/bench.lua (named, so applying them again replaces them)."""
    text = RULES.read_text(encoding="utf-8")
    for chunk in text.split("hl.window_rule(")[1:]:
        depth, end = 1, 0
        for i, ch in enumerate(chunk):
            depth += ch == "("
            depth -= ch == ")"
            if depth == 0:
                end = i
                break
        H.request("eval hl.window_rule(" + chunk[: end + 1])


def hyprland_module(name: str):
    """hyprland/NAME.py under its own module name (windows/ has a guard.py too)."""
    spec = importlib.util.spec_from_file_location(
        f"hyprland_{name}", BENCH / "hyprland" / f"{name}.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def task_spec(task: str) -> dict:
    return json.loads((BENCH / "probes" / task / "task.json").read_text(encoding="utf-8"))


def render_brief(task: str, seed: int, condition: str, mouse_user: bool = False) -> str:
    spec = importlib.util.spec_from_file_location(
        f"render_{task}", BENCH / "probes" / task / "render_brief.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    brief = module.render(seed).replace(*PLATFORM_WORDS)
    return v2.brief_for(brief, condition, mouse_user=mouse_user)


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


def kill_pid(pid: int) -> None:
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        pass


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


def screen_too_small(H, width: int, height: int) -> bool:
    monitors = H.query("monitors") or []
    return (
        not monitors
        or max(m["width"] for m in monitors) < width
        or max(m["height"] for m in monitors) < height
    )


def substitute(command: list[str], subs: dict[str, str]) -> list[str]:
    argv = []
    for a in command:
        for key, value in subs.items():
            a = a.replace(key, value)
        argv.append(a)
    return argv


def append_control(control: Path, line: str) -> None:
    with control.open("a", encoding="utf-8") as c:
        c.write(line + "\n")


def start_sentinel(H, env: dict, out: Path, log: Path, control: Path, frame, extra: list[str]):
    """Start hyprland/sentinel.py placed at `frame`; returns (process, window address or None)."""
    proc = subprocess.Popen(
        [sys.executable, str(BENCH / "hyprland" / "sentinel.py"), "--log", str(log),
         "--control", str(control), "--frame", ",".join(map(str, frame)), *extra],
        env=env, start_new_session=True, stdout=(out / "sentinel.log").open("w"), stderr=subprocess.STDOUT,
    )  # fmt: skip
    if not wait_for(lambda: any(ln.get("ev") == "ready" for ln in sentinel_lines(log)), 30):
        return proc, None
    return proc, next(ln["address"] for ln in sentinel_lines(log) if ln.get("ev") == "ready")


def bring_sentinel_front(H, control: Path, own: str) -> bool:
    def in_front() -> bool:
        return H.active_window().get("address") == own and H.clients()[-1].get("address") == own

    append_control(control, "activate")
    front_ok = wait_for(in_front, 10)
    if not front_ok:
        append_control(control, "activate")
        front_ok = wait_for(in_front, 5)
    return front_ok


def read_stream(transcript: Path) -> list[dict]:
    stream = []
    if transcript.exists():
        for raw in transcript.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                stream.append(json.loads(raw))
            except json.JSONDecodeError:
                pass
    return stream


def run(args: argparse.Namespace, command: list[str]) -> dict:
    G = hyprland_module("guard")
    H = hyprland_module("hypr")
    spec = task_spec(args.task)
    if spec.get("parallel"):
        return run_parallel(args, command, G, H, spec)

    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    mode = spec["app_args"][spec["app_args"].index("--mode") + 1]
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    um = bool(spec.get("mouse_user"))
    brief = render_brief(args.task, seed, args.condition, mouse_user=um)
    (out / "brief.md").write_text(brief, encoding="utf-8")
    background = args.condition == "background"
    row: dict = {
        "task": args.task, "run_index": args.run_index, "seed": seed, "condition": args.condition,
        "mode": mode, "platform": "linux-hyprland", "label": args.label, "started": time.time(),
    }  # fmt: skip

    state, events, log, control = (
        out / "state.json", out / "events.jsonl", out / "sentinel.jsonl", out / "sentinel.control",
    )  # fmt: skip
    for stale in (state, events, log, control):
        stale.unlink(missing_ok=True)
    control.write_text("", encoding="utf-8")
    apply_rules(H)
    if screen_too_small(H, 1320, 860):
        row["harness_error"] = "screen smaller than 1320x860 (load hyprland/bench.lua)"
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")
        return row
    env = {**os.environ, "GDK_BACKEND": "wayland", "NO_AT_BRIDGE": "0"}
    lab_frame = LAB_FRAME
    lab_args = ["--mode", mode, "--seed", str(seed), "--state", str(state), "--events", str(events)]
    sentinel_extra = ["--user-activity"] if background else []
    if um:
        if not background:
            row["harness_error"] = "UM-01 runs only in the background condition"
            return row
        if H.click_command() is None:
            row["harness_error"] = "no click tool (wlrctl or ydotool) for the mouse user"
            (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")
            return row
        script = shared_desktop.write_mouse_script(seed, out / "mouse-user.json")
        lab_args += shared_desktop.um_app_args(seed)
        lab_frame = tuple(shared_desktop.um_desk(seed)["frame"])
        sentinel_extra = v2.sentinel_args("background", mouse_script=out / "mouse-user.json")[:2]
        row["mouse_user_script"] = {
            "dots": len(script["dots"]),
            "sequence": len(script["sequence"]),
        }
    lab = subprocess.Popen(
        [sys.executable, str(BENCH / "linux" / "benchlab.py"), *lab_args],
        env=env, start_new_session=True, stdout=(out / "benchlab.log").open("w"), stderr=subprocess.STDOUT,
    )  # fmt: skip
    procs = [lab]
    try:
        if not wait_for(lambda: state.exists() and H.by_pid(lab.pid), 30):
            row["harness_error"] = "BenchLab did not start"
            return row
        lab_address = H.by_pid(lab.pid)[0]["address"]
        H.place(lab_address, lab_frame)
        if not background:
            H.focus(lab_address)
        frame = v2.USER_WINDOW_FRAME if background else WITNESS_FRAME
        sentinel, own = start_sentinel(H, env, out, log, control, frame, sentinel_extra)
        procs.append(sentinel)
        if own is None:
            row["harness_error"] = "sentinel did not start"
            return row
        row["sentinel_front_at_start"] = bring_sentinel_front(H, control, own)
        row["frames"] = {
            c.get("class"): [*c.get("at", []), *c.get("size", [])] for c in H.clients()
        }
        time.sleep(1.0)
        append_control(control, "arm")
        wait_for(lambda: any(ln.get("ev") == "armed" for ln in sentinel_lines(log)), 5)

        subs = {
            "{brief}": str(out / "brief.md"),
            "{out}": str(out),
            "{seed}": str(seed),
            "{mode}": mode,
        }
        argv = substitute(command, subs)
        baseline = {pid for pid, _, _ in G.process_rows()}
        t0 = time.monotonic()
        agent = subprocess.Popen(
            argv, cwd=str(out), env={**os.environ, "CDB_BRIEF_FILE": str(out / "brief.md")},
            start_new_session=True,
        )  # fmt: skip
        procs.append(agent)
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
                kill_pid(pid)
            try:
                hit = front.sample(H.active_window().get("class", ""))
            except (OSError, ValueError, RuntimeError):
                hit = None
            if hit:
                violations.append(
                    {"t_s": round(time.monotonic() - t0, 1), "kind": "front", "detail": hit}
                )
            if violations and args.enforce:
                kill_group(agent)
                break
            if time.monotonic() - t0 > args.timeout:
                timed_out = True
                kill_group(agent)
                break
            time.sleep(0.5)
        try:
            agent.wait(timeout=30)
        except subprocess.TimeoutExpired:
            kill_group(agent)
        row["agent_exit"] = agent.returncode
        row["agent_wall_s"] = round(time.monotonic() - t0, 1)
        row["timed_out"] = timed_out

        append_control(control, "disarm")
        wait_for(lambda: any(ln.get("ev") == "disarmed" for ln in sentinel_lines(log)), 5)
        time.sleep(0.3)
        result = out / "result.json"
        checker = [sys.executable, str(BENCH / "probes" / args.task / "evaluate.py"), "--seed", str(seed),
                   "--state", str(state), "--events", str(events), "--result", str(result)]  # fmt: skip
        if um:
            checker += ["--sentinel", str(log)]
        subprocess.run(checker, capture_output=True, timeout=60)
        evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        hard, soft = G.scan_events(read_stream(out / "agent-transcript.jsonl"))
        violations += [{"t_s": None, "kind": "tool_input", "detail": h} for h in hard]
        disturbance = summarize_sentinel.summarize(str(log))
        armed = next((ln for ln in sentinel_lines(log) if ln.get("ev") == "armed"), {})
        frontmost = (armed.get("front") or {}).get("bid") == "ai.cua.benchsentinel"
        passed = bool(evaluation.get("passed")) and not violations
        if um:
            problem = v2.mouse_user_problem(disturbance) or evaluation.get("harness_error")
            if problem:
                row["harness_error"] = problem
        bg = v2.background_score(disturbance, frontmost)
        row.update(
            evaluation={k: evaluation.get(k) for k in ("passed", "score", "checks", "diagnostics")},
            checker_passed=bool(evaluation.get("passed")),
            gui_only={"violation": bool(violations), "violations": violations[:20],
                      "mentions": soft, "frontmost_seen": sorted(front.seen)},
            passed=passed,
            disturbance=disturbance,
            background=bg,
            a15=v2.background_condition_score(bg, disturbance, passed, args.condition),
            a22=v2.multiplayer_score(disturbance, bg["measured"], passed, args.condition),
        )  # fmt: skip
        return row
    finally:
        append_control(control, "quit")
        time.sleep(0.5)
        for proc in procs:
            if proc.poll() is None:
                kill_group(proc)
        row["finished"] = time.time()
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")


PARALLEL_FIELDS = ("task", "seed", "condition", "passed", "harness_error")


def run_parallel(args: argparse.Namespace, command: list[str], G, H, spec: dict) -> dict:
    """PA-01: N desks, N agents started at the same moment, one GUI-only guard over all of them."""
    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    instances = shared_desktop.instances_for(spec, args.instances)
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    background = args.condition == "background"
    row: dict = {
        "task": args.task, "run_index": args.run_index, "seed": seed, "condition": args.condition,
        "mode": "desk", "platform": "linux-hyprland", "label": args.label, "started": time.time(),
        "instances": instances,
    }  # fmt: skip
    log, control = out / "sentinel.jsonl", out / "sentinel.control"
    log.unlink(missing_ok=True)
    control.write_text("", encoding="utf-8")
    agents = shared_desktop.pa_agents(seed, instances, out, PLATFORM_WORDS)
    for entry in agents:
        entry["dir"].mkdir(parents=True, exist_ok=True)
        entry["state"].unlink(missing_ok=True)
        entry["events"].unlink(missing_ok=True)
        (entry["dir"] / "brief.md").write_text(
            v2.brief_for(entry["brief"], args.condition), encoding="utf-8"
        )
    apply_rules(H)
    if screen_too_small(H, *PA_MIN_SCREEN):
        row["harness_error"] = (
            f"screen smaller than {PA_MIN_SCREEN[0]}x{PA_MIN_SCREEN[1]} (load hyprland/bench.lua)"
        )
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")
        return row
    env = {**os.environ, "GDK_BACKEND": "wayland", "NO_AT_BRIDGE": "0"}
    procs: list[subprocess.Popen] = []
    try:
        for entry in agents:
            lab = subprocess.Popen(
                [sys.executable, str(BENCH / "linux" / "benchlab.py"), "--mode", "desk",
                 *entry["app_args"], *(["--no-activate"] if background else [])],
                env=env, start_new_session=True,
                stdout=(entry["dir"] / "benchlab.log").open("w"), stderr=subprocess.STDOUT,
            )  # fmt: skip
            procs.append(lab)
            entry["lab"] = lab
        for entry in agents:
            lab = entry["lab"]
            if not wait_for(lambda: entry["state"].exists() and H.by_pid(lab.pid), 30):
                row["harness_error"] = f"BenchLab copy {entry['label']} did not start"
                return row
            H.place(H.by_pid(lab.pid)[0]["address"], entry["frame"])
        frame = v2.USER_WINDOW_FRAME if background else PA_WITNESS_FRAME
        sentinel, own = start_sentinel(
            H, env, out, log, control, frame, ["--user-activity"] if background else []
        )
        procs.append(sentinel)
        if own is None:
            row["harness_error"] = "sentinel did not start"
            return row
        row["sentinel_front_at_start"] = (
            bring_sentinel_front(H, control, own) if background else None
        )
        row["frames"] = {
            c.get("title"): [*c.get("at", []), *c.get("size", [])] for c in H.clients()
        }
        time.sleep(1.0)
        append_control(control, "arm")
        wait_for(lambda: any(ln.get("ev") == "armed" for ln in sentinel_lines(log)), 5)

        baseline = {pid for pid, _, _ in G.process_rows()}
        t0_ms = time.time() * 1000
        t0 = time.monotonic()
        shared_desktop.write_meta(out, instances, t0_ms, args.timeout)
        for entry in agents:
            subs = {
                "{brief}": str(entry["dir"] / "brief.md"),
                "{out}": str(entry["dir"]),
                "{seed}": str(seed),
                "{mode}": "desk",
                "{instance}": entry["label"],
                "{title}": entry["title"],
            }
            entry["proc"] = subprocess.Popen(
                substitute(command, subs), cwd=str(entry["dir"]),
                env={**os.environ, "CDB_BRIEF_FILE": str(entry["dir"] / "brief.md"),
                     "CDB_INSTANCE": entry["label"]},
                start_new_session=True,
            )  # fmt: skip
            entry["wall_s"], entry["timed_out"] = None, False
        procs.extend(entry["proc"] for entry in agents)

        def running() -> list[dict]:
            live = []
            for entry in agents:
                if entry["proc"].poll() is None:
                    live.append(entry)
                elif entry["wall_s"] is None:
                    entry["wall_s"] = round(time.monotonic() - t0, 1)
            return live

        def kill_all(timed_out: bool = False) -> None:
            for entry in running():
                entry["timed_out"] = timed_out
                kill_group(entry["proc"])

        own_tree: set[int] = set()
        front = G.FrontWatch()
        violations: list[dict] = []
        timed_out = False
        while running():
            rows = G.process_rows()
            own_tree |= G.descendants(rows, os.getpid())
            for pid, label in G.new_side_doors(rows, baseline, os.getpid(), own_tree):
                violations.append(
                    {"t_s": round(time.monotonic() - t0, 1), "kind": "process", "detail": label,
                     "pid": pid}
                )  # fmt: skip
                kill_pid(pid)
            try:
                hit = front.sample(H.active_window().get("class", ""))
            except (OSError, ValueError, RuntimeError):
                hit = None
            if hit:
                violations.append(
                    {"t_s": round(time.monotonic() - t0, 1), "kind": "front", "detail": hit}
                )
            if violations and args.enforce:
                kill_all()
                break
            if time.monotonic() - t0 > args.timeout:
                timed_out = True
                kill_all(timed_out=True)
                break
            time.sleep(0.5)
        for entry in agents:
            try:
                entry["proc"].wait(timeout=30)
            except subprocess.TimeoutExpired:
                kill_group(entry["proc"])
            if entry["wall_s"] is None:
                entry["wall_s"] = round(time.monotonic() - t0, 1)
        row["agent_wall_s"] = round(time.monotonic() - t0, 1)
        row["timed_out"] = timed_out

        append_control(control, "disarm")
        wait_for(lambda: any(ln.get("ev") == "disarmed" for ln in sentinel_lines(log)), 5)
        time.sleep(0.3)
        result = out / "result.json"
        subprocess.run(
            shared_desktop.evaluate_argv(sys.executable, seed, out, result),
            capture_output=True, timeout=60,
        )  # fmt: skip
        evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        if evaluation.get("harness_error"):
            row["harness_error"] = evaluation["harness_error"]
        soft: list = []
        for entry in agents:
            hard, mentions = G.scan_events(read_stream(entry["dir"] / "agent-transcript.jsonl"))
            soft += mentions
            violations += [
                {"t_s": None, "kind": "tool_input", "detail": h, "agent": entry["label"]}
                for h in hard
            ]
        disturbance = summarize_sentinel.summarize(str(log))
        armed = next((ln for ln in sentinel_lines(log) if ln.get("ev") == "armed"), {})
        frontmost = (armed.get("front") or {}).get("bid") == "ai.cua.benchsentinel"
        passed = bool(evaluation.get("passed")) and not violations
        bg = v2.background_score(disturbance, frontmost)
        row.update(
            evaluation={k: evaluation.get(k) for k in ("passed", "score", "checks", "diagnostics")},
            checker_passed=bool(evaluation.get("passed")),
            parallel=shared_desktop.parallel_of(evaluation),
            agents=shared_desktop.aggregate_agents(
                [
                    {"label": e["label"], "wall_s": e["wall_s"], "timed_out": e["timed_out"],
                     "returncode": e["proc"].returncode}
                    for e in agents
                ]
            ),
            gui_only={"violation": bool(violations), "violations": violations[:20],
                      "mentions": soft, "frontmost_seen": sorted(front.seen)},
            passed=passed,
            disturbance=disturbance,
            background=bg,
            a15=v2.background_condition_score(bg, disturbance, passed, args.condition),
            a22=v2.multiplayer_score(disturbance, bg["measured"], passed, args.condition),
        )  # fmt: skip
        return row
    finally:
        append_control(control, "quit")
        time.sleep(0.5)
        for proc in procs:
            if proc.poll() is None:
                kill_group(proc)
        row["finished"] = time.time()
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")


def main(argv: list[str]) -> int:
    if "--" not in argv:
        print(
            "usage: hyprland_session.py --task ID --run-index N --condition C --out DIR -- COMMAND...",
            file=sys.stderr,
        )
        return 2
    split = argv.index("--")
    parser = argparse.ArgumentParser()
    parser.add_argument("--task", choices=(*TASKS, *HARD_TASKS), required=True)
    parser.add_argument("--run-index", type=int, required=True)
    parser.add_argument("--condition", choices=v2.CONDITIONS, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--label", default="")
    parser.add_argument("--timeout", type=float, default=360.0)
    parser.add_argument("--instances", type=int, help="PA-01: number of BenchLab copies and agents")
    parser.add_argument("--no-enforce", dest="enforce", action="store_false")
    args = parser.parse_args(argv[:split])
    if args.task in HARD_TASKS:  # not a fail: the task is simply not run here
        message = (
            f"{args.task} is not supported on Hyprland (use the X11 launcher, Windows or macOS)"
        )
        print(message, file=sys.stderr)
        print(json.dumps({"task": args.task, "seed": None, "condition": args.condition, "passed": None,
                          "harness_error": message}))  # fmt: skip
        return 3
    if args.task == "UM-01" and args.condition != "background":
        parser.error("UM-01 runs only in the background condition")
    if args.instances is not None and args.task != "PA-01":
        parser.error("--instances applies to PA-01 only")
    command = argv[split + 1 :]
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE") or not command:
        print("hyprland_session.py needs a Hyprland session and a command", file=sys.stderr)
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
