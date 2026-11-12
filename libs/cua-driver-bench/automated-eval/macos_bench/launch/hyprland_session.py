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
import summarize_sentinel  # noqa: E402

TASKS = ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04")
LAB_FRAME = (40, 40, 760, 540)
WITNESS_FRAME = (1000, 640, 600, 320)
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


def render_brief(task: str, seed: int, condition: str) -> str:
    spec = importlib.util.spec_from_file_location(
        f"render_{task}", BENCH / "probes" / task / "render_brief.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    brief = module.render(seed).replace(*PLATFORM_WORDS)
    return v2.brief_for(brief, condition)


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


def park_pointer(H) -> dict:
    """Amendment 24: move the real pointer to the fixed park point and read it back. Hyprland 0.55+ takes the
    Lua dispatcher; the legacy `movecursor` is the fallback. The record says ok=False when it did not land."""
    x, y = v2.PARK_POINT
    record = v2.park_record(None, "hyprland dispatch")
    for expression in (f"hl.dsp.cursor.move({{ x = {x}, y = {y} }})", f"movecursor {x} {y}"):
        try:
            H.dispatch(expression)
            record = v2.park_record(tuple(H.cursor()), "hyprland dispatch")
        except OSError:
            continue
        if record["ok"]:
            break
    return record


def run(args: argparse.Namespace, command: list[str]) -> dict:
    G = hyprland_module("guard")
    H = hyprland_module("hypr")

    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    spec = task_spec(args.task)
    mode = spec["app_args"][spec["app_args"].index("--mode") + 1]
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    brief = render_brief(args.task, seed, args.condition)
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
    monitors = H.query("monitors") or []
    if (
        not monitors
        or max(m["width"] for m in monitors) < 1320
        or max(m["height"] for m in monitors) < 860
    ):
        row["harness_error"] = "screen smaller than 1320x860 (load hyprland/bench.lua)"
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")
        return row
    row["pointer_parked"] = park_pointer(H)
    if not row["pointer_parked"]["ok"]:
        row["harness_error"] = f"pointer park failed: {row['pointer_parked']}"
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")
        return row
    env = {**os.environ, "GDK_BACKEND": "wayland", "NO_AT_BRIDGE": "0"}
    lab = subprocess.Popen(
        [sys.executable, str(BENCH / "linux" / "benchlab.py"), "--mode", mode, "--seed", str(seed),
         "--state", str(state), "--events", str(events)],
        env=env, start_new_session=True, stdout=(out / "benchlab.log").open("w"), stderr=subprocess.STDOUT,
    )  # fmt: skip
    procs = [lab]
    try:
        if not wait_for(lambda: state.exists() and H.by_pid(lab.pid), 30):
            row["harness_error"] = "BenchLab did not start"
            return row
        lab_address = H.by_pid(lab.pid)[0]["address"]
        H.place(lab_address, LAB_FRAME)
        if not background:
            H.focus(lab_address)
        frame = v2.USER_WINDOW_FRAME if background else WITNESS_FRAME
        sentinel = subprocess.Popen(
            [sys.executable, str(BENCH / "hyprland" / "sentinel.py"), "--log", str(log),
             "--control", str(control), "--frame", ",".join(map(str, frame)),
             *(["--user-activity"] if background else [])],
            env=env, start_new_session=True, stdout=(out / "sentinel.log").open("w"), stderr=subprocess.STDOUT,
        )  # fmt: skip
        procs.append(sentinel)
        if not wait_for(lambda: any(ln.get("ev") == "ready" for ln in sentinel_lines(log)), 30):
            row["harness_error"] = "sentinel did not start"
            return row
        own = next(ln["address"] for ln in sentinel_lines(log) if ln.get("ev") == "ready")

        def in_front() -> bool:
            return H.active_window().get("address") == own and H.clients()[-1].get("address") == own

        with control.open("a", encoding="utf-8") as c:
            c.write("activate\n")
        front_ok = wait_for(in_front, 10)
        if not front_ok:
            with control.open("a", encoding="utf-8") as c:
                c.write("activate\n")
            front_ok = wait_for(in_front, 5)
        row["sentinel_front_at_start"] = front_ok
        row["frames"] = {
            c.get("class"): [*c.get("at", []), *c.get("size", [])] for c in H.clients()
        }
        time.sleep(1.0)
        with control.open("a", encoding="utf-8") as c:
            c.write("arm\n")
        wait_for(lambda: any(ln.get("ev") == "armed" for ln in sentinel_lines(log)), 5)

        subs = {
            "{brief}": str(out / "brief.md"),
            "{out}": str(out),
            "{seed}": str(seed),
            "{mode}": mode,
        }
        argv = []
        for a in command:
            for key, value in subs.items():
                a = a.replace(key, value)
            argv.append(a)
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

        with control.open("a", encoding="utf-8") as c:
            c.write("disarm\n")
        wait_for(lambda: any(ln.get("ev") == "disarmed" for ln in sentinel_lines(log)), 5)
        time.sleep(0.3)
        result = out / "result.json"
        subprocess.run(
            [sys.executable, str(BENCH / "probes" / args.task / "evaluate.py"), "--seed", str(seed),
             "--state", str(state), "--events", str(events), "--result", str(result)],
            capture_output=True, timeout=60,
        )  # fmt: skip
        evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        transcript = out / "agent-transcript.jsonl"
        stream = []
        if transcript.exists():
            for raw in transcript.read_text(encoding="utf-8", errors="replace").splitlines():
                try:
                    stream.append(json.loads(raw))
                except json.JSONDecodeError:
                    pass
        hard, soft = G.scan_events(stream)
        violations += [{"t_s": None, "kind": "tool_input", "detail": h} for h in hard]
        disturbance = summarize_sentinel.summarize(str(log))
        armed = next((ln for ln in sentinel_lines(log) if ln.get("ev") == "armed"), {})
        frontmost = (armed.get("front") or {}).get("bid") == "ai.cua.benchsentinel"
        passed = bool(evaluation.get("passed")) and not violations
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
        with control.open("a", encoding="utf-8") as c:
            c.write("quit\n")
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
    parser.add_argument("--task", choices=TASKS, required=True)
    parser.add_argument("--run-index", type=int, required=True)
    parser.add_argument("--condition", choices=v2.CONDITIONS, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--label", default="")
    parser.add_argument("--timeout", type=float, default=360.0)
    parser.add_argument("--no-enforce", dest="enforce", action="store_false")
    args = parser.parse_args(argv[:split])
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
