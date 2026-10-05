#!/usr/bin/env python3
"""Local macOS pilot: Codex built-in computer use vs Cua Driver, same model.

Diagnostic and non-certifying. Runs on the host Mac (no VM), so it is gated on
human idle time and measures foreground disturbance with a sentinel app. The two
arms differ only in the computer-use tool layer; see PREREGISTRATION.md.

    .venv/bin/python automated-eval/macos_pilot/run_pilot.py --out artifacts/pilot-1 \
        --arms cua-driver-mcp codex-native-cu --tasks CDB-S04 PROBE-FORMS --runs 3
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import random
import shutil
import signal
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "swift"))

import arms  # noqa: E402
import codex_events  # noqa: E402

SCHEMA = "cdb-pilot-trial/1"
SKIP_APP_KINDS = {"terminal", "editor"}
BENCH_TASKS = {
    "CDB-S01": ("tasks/shared/cdb-s01", 1200, ["multi_app", "browser", "coding", "electron"]),
    "CDB-S02": ("tasks/shared/cdb-s02", 1200, ["multi_app", "libreoffice", "electron"]),
    "CDB-S03": ("tasks/shared/cdb-s03", 1200, ["multi_app", "libreoffice", "gnucash", "electron"]),
    "CDB-S04": ("tasks/shared/cdb-s04", 1200, ["multi_app", "browser", "electron"]),
}
PRICE = {"input": 2.5, "cached": 0.25, "output": 15.0}  # USD per million tokens, ASSUMED
ASSUMED_CACHED_FRACTION = 0.85


def _load_compare() -> Any:
    spec = importlib.util.spec_from_file_location(
        "cdb_compare_drivers", REPO / "automated-eval" / "compare_drivers.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@dataclass
class Task:
    id: str
    group: str  # bench | probe
    timeout_s: int
    tags: list[str]
    bundle: Path | None = None  # bench tasks
    probe_dir: Path | None = None  # probes
    app_args: list[str] = field(default_factory=list)


def load_tasks() -> dict[str, Task]:
    tasks: dict[str, Task] = {}
    for task_id, (rel, timeout, tags) in BENCH_TASKS.items():
        tasks[task_id] = Task(task_id, "bench", timeout, tags, bundle=REPO / rel)
    probes = HERE / "probes"
    if probes.is_dir():
        for meta in sorted(probes.glob("*/task.json")):
            data = json.loads(meta.read_text("utf-8"))
            tasks[data["id"]] = Task(
                data["id"],
                "probe",
                int(data.get("timeout_s", 300)),
                list(data.get("dimension_tags", [])),
                probe_dir=meta.parent,
                app_args=list(data.get("app_args", [])),
            )
    return tasks


def probe_seed(task_id: str, run_index: int) -> int:
    digest = hashlib.sha256(f"{task_id}:{run_index}".encode()).hexdigest()
    return int(digest[:8], 16) % 1_000_000


def build_schedule(
    task_ids: list[str], arm_names: list[str], runs: int, seed: int
) -> list[dict[str, Any]]:
    """Blocked randomisation: per run block, tasks in random order, arm order random."""
    rng = random.Random(seed)
    schedule: list[dict[str, Any]] = []
    for run_index in range(runs):
        order = task_ids[:]
        rng.shuffle(order)
        for task_id in order:
            arm_order = arm_names[:]
            rng.shuffle(arm_order)
            for arm in arm_order:
                schedule.append({"task": task_id, "arm": arm, "run_index": run_index})
    for index, entry in enumerate(schedule):
        entry["order_index"] = index
    return schedule


# ---------------------------------------------------------------- environment


def hid_idle_seconds() -> float:
    try:
        out = subprocess.run(
            ["ioreg", "-c", "IOHIDSystem"], capture_output=True, text=True, timeout=10
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return 0.0
    for line in out.splitlines():
        if "HIDIdleTime" in line:
            return int(line.rsplit("=", 1)[1]) / 1e9
    return 0.0


def wait_for_idle(minimum: float, max_wait_s: float, stop_file: Path) -> float | None:
    deadline = time.monotonic() + max_wait_s
    while True:
        if stop_file.exists():
            return None
        idle = hid_idle_seconds()
        if idle >= minimum:
            return idle
        if time.monotonic() > deadline:
            return None
        time.sleep(10)


class IdleSampler:
    """1 Hz HID idle sampler (ioreg). A drop between samples means real HID input occurred."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)

    def _run(self) -> None:
        with self.path.open("w", encoding="utf-8") as handle:
            while not self.stop.is_set():
                handle.write(
                    json.dumps(
                        {"t": round(time.time() * 1000, 1), "idle_s": round(hid_idle_seconds(), 2)}
                    )
                    + "\n"
                )
                handle.flush()
                self.stop.wait(1.0)

    def __enter__(self) -> "IdleSampler":
        self.thread.start()
        return self

    def __exit__(self, *_: Any) -> None:
        self.stop.set()
        self.thread.join(timeout=5)


def hid_drops(path: Path) -> int:
    """Number of HID-idle drops (> 0.5 s) in a sampler log: real HID input events."""
    previous = None
    drops = 0
    for line in path.read_text("utf-8").splitlines():
        value = json.loads(line)["idle_s"]
        if previous is not None and value + 0.5 < previous:
            drops += 1
        previous = value
    return drops


def clean_app_env(home: Path) -> dict[str, str]:
    env = {
        "PATH": f"/opt/homebrew/bin:{arms.CLOSED_PATH}",
        "HOME": str(home),
        "USER": os.environ.get("USER", ""),
        "LANG": "en_US.UTF-8",
        "TMPDIR": str(home / "tmp"),
    }
    (home / "tmp").mkdir(parents=True, exist_ok=True)
    return env


def sweep_processes(needle: str) -> int:
    """Kill leftover processes whose command line contains ``needle``."""
    killed = 0
    out = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True, text=True).stdout
    for line in out.splitlines():
        pid_text, _, command = line.strip().partition(" ")
        if needle in command and int(pid_text) != os.getpid():
            try:
                os.kill(int(pid_text), signal.SIGKILL)
                killed += 1
            except OSError:
                pass
    return killed


def kill_group(pid: int) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pid, sig)
        except OSError:
            return
        time.sleep(1.0)


class Sentinel:
    """BenchSentinel app: frontmost witness for focus, keystroke, click and pointer leaks."""

    def __init__(self, app: Path | None, log: Path) -> None:
        self.app = app
        self.log = log
        self.proc: subprocess.Popen[bytes] | None = None

    @property
    def enabled(self) -> bool:
        return self.app is not None and (self.app / "Contents/MacOS").is_dir()

    def start(self) -> None:
        if not self.enabled:
            return
        exe = next((self.app / "Contents/MacOS").iterdir())
        self.log.parent.mkdir(parents=True, exist_ok=True)
        self.proc = subprocess.Popen(
            [str(exe), "--log", str(self.log)],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        time.sleep(2.0)

    def signal(self, sig: int) -> None:
        if self.proc is not None and self.proc.poll() is None:
            os.kill(self.proc.pid, sig)

    def activate(self) -> None:
        self.signal(signal.SIGUSR1)
        time.sleep(1.5)

    def toggle_armed(self) -> None:
        self.signal(signal.SIGUSR2)
        time.sleep(0.6)

    def stop(self) -> None:
        if self.proc is not None:
            kill_group(self.proc.pid)
            self.proc = None

    def summary(self) -> dict[str, Any]:
        if not self.enabled or not self.log.exists():
            return {"available": False}
        try:
            import summarize_sentinel  # type: ignore

            return summarize_sentinel.summarize(str(self.log))
        except Exception as error:  # noqa: BLE001 - never fail a trial on a summary
            return {"available": False, "error": f"{type(error).__name__}: {error}"}


# ---------------------------------------------------------------- agent run


def run_agent(
    arm: str,
    prompt: str,
    workspace: Path,
    trial_dir: Path,
    timeout_s: int,
    model: str,
    effort: str,
) -> dict[str, Any]:
    home = trial_dir / "home"
    env = arms.render_home(arm, home, model, effort)
    events_path = trial_dir / "codex-events.tsv"
    stderr_path = trial_dir / "codex.stderr"
    started = time.monotonic()
    done = threading.Event()
    with events_path.open("w", encoding="utf-8") as events, stderr_path.open("wb") as err:
        proc = subprocess.Popen(
            arms.codex_argv(workspace, model),
            cwd=workspace,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=err,
            start_new_session=True,
        )

        def pump() -> None:
            assert proc.stdout is not None
            for raw in proc.stdout:
                line = raw.decode("utf-8", "replace").rstrip("\n")
                events.write(f"{time.time() * 1000:.1f}\t{line}\n")
                events.flush()
                if '"type":"turn.completed"' in line.replace(" ", ""):
                    done.set()

        reader = threading.Thread(target=pump, daemon=True)
        reader.start()
        assert proc.stdin is not None
        proc.stdin.write(prompt.encode("utf-8"))
        proc.stdin.close()
        timed_out = False
        while proc.poll() is None:
            if done.is_set():
                time.sleep(3.0)
                if proc.poll() is None:
                    kill_group(proc.pid)
                break
            if time.monotonic() - started > timeout_s:
                timed_out = True
                kill_group(proc.pid)
                break
            time.sleep(0.5)
        reader.join(timeout=5)
    return {
        "returncode": proc.returncode,
        "timed_out": timed_out,
        "agent_wall_s": round(time.monotonic() - started, 2),
        "events_path": events_path,
    }


def estimate_cost(tokens: dict[str, int]) -> dict[str, float]:
    inp, cached, out = tokens["input"], tokens["cached_input"], tokens["output"]
    upper = inp * PRICE["input"] / 1e6 + out * PRICE["output"] / 1e6
    assumed_cached = cached  # measured cache reads when the rollout reports them
    realistic = (
        (inp - assumed_cached) * PRICE["input"] / 1e6
        + assumed_cached * PRICE["cached"] / 1e6
        + out * PRICE["output"] / 1e6
    )
    return {"upper": round(upper, 4), "assumed_cached": round(realistic, 4)}


# ---------------------------------------------------------------- bench tasks


def launch_apps(
    compare: Any,
    descriptor: dict[str, Any],
    variables: dict[str, str],
    app_env: dict[str, str],
    log_dir: Path,
) -> list[subprocess.Popen[bytes]]:
    procs: list[subprocess.Popen[bytes]] = []
    log_dir.mkdir(parents=True, exist_ok=True)
    for raw in descriptor["apps"]:
        app = compare.expand_placeholders(raw, variables)
        if app.get("kind") in SKIP_APP_KINDS:
            continue
        env = {**app_env, **{k: str(v) for k, v in dict(app.get("env", {})).items()}}
        for name in app.get("unset_env", []):
            env.pop(str(name), None)
        cwd = Path(app.get("cwd", variables["bundle"]))
        command = compare._resolve_executable(app["command"])
        out = (log_dir / f"{app['id']}.log").open("wb")
        proc = subprocess.Popen(
            command,
            cwd=cwd,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=out,
            start_new_session=True,
        )
        procs.append(proc)
        ready = app.get("ready")
        url = ready.get("http", ready.get("url")) if isinstance(ready, dict) else None
        if url:
            import urllib.request

            deadline = time.monotonic() + float(ready.get("timeout_seconds", 20))
            while time.monotonic() < deadline:
                if proc.poll() is not None:
                    raise RuntimeError(f"app {app['id']} exited before readiness")
                try:
                    with urllib.request.urlopen(url, timeout=1.0) as response:
                        if response.status < 500:
                            break
                except OSError:
                    time.sleep(0.3)
            else:
                raise TimeoutError(f"app {app['id']} readiness timed out")
    time.sleep(6.0)
    for proc in procs:
        if proc.poll() is not None:
            raise RuntimeError("an app exited during startup")
    return procs


def prepare_bench(
    compare: Any, task: Task, trial_dir: Path, workspace: Path
) -> tuple[dict[str, Any], dict[str, str], str, Path]:
    bundle = task.bundle
    assert bundle is not None
    staged = trial_dir / "bundle"
    subprocess.run(["cp", "-Rc", str(bundle), str(staged)], check=True)
    for hidden in ("evaluator", "tests", "reset", "tools"):
        shutil.rmtree(staged / hidden, ignore_errors=True)
    descriptor = compare.load_launch_descriptor(bundle, "macos")
    variables = compare._descriptor_variables(staged, workspace)
    variables["python"] = sys.executable
    for step in ("setup",):
        command = compare.expand_placeholders(
            descriptor["semantics"]["reset"][step], {**variables, "bundle": str(bundle)}
        )
        subprocess.run(command, cwd=bundle, check=True, capture_output=True, timeout=300)
    brief = (bundle / descriptor["semantics"]["brief"]).read_text("utf-8")
    return descriptor, variables, brief, bundle


def evaluate_bench(
    compare: Any,
    task: Task,
    descriptor: dict[str, Any],
    variables: dict[str, str],
    workspace: Path,
    artifacts: Path,
    agent_exit: int,
) -> dict[str, Any]:
    assert task.bundle is not None
    node = compare._evaluator_node_options(task.bundle, "macos")
    result = artifacts / "evaluator-result.json"
    extra = {
        "evaluator_node": str(node.get("evaluator_node_path", "")),
        "evaluator_node_sha256": str(node.get("evaluator_node_sha256", "")),
        "evaluator_node_version": str(node.get("evaluator_node_version", "")),
        "artifacts": str(artifacts),
        "result": str(result),
        "agent_exit_code": str(agent_exit),
        "bundle": str(task.bundle),
    }
    command = compare.expand_placeholders(
        descriptor["semantics"]["evaluate"], {**variables, **extra}
    )
    completed = subprocess.run(
        command, cwd=task.bundle, capture_output=True, text=True, timeout=600
    )
    (artifacts / "evaluator.stdout").write_text(completed.stdout[-4000:], "utf-8")
    (artifacts / "evaluator.stderr").write_text(completed.stderr[-4000:], "utf-8")
    if not result.is_file():
        return {"passed": False, "score": None, "error": "evaluator wrote no result"}
    data = json.loads(result.read_text("utf-8"))
    return {
        "passed": bool(data.get("passed")),
        "score": data.get("score"),
        "checks": {
            name: bool(check.get("passed")) if isinstance(check, dict) else check
            for name, check in (
                data.get("checks") or (data.get("detail") or {}).get("checks") or {}
            ).items()
        },
    }


# ---------------------------------------------------------------- probes


def prepare_probe(
    task: Task, seed: int, trial_dir: Path, lab_app: Path
) -> tuple[str, dict[str, Path]]:
    assert task.probe_dir is not None
    artifacts = trial_dir / "artifacts"
    paths = {"state": artifacts / "lab-state.json", "events": artifacts / "lab-events.jsonl"}
    spec = importlib.util.spec_from_file_location(
        f"render_{task.id}", task.probe_dir / "render_brief.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    brief = module.render(seed)
    return brief, paths


def launch_lab(
    task: Task, seed: int, paths: dict[str, Path], lab_app: Path, app_env: dict[str, str]
) -> subprocess.Popen[bytes]:
    exe = next((lab_app / "Contents/MacOS").iterdir())
    args = [
        str(exe),
        *task.app_args,
        "--seed",
        str(seed),
        "--state",
        str(paths["state"]),
        "--events",
        str(paths["events"]),
    ]
    proc = subprocess.Popen(
        args,
        env=app_env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and not paths["state"].exists():
        time.sleep(0.3)
    time.sleep(1.5)
    return proc


def evaluate_probe(
    task: Task, seed: int, paths: dict[str, Path], artifacts: Path
) -> dict[str, Any]:
    assert task.probe_dir is not None
    result = artifacts / "evaluator-result.json"
    completed = subprocess.run(
        [
            sys.executable,
            str(task.probe_dir / "evaluate.py"),
            "--seed",
            str(seed),
            "--state",
            str(paths["state"]),
            "--events",
            str(paths["events"]),
            "--result",
            str(result),
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    (artifacts / "evaluator.stderr").write_text(completed.stderr[-4000:], "utf-8")
    if not result.is_file():
        return {"passed": False, "score": None, "error": "evaluator wrote no result"}
    data = json.loads(result.read_text("utf-8"))
    return {
        "passed": bool(data.get("passed")),
        "score": data.get("score"),
        "checks": {
            name: bool(check.get("passed")) if isinstance(check, dict) else check
            for name, check in (
                data.get("checks") or (data.get("detail") or {}).get("checks") or {}
            ).items()
        },
    }


# ---------------------------------------------------------------- one trial


def run_trial(
    args: argparse.Namespace,
    compare: Any,
    task: Task,
    arm: str,
    run_index: int,
    order_index: int,
    out: Path,
    build_dir: Path | None,
) -> dict[str, Any]:
    trial_id = f"{order_index:03d}-{arm}-{task.id}-r{run_index}"
    trial_dir = out / "trials" / trial_id
    trial_dir.mkdir(parents=True)
    artifacts = trial_dir / "artifacts"
    artifacts.mkdir()
    workspace = trial_dir / "workspace"
    seed = probe_seed(task.id, run_index) if task.group == "probe" else 0
    row: dict[str, Any] = {
        "schema": SCHEMA,
        "trial_id": trial_id,
        "arm": arm,
        "task": task.id,
        "task_group": task.group,
        "dimension_tags": task.tags,
        "run_index": run_index,
        "seed": seed,
        "order_index": order_index,
        "started_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "model": args.model,
        "reasoning_effort": args.effort,
    }
    idle_start = wait_for_idle(args.idle_min, args.max_idle_wait_h * 3600, out / "STOP")
    if idle_start is None:
        raise KeyboardInterrupt("idle gate or STOP file")
    row["idle_at_start_s"] = round(idle_start)
    sentinel = Sentinel(
        build_dir / "BenchSentinel.app" if build_dir and not args.no_sentinel else None,
        artifacts / "sentinel.jsonl",
    )
    app_env = clean_app_env(trial_dir / "apphome")
    procs: list[subprocess.Popen[bytes]] = []
    descriptor = variables = None
    agent: dict[str, Any] = {"returncode": None, "timed_out": False, "agent_wall_s": 0.0}
    t0 = time.monotonic()
    evaluation: dict[str, Any] = {"passed": False, "score": None}
    summary: dict[str, Any] = {}
    status = "completed"
    note = ""
    try:
        workspace.mkdir()
        sentinel.start()
        if task.group == "bench":
            descriptor, variables, brief, _ = prepare_bench(compare, task, trial_dir, workspace)
            procs += launch_apps(compare, descriptor, variables, app_env, artifacts / "apps")
        else:
            assert build_dir is not None
            if "clipboard" in task.tags:
                sweep_processes("/Calculator.app/Contents/MacOS/Calculator")
            brief, paths = prepare_probe(task, seed, trial_dir, build_dir / "BenchLab.app")
            procs.append(launch_lab(task, seed, paths, build_dir / "BenchLab.app", app_env))
        sentinel.activate()
        prompt = (
            arms.PREAMBLE
            + f"You have about {task.timeout_s // 60} minutes.\n\n"
            + brief.strip()
            + "\n"
        )
        (artifacts / "prompt.txt").write_text(prompt, "utf-8")
        sentinel.toggle_armed()
        if args.no_agent:
            time.sleep(5)
            (trial_dir / "codex-events.tsv").write_text("", "utf-8")
            agent = {
                "returncode": 0,
                "timed_out": False,
                "agent_wall_s": 5.0,
                "events_path": trial_dir / "codex-events.tsv",
            }
        else:
            with IdleSampler(artifacts / "hid-idle.jsonl"):
                agent = run_agent(
                    arm, prompt, workspace, trial_dir, task.timeout_s, args.model, args.effort
                )
        sentinel.toggle_armed()
        events = codex_events.read_events(agent["events_path"])
        summary = codex_events.summarize(events)
        usage = codex_events.rollout_usage(trial_dir / "home" / ".codex")
        if usage:
            summary["tokens"] = usage
            summary["token_source"] = "rollout"
        if args.no_agent:
            summary["turn_completed"] = True
        if agent["timed_out"]:
            status = "timeout"
        elif agent["returncode"] not in (0, None, -15, -9) and not summary["turn_completed"]:
            status = "infra_error" if summary["infra_error_suspected"] else "agent_error"
        elif not summary["turn_completed"]:
            status = "infra_error" if summary["infra_error_suspected"] else "agent_error"
        if task.group == "bench":
            evaluation = evaluate_bench(
                compare, task, descriptor, variables, workspace, artifacts, agent["returncode"] or 0
            )
        else:
            evaluation = evaluate_probe(task, seed, paths, artifacts)
    except KeyboardInterrupt:
        raise
    except Exception as error:  # noqa: BLE001 - record and continue the matrix
        status = "infra_error"
        note = f"{type(error).__name__}: {error}"
    finally:
        sentinel.stop()
        for proc in procs:
            kill_group(proc.pid)
        sweep_processes(str(trial_dir))
        if task.group == "probe" and "clipboard" in task.tags:
            sweep_processes("/Calculator.app/Contents/MacOS/Calculator")
    summary_sentinel = sentinel.summary()
    hid = summary_sentinel.get("hid_events", {}) if sentinel.enabled else {}
    hid_total = sum(int(v) for v in hid.values()) if hid else 0
    tokens = summary.get("tokens", {"input": 0, "cached_input": 0, "output": 0, "reasoning": 0})
    cost = estimate_cost(tokens)
    row.update(
        {
            "status": status,
            "excluded": status == "infra_error",
            "excluded_reason": (note or "infrastructure failure")
            if status == "infra_error"
            else None,
            "passed": bool(evaluation.get("passed")) and status in ("completed", "timeout"),
            "score": evaluation.get("score"),
            "checks": evaluation.get("checks"),
            "wall_s": round(time.monotonic() - t0, 2),
            "agent_wall_s": agent["agent_wall_s"],
            "tool_calls": summary.get("tool_calls", {"total": 0, "by_class": {}, "failed": 0}),
            "steps": summary.get("steps", 0),
            "shell_commands": summary.get("shell_commands", 0),
            "action_latency_ms": summary.get("action_latency_ms", {}),
            "tokens": tokens,
            "token_source": summary.get("token_source", "turn.completed"),
            "est_cost_usd": cost["assumed_cached"],
            "est_cost_usd_upper": cost["upper"],
            "disturbance": {
                "available": bool(summary_sentinel.get("available")),
                "front_changes": summary_sentinel.get("front_changes", 0),
                "front_changed_to": summary_sentinel.get("front_changed_to", []),
                "key_loss": summary_sentinel.get("key_loss", 0),
                "keystrokes_leaked": summary_sentinel.get("keystrokes_leaked", 0),
                "clicks_leaked": summary_sentinel.get("clicks_leaked", 0),
                "scrolls_leaked": summary_sentinel.get("scrolls_leaked", 0),
                "pointer_max_deviation_px": summary_sentinel.get("pointer_max_deviation_px", 0.0),
                "pointer_deviation_episodes": summary_sentinel.get("pointer_deviation_episodes", 0),
                "hid_events": hid or {"move": 0, "down": 0, "key": 0, "scroll": 0},
                "human_input_suspected": bool(hid_total and idle_start < 900),
            },
            "hid_idle_drops": hid_drops(artifacts / "hid-idle.jsonl")
            if (artifacts / "hid-idle.jsonl").exists()
            else None,
            "confirmation_requested": summary.get("confirmation_requested", False),
            "evaluator_read_suspected": summary.get("evaluator_read_suspected", False),
            "last_message_chars": len(summary.get("last_message", "")),
            "error_events": summary.get("error_events", []),
            "notes": note,
        }
    )
    (trial_dir / "trial.json").write_text(json.dumps(row, indent=2) + "\n", "utf-8")
    # Keep logs, drop bulky or sensitive working state.
    for name in ("workspace", "apphome", "bundle", "home"):
        shutil.rmtree(trial_dir / name, ignore_errors=True)
    return row


# ---------------------------------------------------------------- main


def start_cua_daemon(binary: Path, socket: str, state_dir: Path) -> subprocess.Popen[bytes]:
    """Private Cua Driver daemon for the release under test (own HOME, no telemetry)."""
    home = state_dir / "home"
    home.mkdir(parents=True, exist_ok=True)
    try:
        os.unlink(socket)
    except OSError:
        pass
    env = {
        "HOME": str(home),
        "PATH": f"/opt/homebrew/bin:{arms.CLOSED_PATH}",
        "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
    }
    log = (state_dir / "daemon.log").open("wb")
    proc = subprocess.Popen(
        [str(binary), "serve", "--socket", socket, "--dangerously-bypass-approvals"],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=log,
        start_new_session=True,
    )
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        done = subprocess.run(
            [str(binary), "status", "--socket", socket],
            env=env,
            capture_output=True,
            text=True,
            timeout=10,
        )
        if done.returncode == 0:
            return proc
        time.sleep(0.5)
    kill_group(proc.pid)
    raise RuntimeError("Cua Driver daemon did not become ready")


def environment_manifest(args: argparse.Namespace) -> dict[str, Any]:
    def run(*cmd: str) -> str:
        try:
            return subprocess.run(cmd, capture_output=True, text=True, timeout=20).stdout.strip()
        except (OSError, subprocess.TimeoutExpired):
            return "unavailable"

    return {
        "created_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "macos": run("sw_vers", "-productVersion"),
        "hardware": run("sysctl", "-n", "hw.model"),
        "codex_cli": run(str(arms.CODEX_BIN), "--version"),
        "cua_driver": run(str(arms.CUA_DRIVER_BIN), "--version"),
        "chatgpt_app": run(
            "defaults",
            "read",
            "/Applications/ChatGPT.app/Contents/Info",
            "CFBundleShortVersionString",
        ),
        "bench_repo_commit": run("git", "-C", str(REPO), "rev-parse", "HEAD"),
        "model": args.model,
        "reasoning_effort": args.effort,
        "price_assumed_usd_per_mtok": PRICE,
        "assumed_cached_fraction": ASSUMED_CACHED_FRACTION,
        "preamble_sha256": hashlib.sha256(arms.PREAMBLE.encode()).hexdigest(),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--arms", nargs="+", default=list(arms.ARMS), choices=arms.ARMS)
    parser.add_argument("--tasks", nargs="+", default=None)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--seed", type=int, default=20261005, help="schedule seed")
    parser.add_argument("--model", default="gpt-6-astra")
    parser.add_argument("--effort", default="high", choices=["low", "medium", "high", "xhigh"])
    parser.add_argument(
        "--build-dir", type=Path, default=None, help="dir with BenchSentinel.app and BenchLab.app"
    )
    parser.add_argument("--no-sentinel", action="store_true")
    parser.add_argument(
        "--cua-app",
        type=Path,
        default=None,
        help="CuaDriver.app of the release under test; default is the installed driver and its daemon",
    )
    parser.add_argument(
        "--cua-skills",
        type=Path,
        default=None,
        help="skill directory of the same release (required with --cua-app)",
    )
    parser.add_argument("--cua-socket", default="/tmp/cdb-pilot-cua.sock")
    parser.add_argument(
        "--idle-min",
        type=float,
        default=300.0,
        help="seconds of human idle required before a trial",
    )
    parser.add_argument("--max-idle-wait-h", type=float, default=12.0)
    parser.add_argument(
        "--budget-usd",
        type=float,
        default=30.0,
        help="stop when the assumed-cached estimate exceeds this",
    )
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument(
        "--no-agent",
        action="store_true",
        help="debug: set up, wait 5 s, evaluate, clean up without running Codex",
    )
    args = parser.parse_args()

    tasks = load_tasks()
    selected = args.tasks or list(tasks)
    unknown = [t for t in selected if t not in tasks]
    if unknown:
        parser.error(f"unknown tasks: {unknown}; known: {sorted(tasks)}")
    schedule = build_schedule(selected, list(args.arms), args.runs, args.seed)
    if args.dry_run:
        for entry in schedule:
            print(json.dumps(entry))
        print(f"{len(schedule)} trials")
        return 0

    daemon: subprocess.Popen[bytes] | None = None
    if args.cua_app is not None:
        if args.cua_skills is None:
            parser.error("--cua-skills is required with --cua-app")
        binary = args.cua_app / "Contents/MacOS/cua-driver"
        arms.use_cua_release(binary, args.cua_skills, args.cua_socket)
    problems = [p for arm in args.arms for p in arms.preflight_arm(arm)]
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 2
    import mcp_preflight

    if args.cua_app is not None and "cua-driver-mcp" in args.arms:
        daemon = start_cua_daemon(
            arms.CUA_DRIVER_BIN, args.cua_socket, args.out.resolve() / "cua-daemon"
        )
    for arm in args.arms:
        ok, detail = mcp_preflight.live_check(arm)
        if not ok:
            print(f"arm {arm} is not usable on this host: {detail}", file=sys.stderr)
            if daemon is not None:
                kill_group(daemon.pid)
            return 3
    needs_build = any(tasks[t].group == "probe" for t in selected)
    if needs_build and (args.build_dir is None or not (args.build_dir / "BenchLab.app").exists()):
        parser.error(
            "--build-dir with BenchLab.app is required for probe tasks (see swift/build.sh)"
        )
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    (out / "trials").mkdir(exist_ok=True)
    results = out / "results.jsonl"
    done = set()
    if results.exists():
        for line in results.read_text("utf-8").splitlines():
            row = json.loads(line)
            done.add((row["arm"], row["task"], row["run_index"]))
    (out / "manifest.json").write_text(
        json.dumps({**environment_manifest(args), "schedule": schedule}, indent=2) + "\n", "utf-8"
    )
    compare = _load_compare()
    spent = 0.0
    if results.exists():
        spent = sum(
            json.loads(ln).get("est_cost_usd") or 0 for ln in results.read_text().splitlines()
        )
    for entry in schedule:
        key = (entry["arm"], entry["task"], entry["run_index"])
        if key in done:
            continue
        if spent >= args.budget_usd:
            print(
                f"budget cap reached (assumed-cached estimate ${spent:.2f}); stopping",
                file=sys.stderr,
            )
            break
        try:
            row = run_trial(
                args,
                compare,
                tasks[entry["task"]],
                entry["arm"],
                entry["run_index"],
                entry["order_index"],
                out,
                args.build_dir,
            )
        except KeyboardInterrupt as stop:
            print(f"stopped: {stop}", file=sys.stderr)
            break
        spent += row.get("est_cost_usd") or 0
        with results.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(row) + "\n")
        if row["excluded"] and not args.no_agent:
            # Pre-registered rerun policy: at most one retry per verified infrastructure failure.
            try:
                retry = run_trial(
                    args,
                    compare,
                    tasks[entry["task"]],
                    entry["arm"],
                    entry["run_index"],
                    entry["order_index"] + 1000,
                    out,
                    args.build_dir,
                )
            except KeyboardInterrupt as stop:
                print(f"stopped: {stop}", file=sys.stderr)
                break
            retry["attempt"] = 2
            spent += retry.get("est_cost_usd") or 0
            with results.open("a", encoding="utf-8") as handle:
                handle.write(json.dumps(retry) + "\n")
            row = retry
        print(
            f"{row['trial_id']}: {row['status']} passed={row['passed']} "
            f"wall={row['wall_s']}s steps={row['steps']} est=${row['est_cost_usd']:.2f} total=${spent:.2f}",
            flush=True,
        )
    if daemon is not None:
        kill_group(daemon.pid)
        try:
            os.unlink(args.cua_socket)
        except OSError:
            pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
