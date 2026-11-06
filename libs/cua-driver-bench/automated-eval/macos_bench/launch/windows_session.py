#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.
#
# Usage: windows_session.py --task ID --run-index N --condition foreground|background --out DIR
#                           [--timeout S] -- COMMAND [ARG...]
#
# One trial on a Windows desktop, run in the interactive session of the user who owns it:
#   1. derive the seed (fairness rule), render the brief (+ the background paragraph), write DIR/brief.md;
#   2. start the task's apps: BenchLab (windows/benchlab.py) for the BenchLab tasks, or for a pack task
#      (CDB-G02 to CDB-G04) reset its workspace and start the apps of its platform/launch.windows.json
#      (windows/cdb.py); then the sentinel (windows/sentinel.py): in the background condition the
#      "User activity" window partly covers the task's windows, is brought to the front and types;
#   3. arm the sentinel, run COMMAND (the agent; `{brief}`, `{out}`, `{seed}`, `{mode}`, `{workspace}` in its
#      arguments are filled in) with the GUI-only guard watching, stop it at the time limit or on a violation;
#   4. disarm, run the task's checker, summarise the sentinel, write DIR/row.json (a pack task also leaves
#      DIR/workspace-final.zip), stop the apps.
# BenchLab and the sentinel run from copies of pythonw.exe named BenchLab.exe and BenchSentinel.exe, so every
# tool sees them under those names.
from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

BENCH = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(BENCH))
sys.path.insert(0, str(BENCH / "windows"))
sys.path.insert(0, str(BENCH / "probes" / "_common"))
sys.path.insert(0, str(BENCH / "swift"))
sys.path.insert(0, str(BENCH.parents[1] / "fairness"))
import bench_v2 as v2  # noqa: E402
import cdb_fairness  # noqa: E402
import summarize_sentinel  # noqa: E402

LAB_TASKS = ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04")
CDB_TASKS = ("CDB-G02", "CDB-G03", "CDB-G04")
TASKS = CDB_TASKS + LAB_TASKS
LAB_FRAME = (40, 40, 760, 540)
WITNESS_FRAME = (1000, 640, 600, 320)
PLATFORM_WORDS = ("on your Mac", "on this Windows PC")


def task_spec(task: str) -> dict:
    return json.loads((BENCH / "probes" / task / "task.json").read_text(encoding="utf-8"))


def render_brief(task: str, seed: int, condition: str, cdb=None) -> str:
    if cdb is not None:
        return v2.brief_for(cdb.brief().strip(), condition)
    spec = importlib.util.spec_from_file_location(
        f"render_{task}", BENCH / "probes" / task / "render_brief.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    brief = module.render(seed).replace(*PLATFORM_WORDS)
    return v2.brief_for(brief, condition)


def named_python(name: str) -> Path:
    """A copy of pythonw.exe called NAME.exe next to it (it finds its DLLs and stdlib from there)."""
    base = Path(sys.executable).with_name("pythonw.exe")
    target = base.with_name(f"{name}.exe")
    if not target.exists() or target.stat().st_size != base.stat().st_size:
        shutil.copy2(base, target)
    return target


def kill_tree(pid: int) -> None:
    subprocess.run(["taskkill", "/T", "/F", "/PID", str(pid)], capture_output=True)


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


def park_pointer(W) -> dict:
    """Amendment 24: move the real pointer to the fixed park point and read it back (SetCursorPos/GetCursorPos)."""
    x, y = v2.PARK_POINT
    W.user32.SetCursorPos(int(x), int(y))
    return v2.park_record(tuple(float(v) for v in W.cursor()), "SetCursorPos")


def run(args: argparse.Namespace, command: list[str]) -> dict:
    import guard as G
    import winenv as W

    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    spec = task_spec(args.task)
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    cdb = None
    if spec.get("kind") == "cdb":
        import cdb as C

        cdb = C.CdbTask(spec, out, seed=seed)
        mode = spec["pack_task"]
    else:
        mode = spec["app_args"][spec["app_args"].index("--mode") + 1]
    brief = render_brief(args.task, seed, args.condition, cdb)
    (out / "brief.md").write_text(brief, encoding="utf-8")
    background = args.condition == "background"
    row: dict = {
        "task": args.task, "run_index": args.run_index, "seed": seed, "condition": args.condition,
        "mode": mode, "platform": "windows", "label": args.label, "started": time.time(),
    }  # fmt: skip

    state, events, log, control = (
        out / "state.json", out / "events.jsonl", out / "sentinel.jsonl", out / "sentinel.control",
    )  # fmt: skip
    for stale in (state, events, log, control):
        stale.unlink(missing_ok=True)
    control.write_text("", encoding="utf-8")
    procs = []
    try:
        row["pointer_parked"] = park_pointer(W)
        if not row["pointer_parked"]["ok"]:
            row["harness_error"] = f"pointer park failed: {row['pointer_parked']}"
            return row
        if cdb is not None:
            row["workspace"] = str(cdb.workspace)
            try:
                cdb.reset()
                cdb.start_apps()
                cdb.place_windows()
                row["health"] = cdb.health_check()
            except Exception as error:  # noqa: BLE001 - any setup failure is a harness error, never scored
                row["harness_error"] = f"setup: {type(error).__name__}: {str(error)[:400]}"
                return row
            finally:
                row["setup_log"] = list(cdb.log)
        else:
            procs.append(subprocess.Popen(
                [str(named_python("BenchLab")), str(BENCH / "windows" / "benchlab.py"), "--mode", mode,
                 "--seed", str(seed), "--state", str(state), "--events", str(events),
                 "--frame", ",".join(map(str, LAB_FRAME)), *(["--no-activate"] if background else [])],
            ))  # fmt: skip
            if not wait_for(state.exists, 30):
                row["harness_error"] = "BenchLab did not start"
                return row
        frame = v2.USER_WINDOW_FRAME if background else WITNESS_FRAME
        sentinel = subprocess.Popen(
            [str(named_python("BenchSentinel")), str(BENCH / "windows" / "sentinel.py"),
             "--log", str(log), "--control", str(control), "--frame", ",".join(map(str, frame)),
             *(["--user-activity"] if background else [])],
        )  # fmt: skip
        procs.append(sentinel)
        if not wait_for(lambda: any(ln.get("ev") == "ready" for ln in sentinel_lines(log)), 30):
            row["harness_error"] = "sentinel did not start"
            return row
        own = next(ln["hwnd"] for ln in sentinel_lines(log) if ln.get("ev") == "ready")
        with control.open("a", encoding="utf-8") as c:
            c.write("activate\n")
        front_ok = wait_for(lambda: W.user32.GetForegroundWindow() == own, 10)
        if not front_ok:
            with control.open("a", encoding="utf-8") as c:
                c.write("activate\n")
            front_ok = wait_for(lambda: W.user32.GetForegroundWindow() == own, 5)
        row["sentinel_front_at_start"] = front_ok
        time.sleep(1.0)
        with control.open("a", encoding="utf-8") as c:
            c.write("arm\n")
        wait_for(lambda: any(ln.get("ev") == "armed" for ln in sentinel_lines(log)), 5)

        subs = {
            "{brief}": str(out / "brief.md"),
            "{out}": str(out),
            "{seed}": str(seed),
            "{mode}": mode,
            "{workspace}": str(cdb.workspace) if cdb is not None else "",
        }
        argv = []
        for a in command:
            for key, value in subs.items():
                a = a.replace(key, value)
            argv.append(a)
        rows0 = W.processes()
        baseline = {pid for pid, _, _ in rows0}
        t0 = time.monotonic()
        agent = subprocess.Popen(
            argv, cwd=str(out), env={**os.environ, "CDB_BRIEF_FILE": str(out / "brief.md")}
        )
        procs.append(agent)
        own_tree: set[int] = set()
        helpers: dict[int, dict] = {}
        session = W.session_of(os.getpid())
        front = G.FrontWatch()
        violations: list[dict] = []
        timed_out = False
        while agent.poll() is None:
            rows = W.processes()
            own_tree |= G.descendants(rows, os.getpid())
            # Only the runner's own (interactive) session: remote administration (SSH in session 0) is not the GUI.
            here = [r for r in rows if W.session_of(r[0]) == session]
            for pid, label in G.new_side_doors(here, baseline, os.getpid(), own_tree):
                by = G.started_by_tool(rows, pid, args.tool_exe)
                if (
                    by
                ):  # the setup's own binary started it: part of the tool, logged, not a side door
                    helpers.setdefault(
                        pid, {"t_s": round(time.monotonic() - t0, 1), "detail": label, "parent": by}
                    )
                    continue
                violations.append(
                    {
                        "t_s": round(time.monotonic() - t0, 1),
                        "kind": "process",
                        "detail": label,
                        "pid": pid,
                    }
                )
                kill_tree(pid)
            hwnd = W.user32.GetForegroundWindow()
            hit = front.sample(W.process_path(W.window_pid(hwnd)) if hwnd else "")
            if hit:
                violations.append(
                    {"t_s": round(time.monotonic() - t0, 1), "kind": "front", "detail": hit}
                )
            if violations and args.enforce:
                kill_tree(agent.pid)
                break
            if time.monotonic() - t0 > args.timeout:
                timed_out = True
                kill_tree(agent.pid)
                break
            time.sleep(0.5)
        agent.wait(timeout=30)
        row["agent_exit"] = agent.returncode
        row["agent_wall_s"] = round(time.monotonic() - t0, 1)
        row["timed_out"] = timed_out

        with control.open("a", encoding="utf-8") as c:
            c.write("disarm\n")
        wait_for(lambda: any(ln.get("ev") == "disarmed" for ln in sentinel_lines(log)), 5)
        time.sleep(0.3)
        result = out / "result.json"
        if cdb is not None:
            evaluation = cdb.evaluate(124 if timed_out else int(agent.returncode or 0))
            result.write_text(json.dumps(evaluation, indent=1), encoding="utf-8")
            cdb.snapshot_workspace(out / "workspace-final.zip")
            if evaluation.get("harness_error"):
                row["harness_error"] = f"evaluate: {evaluation['harness_error']}"
        else:
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
                      "mentions": soft, "frontmost_seen": sorted(front.seen),
                      "tool_helpers": list(helpers.values())[:20]},
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
                kill_tree(proc.pid)
        if cdb is not None:
            cdb.stop_apps()
            cdb.clean_workspace()
        row["finished"] = time.time()
        (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")


def main(argv: list[str]) -> int:
    if "--" not in argv:
        print(
            "usage: windows_session.py --task ID --run-index N --condition C --out DIR -- COMMAND...",
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
    parser.add_argument(
        "--tool-exe", action="append", default=[],
        help="executable of the setup's own tool (repeatable): a shell it starts itself is not a side door",
    )  # fmt: skip
    args = parser.parse_args(argv[:split])
    command = argv[split + 1 :]
    if sys.platform != "win32":
        print("windows_session.py requires Windows", file=sys.stderr)
        return 2
    if not command:
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
