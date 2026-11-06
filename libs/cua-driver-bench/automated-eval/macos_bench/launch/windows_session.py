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
import shared_desktop  # noqa: E402
import summarize_sentinel  # noqa: E402

LAB_TASKS = ("MB-09", "MB-10", "MB-11", "IR-01", "IR-02", "IR-03", "IR-04")
CDB_TASKS = ("CDB-G02", "CDB-G03", "CDB-G04")
SHARED_DESKTOP_TASKS = ("PA-01", "UM-01")
TASKS = CDB_TASKS + LAB_TASKS + SHARED_DESKTOP_TASKS
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


def fill(command: list[str], subs: dict[str, str]) -> list[str]:
    """The agent command with {brief}, {out}, {seed}, {mode} (and, for PA-01, {instance}, {title}) filled in."""
    argv = []
    for a in command:
        for key, value in subs.items():
            a = a.replace(key, value)
        argv.append(a)
    return argv


def benchlab_argv(
    seed: int, mode: str, state: Path, events: Path, frame: tuple, *, background: bool
) -> list[str]:
    """BenchLab's command line for one of the original modes."""
    return [str(named_python("BenchLab")), str(BENCH / "windows" / "benchlab.py"), "--mode", mode,
            "--seed", str(seed), "--state", str(state), "--events", str(events),
            "--frame", ",".join(map(str, frame)), *(["--no-activate"] if background else [])]  # fmt: skip


def desk_argv(app_args: list[str], *, background: bool) -> list[str]:
    """BenchLab's command line for a desk (PA-01 copy, UM-01 desk): app_args are `--seed .. --state ..
    --events .. --instance L --frame x,y,w,h` (shared_desktop.pa_agents, um_app_args)."""
    return [str(named_python("BenchLab")), str(BENCH / "windows" / "benchlab.py"), "--mode", "desk",
            *app_args, *(["--no-activate"] if background else [])]  # fmt: skip


def um_desk_args(seed: int, state: Path, events: Path) -> list[str]:
    return ["--seed", str(seed), "--state", str(state), "--events", str(events),
            *shared_desktop.um_app_args(seed)]  # fmt: skip


def sentinel_argv(
    log: Path, control: Path, frame: tuple, *, user_activity: bool, mouse_script: Path | None = None
) -> list[str]:
    argv = [str(named_python("BenchSentinel")), str(BENCH / "windows" / "sentinel.py"),
            "--log", str(log), "--control", str(control), "--frame", ",".join(map(str, frame))]  # fmt: skip
    if mouse_script is not None:
        argv += ["--mouse-user", str(mouse_script)]
    elif user_activity:
        argv.append("--user-activity")
    return argv


def pa_plan(seed: int, instances: int, out: Path) -> list[dict]:
    """The copies of a PA-01 trial (cell order): label, title, frame, brief, directory, state, events, app_args."""
    return shared_desktop.pa_agents(seed, instances, out, PLATFORM_WORDS)


def agent_env(brief: Path, **extra: str) -> dict[str, str]:
    return {**os.environ, "CDB_BRIEF_FILE": str(brief), **extra}


def scan_transcript(path: Path) -> tuple[list[str], list[str]]:
    import guard as G

    stream = []
    if path.exists():
        for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
            try:
                stream.append(json.loads(raw))
            except json.JSONDecodeError:
                pass
    return G.scan_events(stream)


def watch_agents(
    agents: list[subprocess.Popen], args: argparse.Namespace, enforce: bool
) -> tuple[list[dict], bool, list[float], object]:
    """The GUI-only guard over every agent of a trial: any violation kills all of them (when enforcing), the
    time limit kills all. Returns (violations, timed_out, per-agent wall seconds, the front-window watch)."""
    import guard as G
    import winenv as W

    baseline = {pid for pid, _, _ in W.processes()}
    t0 = time.monotonic()
    own_tree: set[int] = set()
    session = W.session_of(os.getpid())
    front = G.FrontWatch()
    violations: list[dict] = []
    timed_out = False
    ended: dict[int, float] = {}
    while any(a.poll() is None for a in agents):
        for i, a in enumerate(agents):
            if a.poll() is not None:
                ended.setdefault(i, time.monotonic() - t0)
        rows = W.processes()
        for a in agents:
            own_tree |= G.descendants(rows, a.pid)
        own_tree |= G.descendants(rows, os.getpid())
        here = [r for r in rows if W.session_of(r[0]) == session]
        for pid, label in G.new_side_doors(here, baseline, os.getpid(), own_tree):
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
        if violations and enforce:
            for a in agents:
                kill_tree(a.pid)
            break
        if time.monotonic() - t0 > args.timeout:
            timed_out = True
            for a in agents:
                kill_tree(a.pid)
            break
        time.sleep(0.5)
    for a in agents:
        a.wait(timeout=30)
    end = time.monotonic() - t0
    walls = [round(ended.get(i, end), 1) for i in range(len(agents))]
    return violations, timed_out, walls, front


def arm(control: Path, log: Path) -> None:
    with control.open("a", encoding="utf-8") as c:
        c.write("arm\n")
    wait_for(lambda: any(ln.get("ev") == "armed" for ln in sentinel_lines(log)), 5)


def disarm(control: Path, log: Path) -> None:
    with control.open("a", encoding="utf-8") as c:
        c.write("disarm\n")
    wait_for(lambda: any(ln.get("ev") == "disarmed" for ln in sentinel_lines(log)), 5)
    time.sleep(0.3)


def run_parallel(args: argparse.Namespace, command: list[str]) -> dict:
    """PA-01: N copies of BenchLab (desk), one agent each, started at the same moment."""
    import winenv as W

    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    spec = task_spec(args.task)
    n = shared_desktop.instances_for(spec, args.instances)
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    background = args.condition == "background"
    plan = pa_plan(seed, n, out)
    row: dict = {
        "task": args.task, "run_index": args.run_index, "seed": seed, "condition": args.condition,
        "mode": "desk", "platform": "windows", "label": args.label, "started": time.time(),
        "instances": n,
    }  # fmt: skip
    log, control = out / "sentinel.jsonl", out / "sentinel.control"
    for stale in (log, control, out / shared_desktop.PARALLEL_META):
        stale.unlink(missing_ok=True)
    control.write_text("", encoding="utf-8")
    procs: list[subprocess.Popen] = []
    try:
        for entry in plan:
            entry["dir"].mkdir(parents=True, exist_ok=True)
            for stale in (entry["state"], entry["events"]):
                stale.unlink(missing_ok=True)
            entry["brief_path"] = entry["dir"] / "brief.md"
            entry["brief_path"].write_text(
                v2.brief_for(entry["brief"], args.condition), encoding="utf-8"
            )
            procs.append(subprocess.Popen(desk_argv(entry["app_args"], background=background)))
        if not all(wait_for(entry["state"].exists, 30) for entry in plan):
            row["harness_error"] = "BenchLab did not start"
            return row
        if background:
            procs.append(
                subprocess.Popen(
                    sentinel_argv(log, control, v2.USER_WINDOW_FRAME, user_activity=True)
                )
            )
        else:
            procs.append(
                subprocess.Popen(
                    sentinel_argv(
                        log, control, shared_desktop.PA_WITNESS_FRAME, user_activity=False
                    )
                )
            )
        if not wait_for(lambda: any(ln.get("ev") == "ready" for ln in sentinel_lines(log)), 30):
            row["harness_error"] = "sentinel did not start"
            return row
        own = next(ln["hwnd"] for ln in sentinel_lines(log) if ln.get("ev") == "ready")
        with control.open("a", encoding="utf-8") as c:
            c.write("activate\n")
        front_ok = wait_for(lambda: W.user32.GetForegroundWindow() == own, 10)
        row["sentinel_front_at_start"] = front_ok
        time.sleep(1.0)
        arm(control, log)

        t0_ms = time.time() * 1000
        shared_desktop.write_meta(out, n, t0_ms, args.timeout)
        agents = []
        for entry in plan:
            subs = {
                "{brief}": str(entry["brief_path"]), "{out}": str(entry["dir"]), "{seed}": str(seed),
                "{mode}": "desk", "{instance}": entry["label"], "{title}": entry["title"],
            }  # fmt: skip
            agents.append(
                subprocess.Popen(
                    fill(command, subs), cwd=str(entry["dir"]),
                    env=agent_env(entry["brief_path"], CDB_INSTANCE=entry["label"]),
                )
            )  # fmt: skip
        procs.extend(agents)
        violations, timed_out, walls, front = watch_agents(agents, args, args.enforce)
        row["timed_out"] = timed_out
        row["agent_wall_s"] = max(walls, default=0.0)
        disarm(control, log)

        result = out / "result.json"
        subprocess.run(shared_desktop.evaluate_argv(sys.executable, seed, out, result),
                       capture_output=True, timeout=120)  # fmt: skip
        evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        soft: list[str] = []
        for entry in plan:
            hard, mentions = scan_transcript(entry["dir"] / "agent-transcript.jsonl")
            violations += [
                {"t_s": None, "kind": "tool_input", "detail": h, "agent": entry["label"]}
                for h in hard
            ]
            soft += mentions
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
            parallel=shared_desktop.parallel_of(evaluation),
            agents=shared_desktop.aggregate_agents(
                [
                    {"label": entry["label"], "wall_s": walls[i], "timed_out": timed_out,
                     "returncode": agents[i].returncode}
                    for i, entry in enumerate(plan)
                ]
            ),
        )  # fmt: skip
        if evaluation.get("harness_error"):
            row["harness_error"] = evaluation["harness_error"]
        return row
    finally:
        finish(row, procs, control, out)


def run_mouse_user(args: argparse.Namespace, command: list[str]) -> dict:
    """UM-01: one desk, and a sentinel whose user moves and clicks the real pointer (background only)."""
    import winenv as W

    out: Path = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    seed = cdb_fairness.trial_seed(args.task, args.run_index)
    desk = shared_desktop.um_desk(seed)
    script = out / "mouse-user.json"
    shared_desktop.write_mouse_script(seed, script)
    brief = v2.brief_for(
        render_raw(args.task, seed).replace(*PLATFORM_WORDS), "background", mouse_user=True
    )
    (out / "brief.md").write_text(brief, encoding="utf-8")
    row: dict = {
        "task": args.task, "run_index": args.run_index, "seed": seed, "condition": "background",
        "mode": "desk", "platform": "windows", "label": args.label, "started": time.time(),
    }  # fmt: skip
    state, events, log, control = (
        out / "state.json", out / "events.jsonl", out / "sentinel.jsonl", out / "sentinel.control",
    )  # fmt: skip
    for stale in (state, events, log, control):
        stale.unlink(missing_ok=True)
    control.write_text("", encoding="utf-8")
    procs: list[subprocess.Popen] = []
    try:
        procs.append(
            subprocess.Popen(desk_argv(um_desk_args(seed, state, events), background=True))
        )
        if not wait_for(state.exists, 30):
            row["harness_error"] = "BenchLab did not start"
            return row
        procs.append(subprocess.Popen(
            sentinel_argv(log, control, v2.USER_WINDOW_FRAME, user_activity=False, mouse_script=script)
        ))  # fmt: skip
        if not wait_for(lambda: any(ln.get("ev") == "ready" for ln in sentinel_lines(log)), 30):
            row["harness_error"] = "sentinel did not start"
            return row
        own = next(ln["hwnd"] for ln in sentinel_lines(log) if ln.get("ev") == "ready")
        with control.open("a", encoding="utf-8") as c:
            c.write("activate\n")
        row["sentinel_front_at_start"] = wait_for(lambda: W.user32.GetForegroundWindow() == own, 10)
        time.sleep(1.0)
        arm(control, log)
        subs = {"{brief}": str(out / "brief.md"), "{out}": str(out), "{seed}": str(seed),
                "{mode}": "desk", "{instance}": desk["label"], "{title}": desk["title"]}  # fmt: skip
        agent = subprocess.Popen(
            fill(command, subs),
            cwd=str(out),
            env=agent_env(out / "brief.md", CDB_INSTANCE=desk["label"]),
        )
        procs.append(agent)
        violations, timed_out, walls, front = watch_agents([agent], args, args.enforce)
        row.update(agent_exit=agent.returncode, agent_wall_s=walls[0], timed_out=timed_out)
        disarm(control, log)

        result = out / "result.json"
        disturbance = summarize_sentinel.summarize(str(log))
        subprocess.run(
            [sys.executable, str(BENCH / "probes" / args.task / "evaluate.py"), "--seed", str(seed),
             "--state", str(state), "--events", str(events), "--sentinel", str(log),
             "--result", str(result)],
            capture_output=True, timeout=60,
        )  # fmt: skip
        evaluation = json.loads(result.read_text(encoding="utf-8")) if result.exists() else {}
        hard, soft = scan_transcript(out / "agent-transcript.jsonl")
        violations += [{"t_s": None, "kind": "tool_input", "detail": h} for h in hard]
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
            a15=v2.background_condition_score(bg, disturbance, passed, "background"),
            a22=v2.multiplayer_score(disturbance, bg["measured"], passed, "background"),
        )  # fmt: skip
        problem = v2.mouse_user_problem(disturbance)
        if problem is not None:
            row["harness_error"] = problem
        return row
    finally:
        finish(row, procs, control, out)


def render_raw(task: str, seed: int) -> str:
    spec = importlib.util.spec_from_file_location(
        f"render_{task}", BENCH / "probes" / task / "render_brief.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.render(seed)


def finish(row: dict, procs: list[subprocess.Popen], control: Path, out: Path) -> None:
    """Quit the sentinel, stop whatever still runs, and write row.json."""
    with control.open("a", encoding="utf-8") as c:
        c.write("quit\n")
    time.sleep(0.5)
    for proc in procs:
        if proc.poll() is None:
            kill_tree(proc.pid)
    row["finished"] = time.time()
    (out / "row.json").write_text(json.dumps(row, indent=1), encoding="utf-8")


def run(args: argparse.Namespace, command: list[str]) -> dict:
    if args.task == "PA-01":
        return run_parallel(args, command)
    if args.task == "UM-01":
        return run_mouse_user(args, command)
    return run_single(args, command)


def run_single(args: argparse.Namespace, command: list[str]) -> dict:
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
    parser.add_argument(
        "--instances",
        type=int,
        help="PA-01: copies of BenchLab, one agent each (default: task.json)",
    )
    parser.add_argument("--no-enforce", dest="enforce", action="store_false")
    parser.add_argument(
        "--tool-exe", action="append", default=[],
        help="executable of the setup's own tool (repeatable): a shell it starts itself is not a side door",
    )  # fmt: skip
    args = parser.parse_args(argv[:split])
    command = argv[split + 1 :]
    if args.task == "UM-01" and args.condition != "background":
        parser.error("UM-01 runs in the background condition only")
    if args.instances is not None and args.task != "PA-01":
        parser.error("--instances applies to PA-01 only")
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
