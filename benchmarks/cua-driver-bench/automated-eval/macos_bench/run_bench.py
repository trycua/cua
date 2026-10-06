#!/usr/bin/env python3
"""Claude Code computer-use benchmark runner: same model, same harness, two MCP tool layers.

    run_bench.py preflight  --run-id NAME [--build-dir DIR]
    run_bench.py dry-run    --phase1-runs 3 --phase2-runs 2
    run_bench.py run        --run-id NAME --build-dir DIR --phase1-runs 3 --phase2-runs 2 \
                            --cutoff-utc 2026-10-06T03:00:00Z
    run_bench.py status     --run-id NAME

Arms (run_bench.py never mixes them up with the Codex pilot arms):
    cc-cua-driver   Claude Code + Cua Driver 0.34.0 MCP + the Cua Driver skill of that release
    cc-codex-cu     Claude Code + Codex computer-use `cua_repl` MCP (server `codex-cu`), no skill
Fallback arms (`codex exec --json`, behind --allow-codex-arms; not run at scale): codex-native-cu,
codex-cua-driver. They reuse run_pilot.py.

Pre-registered rules are in bench_core.py. Per-trial output lives under runs/<run>/trials/<trial id>/.
Run it from a Terminal.app shell (launch_bench.command): the Codex computer-use service refuses
clients whose responsible app cannot show the macOS Automation prompt.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "swift"))

import arms  # noqa: E402
import bench_core as core  # noqa: E402
import claude_arms as ca  # noqa: E402
import cdb_adapter  # noqa: E402
import claude_driver  # noqa: E402
import claude_events  # noqa: E402
import recorder as rec  # noqa: E402
import run_pilot as pilot  # noqa: E402

SCHEMA = "cdb-bench-trial/1"
WORK = ca.WORK
DEFAULT_LEDGER = WORK / "ledger" / "spend.jsonl"
DEFAULT_RUNS = WORK / "runs"
DISK_FLOOR_GB = 80.0
PRICE_FALLBACK = {  # USD per million tokens, ASSUMED list prices, used only when no total_cost_usd exists
    "input": 3.0,
    "output": 15.0,
    "cache_read": 0.30,
    "cache_write": 6.0,
}


class StopRun(Exception):
    def __init__(self, reason: str, detail: str = "") -> None:
        super().__init__(f"{reason}: {detail}")
        self.reason, self.detail = reason, detail


# ---------------------------------------------------------------- tasks


def load_tasks(probes_dir: Path) -> dict[str, pilot.Task]:
    tasks: dict[str, pilot.Task] = {}
    for meta in sorted(probes_dir.glob("*/task.json")):
        data = json.loads(meta.read_text("utf-8"))
        if data.get("status") == "dropped":
            continue  # kept on disk, not run (PREREGISTRATION.md, amendment of 6 Oct)
        task = pilot.Task(
            data["id"],
            "probe",
            int(data.get("timeout_s", 240)),
            list(data.get("dimension_tags", [])),
            probe_dir=meta.parent,
            app_args=list(data.get("app_args", [])),
        )
        task.spec = data  # type: ignore[attr-defined]
        tasks[task.id] = task
    return tasks


def default_task(tasks: dict[str, pilot.Task], task_id: str) -> bool:
    """In the default schedule: MB-* and CDB-* tasks except those that run in their own run (`separate_run`)."""
    return task_id.startswith(("MB-", "CDB-")) and not task_spec(tasks[task_id]).get("separate_run")


def task_spec(task: pilot.Task) -> dict[str, Any]:
    return getattr(task, "spec", {})


def allowed_apps(task: pilot.Task) -> set[str]:
    spec = task_spec(task)
    names = {"BenchLab", *spec.get("needs_apps", []), *spec.get("allowed_apps", [])}
    out = {n.lower() for n in names}
    if spec.get("kind") == "cdb":
        out |= cdb_adapter.allowed_app_names(spec)
    return out


# ---------------------------------------------------------------- context


@dataclass
class Ctx:
    args: argparse.Namespace
    run_dir: Path
    ledger: core.Ledger
    tasks: dict[str, pilot.Task]
    task_ids: list[str]
    arm_names: list[str]
    full_task_ids: list[str] = field(default_factory=list)  # positions in the pre-registered order
    mcp: dict[str, tuple[Path, str]] = field(default_factory=dict)
    recorder: Any = None
    compress: rec.CompressQueue | None = None
    daemon: subprocess.Popen[bytes] | None = None
    rec_daemon: subprocess.Popen[bytes] | None = None
    quota: dict[str, Any] | None = None
    cutoff: float | None = None
    versions: dict[str, Any] = field(default_factory=dict)
    state: dict[str, Any] = field(default_factory=dict)
    lock: threading.Lock = field(default_factory=threading.Lock)
    abort: threading.Event = field(default_factory=threading.Event)
    block_durations: list[float] = field(default_factory=list)

    def blocks(self) -> list[dict[str, Any]]:
        """Task-major blocks. Positions come from the full ordered list so ``--only-task`` keeps the
        pre-registered first-arm parity."""
        blocks = core.build_blocks(
            self.full_task_ids or self.task_ids,
            self.arm_names,
            self.args.phase1_runs,
            self.args.phase2_runs,
            self.args.schedule_seed,
        )
        kept = [b for b in blocks if b["task"] in self.task_ids]
        index = 0
        for block in kept:
            for entry in block["entries"]:
                entry["order_index"] = index
                if self.args.smoke:
                    entry["trial_id"] = "SMOKE-" + entry["trial_id"]
                index += 1
        return kept

    @property
    def results_path(self) -> Path:
        return self.run_dir / "results.jsonl"

    def log(self, message: str) -> None:
        line = f"{datetime.now(timezone.utc).strftime('%H:%M:%S')} {message}"
        print(line, flush=True)
        with (self.run_dir / "runner.log").open("a", encoding="utf-8") as handle:
            handle.write(line + "\n")

    def set_state(self, **fields: Any) -> None:
        with self.lock:
            self.state.update(fields)
        self.write_heartbeat()

    def write_heartbeat(self) -> None:
        with self.lock:
            state = dict(self.state)
        started = state.get("trial_started_mono")
        beat = {
            "ts": datetime.now(timezone.utc).isoformat(timespec="seconds"),
            "run_id": self.run_dir.name,
            "state": state.get("state", "starting"),
            "trial_id": state.get("trial_id"),
            "attempt": state.get("attempt"),
            "trial_elapsed_s": round(time.monotonic() - started, 1) if started else None,
            "trials_done": state.get("trials_done", 0),
            "blocks_complete": state.get("blocks_complete", 0),
            "blocks_total": state.get("blocks_total", 0),
            "phase": state.get("phase"),
            "spend_run_usd": round(state.get("spend_run", 0.0), 4),
            "spend_ledger_total_usd": self.ledger.cumulative(),
            "quota": self.quota,
            "pause": state.get("pause"),
            "compress_pending": self.compress.pending() if self.compress else 0,
            "free_gb": round(rec.free_gb(self.run_dir), 1),
            "cutoff_utc": datetime.fromtimestamp(self.cutoff, timezone.utc).isoformat(
                timespec="seconds"
            )
            if self.cutoff
            else None,
        }
        tmp = self.run_dir / "heartbeat.json.tmp"
        tmp.write_text(json.dumps(beat, indent=2) + "\n", "utf-8")
        tmp.replace(self.run_dir / "heartbeat.json")


def start_heartbeat(ctx: Ctx) -> threading.Event:
    stop = threading.Event()

    def loop() -> None:
        while not stop.wait(30.0):
            try:
                ctx.write_heartbeat()
            except OSError:
                pass

    threading.Thread(target=loop, daemon=True).start()
    return stop


# ---------------------------------------------------------------- helpers


def git_commit(path: Path) -> str:
    try:
        return (
            subprocess.run(
                ["git", "-C", str(path), "rev-parse", "HEAD"],
                capture_output=True,
                text=True,
                timeout=10,
            ).stdout.strip()
            or "unknown"
        )
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"


def sw_vers() -> str:
    return subprocess.run(
        ["sw_vers", "-productVersion"], capture_output=True, text=True
    ).stdout.strip()


def claude_version() -> str:
    try:
        return subprocess.run(
            [str(ca.CLAUDE_BIN), "--version"],
            capture_output=True,
            text=True,
            timeout=30,
            env=ca.claude_env(),
        ).stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return "unavailable"


def src_hash() -> str:
    digest = hashlib.sha256()
    for path in (
        sorted(HERE.glob("*.py"))
        + sorted((HERE / "probes").rglob("*.py"))
        + sorted((HERE / "probes").rglob("*.md"))
        + sorted((HERE / "probes").rglob("task.json"))
    ):
        if "__pycache__" in path.parts:
            continue
        digest.update(str(path.relative_to(HERE)).encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()


def strip_bulk(value: Any, limit: int = 4000) -> Any:
    """Replace huge strings (base64 screenshots) so stored stream logs stay small."""
    if isinstance(value, str):
        return value if len(value) <= limit else f"{value[:120]}...<{len(value)} chars stripped>"
    if isinstance(value, list):
        return [strip_bulk(v, limit) for v in value]
    if isinstance(value, dict):
        return {k: strip_bulk(v, limit) for k, v in value.items()}
    return value


def strip_stream_file(path: Path) -> int:
    """Rewrite a stream log without bulk payloads. Returns bytes saved."""
    if not path.exists():
        return 0
    before = path.stat().st_size
    out: list[str] = []
    for raw in path.read_text("utf-8", "replace").splitlines():
        head, sep, body = raw.partition("\t") if not raw.startswith("{") else ("", "", raw)
        try:
            obj = json.loads(body)
        except json.JSONDecodeError:
            out.append(raw)
            continue
        text = json.dumps(strip_bulk(obj), separators=(",", ":"))
        out.append(f"{head}\t{text}" if sep else text)
    path.write_text("\n".join(out) + "\n", "utf-8")
    return before - path.stat().st_size


def estimate_cost(tokens: dict[str, int]) -> float:
    return round(sum(tokens.get(k, 0) * PRICE_FALLBACK[k] for k in PRICE_FALLBACK) / 1e6, 6)


def terminal_ancestor() -> bool:
    """True when a Terminal.app process is among this process' ancestors."""
    pid = os.getpid()
    for _ in range(30):
        done = subprocess.run(
            ["ps", "-o", "ppid=,comm=", "-p", str(pid)], capture_output=True, text=True
        )
        parts = done.stdout.strip().split(None, 1)
        if len(parts) < 2:
            return False
        if "Terminal.app" in parts[1]:
            return True
        pid = int(parts[0])
        if pid <= 1:
            return False
    return False


def merge_quota(old: dict[str, Any] | None, new: dict[str, Any] | None) -> dict[str, Any] | None:
    if not new:
        return old
    merged = dict(old or {})
    for key, value in new.items():
        if value is not None:
            merged[key] = value
    merged["seen_utc"] = datetime.now(timezone.utc).isoformat(timespec="seconds")
    return merged


QUOTA_LATEST = WORK / "ledger" / "quota_latest.json"


def note_quota(ctx: Ctx, new: dict[str, Any] | None, source: str) -> None:
    """Persist a fresh reading so the launcher can gate without a model call. Stand-in claude binaries
    never write it."""
    if (
        new
        and ctx.args.claude_bin is None
        and ctx.ledger.path.resolve() == DEFAULT_LEDGER.resolve()
    ):
        core.persist_quota(QUOTA_LATEST, ctx.quota or new, source)


def sleep_with_heartbeat(ctx: Ctx, seconds: float, reason: str) -> bool:
    """Sleep in short steps. Returns False when interrupted by STOP or abort."""
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if (ctx.run_dir / "STOP").exists() or ctx.abort.is_set():
            return False
        time.sleep(min(5.0, max(0.0, end - time.monotonic())))
    return True


def log_pause(ctx: Ctx, kind: str, seconds: float, message: str, trial_id: str | None) -> None:
    entry = {
        "ts": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "kind": kind,
        "planned_s": round(seconds, 1),
        "message": message[:300],
        "trial_id": trial_id,
    }
    with (ctx.run_dir / "pauses.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(json.dumps({**entry, "event": "start"}) + "\n")
    ctx.log(f"PAUSE {kind} {seconds:.0f}s: {message[:120]}")
    ctx.set_state(state="paused", pause=entry)
    t0 = time.monotonic()
    sleep_with_heartbeat(ctx, seconds, kind)
    with (ctx.run_dir / "pauses.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(
            json.dumps(
                {
                    **entry,
                    "event": "end",
                    "ts": datetime.now(timezone.utc).isoformat(timespec="seconds"),
                    "actual_s": round(time.monotonic() - t0, 1),
                }
            )
            + "\n"
        )
    ctx.set_state(state="idle", pause=None)


# ---------------------------------------------------------------- LaunchServices copies

LSREGISTER = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
BENCH_APPS = ("BenchLab.app", "BenchSentinel.app")


def registered_bench_copies() -> list[str]:
    """Paths of BenchLab/BenchSentinel registered with LaunchServices that still exist on disk.
    The Codex computer-use server resolves apps by name or bundle id and refuses an ambiguous
    bundle id, so exactly one copy of each may be registered."""
    try:
        dump = subprocess.run(
            [LSREGISTER, "-dump"], capture_output=True, text=True, timeout=180
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return []
    found = {m.strip() for m in re.findall(r"^path:\s+(.*?\.app)\s+\(0x", dump, re.M)}
    return sorted(p for p in found if Path(p).name in BENCH_APPS and Path(p).exists())


def unregister_stale_copies(build_dir: Path | None) -> list[str]:
    """Reversible: lsregister -u for copies outside the build dir (files are left alone)."""
    keep = Path(build_dir).resolve() if build_dir else None
    gone = []
    for path in registered_bench_copies():
        if keep and Path(path).resolve().parent == keep:
            continue
        subprocess.run([LSREGISTER, "-u", path], capture_output=True, timeout=60)
        gone.append(path)
    return gone


# ---------------------------------------------------------------- daemons and arms


def ensure_agent_daemon(ctx: Ctx) -> None:
    done = subprocess.run(
        [str(ca.CUA_BIN), "status", "--socket", ca.AGENT_SOCKET],
        env=ca.cua_env(ca.CUA_STATE / "home"),
        capture_output=True,
        text=True,
        timeout=15,
    )
    if done.returncode == 0:
        return
    ctx.log("agent daemon not answering; restarting")
    if ctx.daemon is not None:
        claude_driver.kill_group(ctx.daemon.pid)
    ctx.daemon = ca.start_cua_daemon(ca.AGENT_SOCKET, ca.CUA_STATE)


def check_pins(pins: dict[str, Any], arm_names: list[str]) -> list[tuple[str, str, str]]:
    """Compare every pinned version and hash with what is installed now. Any difference fails."""
    observed = ca.observed_pins(
        include_codex="cc-codex-cu" in arm_names, include_cua="cc-cua-driver" in arm_names
    )
    out: list[tuple[str, str, str]] = []
    for key, want, got, ok in core.compare_pins(pins, observed):
        if key not in observed:
            continue  # pin of an arm that is not part of this run
        out.append(
            (
                f"pin {key}",
                "pass" if ok else "fail",
                str(got) if ok else f"observed {got!r}, pinned {want!r}",
            )
        )
    return out


def check_daemon(pins: dict[str, Any]) -> list[tuple[str, str, str]]:
    health = ca.cua_health()
    build = health.get("build", {})
    ok_version = build.get("version") == pins["cua_driver_version"]
    ok_sha = build.get("exe_sha256") == pins["cua_driver_binary_sha256"] and build.get(
        "git_sha"
    ) == pins.get("cua_driver_git_sha", build.get("git_sha"))
    checks = health.get("checks", {})
    perms = (
        checks.get("tcc_accessibility") == "pass" and checks.get("tcc_screen_recording") == "pass"
    )
    return [
        ("daemon reports version", "pass" if ok_version else "fail", str(build.get("version"))),
        (
            "daemon exe sha256 and git sha match the pins",
            "pass" if ok_sha else "fail",
            str(build.get("exe_sha256")),
        ),
        (
            "daemon accessibility + screen recording",
            "pass" if perms else "fail",
            json.dumps({k: v for k, v in checks.items() if k.startswith("tcc_")}),
        ),
    ]


def mcp_list_tools(command: str, args: list[str], env: dict[str, str]) -> list[str]:
    client = rec.McpStdioClient([command, *args], env)
    try:
        reply = client.request("tools/list", {}, timeout=60)
        return [t["name"] for t in reply.get("result", {}).get("tools", [])]
    finally:
        client.close()


def mcp_entry(config_path: Path) -> tuple[str, list[str], dict[str, str]]:
    data = json.loads(config_path.read_text("utf-8"))
    ((name, entry),) = data["mcpServers"].items()
    env = {"HOME": os.environ.get("HOME", ""), "PATH": ca.CLAUDE_PATH, **entry.get("env", {})}
    return entry["command"], list(entry.get("args", [])), env


# ---------------------------------------------------------------- one attempt


def evaluate_probe(
    task: pilot.Task, seed: int, paths: dict[str, Path], artifacts: Path, sentinel_log: Path
) -> dict[str, Any]:
    """Call the task's evaluate.py (interface from #59 plus the optional --sentinel and diagnostics)."""
    assert task.probe_dir is not None
    result = artifacts / "evaluator-result.json"
    command = [
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
        "--sentinel",
        str(sentinel_log),
    ]
    completed = subprocess.run(command, capture_output=True, text=True, timeout=120)
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
        "diagnostics": data.get("diagnostics"),
    }


def run_attempt(ctx: Ctx, entry: dict[str, Any], attempt: int) -> dict[str, Any]:
    args = ctx.args
    task = ctx.tasks[entry["task"]]
    arm = entry["arm"]
    spec = task_spec(task)
    timeout_s = int(args.timeout_s or spec.get("timeout_s") or task.timeout_s or 240)
    max_turns = int(args.max_turns or spec.get("max_turns") or 30)
    run_index = entry["run_index"]
    seed = core.probe_seed(task.id, run_index)
    trial_dir = ctx.run_dir / "trials" / entry["trial_id"] / f"a{attempt}"
    shutil.rmtree(trial_dir, ignore_errors=True)
    artifacts = trial_dir / "artifacts"
    artifacts.mkdir(parents=True)
    t0 = time.monotonic()
    started_utc = datetime.now(timezone.utc).isoformat(timespec="seconds")
    idle_start = pilot.hid_idle_seconds()
    row: dict[str, Any] = {
        "schema": SCHEMA,
        "trial_id": entry["trial_id"],
        "attempt": attempt,
        "final": False,
        "smoke": bool(args.smoke),
        "run_id": ctx.run_dir.name,
        "phase": entry["phase"],
        "block": entry["block"],
        "run": entry["run"],
        "run_index": run_index,
        "arm": arm,
        "task": task.id,
        "task_group": task_spec(task).get("group", "probe"),
        "ability": spec.get("ability"),
        "dimension_tags": task.tags,
        "order_index": entry.get("order_index"),
        "task_pos": entry["task_pos"],
        "arm_slot": entry["arm_slot"],
        "first_arm": entry["first_arm"],
        "schedule_seed": entry["schedule_seed"],
        "seed": seed,
        "started_utc": started_utc,
        "model": args.model,
        "effort": args.effort,
        "max_turns": max_turns,
        "timeout_s": timeout_s,
        "idle_at_start_s": round(idle_start),
        "idle_gate_relaxed": bool(args.idle_min and idle_start < args.idle_min),
        "claude_version": ctx.versions.get("claude"),
        "cua_driver_version": ctx.versions.get("cua_driver_version"),
        "cua_driver_sha256": ctx.versions.get("cua_driver_sha256"),
        "cua_driver_daemon_version": ctx.versions.get("cua_daemon_version"),
        "skills_tree_sha256": ctx.versions.get("cua_skills_tree_sha256")
        if arm == "cc-cua-driver"
        else None,
        "macos": ctx.versions.get("macos"),
        "bench_repo_commit": ctx.versions.get("bench_repo_commit"),
        "bench_src_sha256": ctx.versions.get("bench_src_sha256"),
        "quota_five_hour_before": (ctx.quota or {}).get("five_hour"),
        "quota_seven_day_before": (ctx.quota or {}).get("seven_day"),
    }
    sentinel = pilot.Sentinel(
        Path(args.build_dir) / "BenchSentinel.app"
        if args.build_dir and not args.no_sentinel
        else None,
        artifacts / "sentinel.jsonl",
    )
    app_env = pilot.clean_app_env(trial_dir / "apphome")
    lab: subprocess.Popen[bytes] | None = None
    claude: dict[str, Any] = {
        "returncode": None,
        "timed_out": False,
        "wall_s": 0.0,
        "result_seen": False,
        "elicitations": [],
    }
    summary: dict[str, Any] = {}
    evaluation: dict[str, Any] = {"passed": False, "score": None}
    failure: dict[str, Any] = {"kind": None}
    note = ""
    video_raw: Path | None = None
    agent_started = agent_ended = t0
    stderr_path = trial_dir / "claude.stderr"
    stream_path = trial_dir / "claude-stream.tsv"
    needs = list(spec.get("needs_apps", []))
    cdb = cdb_adapter.CdbTask(spec, artifacts) if spec.get("kind") == "cdb" else None
    running_at_end: dict[str, bool] = {}
    peeks: list[str] = []
    front_seen: list[str] = []
    side_door: set[str] = set()
    watcher: FrontWatcher | None = None
    try:
        ctx.set_state(
            state="setup", trial_id=entry["trial_id"], attempt=attempt, trial_started_mono=t0
        )
        kill_bench_apps()
        reset_needs_apps(needs)
        subprocess.run(
            ["pbcopy"], input=b"", capture_output=True
        )  # no stale clipboard between trials
        if arm == "cc-cua-driver":
            ensure_agent_daemon(ctx)
        lab_app = Path(args.build_dir) / "BenchLab.app"
        if cdb is not None:
            cdb.reset()
            brief, paths = cdb.brief(), {}
            sentinel.start()
            cdb.start_apps(windows=lambda win: place_window(ctx, win))
        else:
            brief, paths = pilot.prepare_probe(task, seed, trial_dir, lab_app)
            sentinel.start()
            lab = pilot.launch_lab(task, seed, paths, lab_app, app_env)
        if spec.get("sentinel_frontmost", True):
            sentinel.activate()
        cwd = ca.prepare_cwd(arm)
        mcp_path, server = ctx.mcp[arm]
        argv = ca.claude_argv(
            mcp_config=mcp_path,
            server=server,
            model=args.model,
            max_turns=max_turns,
            max_budget_usd=args.max_budget_usd,
            tool_search=args.tool_search == "default",
            effort=args.effort,
            coding_tools=bool(spec.get("coding_tools")),
        )
        prompt = brief.strip() + "\n"
        if args.tell_budget:
            prompt += f"\nYou have about {timeout_s // 60} minutes and at most {max_turns} turns.\n"
        (artifacts / "prompt.txt").write_text(prompt, "utf-8")
        (artifacts / "argv.json").write_text(json.dumps(argv, indent=2) + "\n", "utf-8")
        recording = ctx.recorder.start(trial_dir / "rec") if ctx.recorder else False
        row["recording_started"] = bool(recording)
        sentinel.toggle_armed()
        ctx.set_state(state="agent")
        agent_started = time.monotonic()
        watcher = FrontWatcher() if not spec.get("coding_tools") and cdb is not None else None
        if watcher:
            watcher.start()
        with pilot.IdleSampler(artifacts / "hid-idle.jsonl"):
            claude = claude_driver.run_claude(
                argv,
                ca.claude_env(),
                cwd,
                prompt,
                allowed_apps(task),
                timeout_s,
                stream_path,
                stderr_path,
                artifacts / "elicitations.jsonl",
                abort=ctx.abort,
            )
        agent_ended = time.monotonic()
        if watcher:
            watcher.finish()
            front_seen, side_door = sorted(watcher.seen), set(watcher.side)
        running_at_end = apps_running(needs)
        sentinel.toggle_armed()
        ctx.set_state(state="teardown")
        video_raw = ctx.recorder.stop() if ctx.recorder and recording else None
        events = claude_events.read_events(stream_path)
        summary = claude_events.summarize(events)
        stderr_text = (
            stderr_path.read_text("utf-8", "replace")[-4000:] if stderr_path.exists() else ""
        )
        failure = claude_events.classify_failure(summary, stderr_text, claude["returncode"])
        try:
            if cdb is not None:
                rc = claude.get("returncode")
                evaluation = cdb.evaluate(124 if rc is None else int(rc))
                snapshot_workspace(cdb.workspace, artifacts / "workspace-final.tgz")
                peeks = evaluator_peeks(events, spec.get("peek_patterns", []))
                if not spec.get("coding_tools"):
                    side_door |= set(side_door_scan(events))
            else:
                evaluation = evaluate_probe(
                    task, seed, paths, artifacts, artifacts / "sentinel.jsonl"
                )
        except Exception as error:  # noqa: BLE001 - an evaluator crash is a failed trial, recorded
            evaluation = {
                "passed": False,
                "score": None,
                "error": f"{type(error).__name__}: {error}",
            }
    except StopRun:
        raise
    except Exception as error:  # noqa: BLE001 - record, classify as harness exception, never crash the matrix
        failure = {"kind": "harness_exception", "message": f"{type(error).__name__}: {error}"}
        note = failure["message"]
    finally:
        try:
            sentinel.stop()
        except Exception:  # noqa: BLE001
            pass
        if ctx.recorder and video_raw is None and row.get("recording_started"):
            video_raw = ctx.recorder.stop()
        if lab is not None:
            claude_driver.kill_group(lab.pid)
        if watcher is not None:
            watcher.finish()
        if cdb is not None:
            cdb.stop_apps()
        pilot.sweep_processes(str(trial_dir))
        kill_bench_apps()
        reset_needs_apps(needs)
        saved = strip_stream_file(stream_path)
    # --- bookkeeping
    quota_after = merge_quota(ctx.quota, summary.get("quota_last"))
    ctx.quota = quota_after
    note_quota(ctx, summary.get("quota_last"), f"trial {entry['trial_id']}")
    tokens = summary.get("tokens", {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0})
    cost = summary.get("total_cost_usd")
    cost_source = "result.total_cost_usd"
    if cost is None:
        cost = estimate_cost(tokens)
        cost_source = "estimated_from_tokens (no result event)"
    result = summary.get("result", {})
    final_text = summary.get("final_text", "")
    summary_sentinel = sentinel.summary()
    hid = summary_sentinel.get("hid_events", {}) if sentinel.enabled else {}
    hid_total = sum(int(v) for v in hid.values()) if hid else 0
    deviation = float(summary_sentinel.get("pointer_max_deviation_px", 0.0) or 0.0)
    infra = failure.get("kind")
    if claude.get("timed_out"):
        status = "timeout"
    elif infra:
        status = "infra_error"
    elif result.get("subtype") == "error_max_turns":
        status = "max_turns"
    elif result.get("is_error") or not claude.get("result_seen"):
        status = "agent_error"
    else:
        status = "completed"
    # a rate-limit or overload that cut a trial short is an infrastructure failure only when the
    # agent had not already finished; the classifier looks at the result event, so trust it.
    row.update(
        {
            "ended_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
            "status": status,
            "infra_failure": infra,
            "infra_message": failure.get("message", "")[:300] if infra else "",
            "infra_reset_epoch": failure.get("reset_epoch") if infra else None,
            "excluded": bool(infra),
            "excluded_reason": f"infra_failure:{infra}" if infra else None,
            "passed": bool(evaluation.get("passed"))
            and status in ("completed", "timeout", "max_turns", "agent_error")
            and not infra,
            "score": evaluation.get("score"),
            "checks": evaluation.get("checks"),
            "diagnostics": evaluation.get("diagnostics"),
            "evaluator_error": evaluation.get("error"),
            "declared": (
                "DONE"
                if final_text.strip().upper().startswith("DONE")
                else "BLOCKED"
                if final_text.strip().upper().startswith("BLOCKED")
                else "other"
            )
            if final_text
            else "none",
            "final_text": final_text[:500],
            "wall_s": round(time.monotonic() - t0, 2),
            "agent_wall_s": claude.get("wall_s"),
            "overhead_s": round((time.monotonic() - t0) - (claude.get("wall_s") or 0.0), 2),
            "turns": summary.get("turns"),
            "num_turns_result": result.get("num_turns"),
            "tool_calls": summary.get(
                "tool_calls", {"total": 0, "by_class": {}, "by_name": {}, "failed": 0}
            ),
            "steps": (summary.get("tool_calls") or {}).get("total", 0),
            "action_latency_ms": summary.get("action_latency_ms", {}),
            "tokens": {
                "input": tokens.get("input", 0),
                "output": tokens.get("output", 0),
                "cache_read": tokens.get("cache_read", 0),
                "cache_write": tokens.get("cache_write", 0),
                "cached_input": tokens.get("cache_read", 0),
                "reasoning": 0,
            },
            "token_source": summary.get("token_source"),
            "baseline_prompt_tokens": summary.get("baseline_prompt_tokens"),
            "cost_usd": cost,
            "est_cost_usd": cost,
            "cost_source": cost_source,
            "duration_ms": result.get("duration_ms"),
            "duration_api_ms": result.get("duration_api_ms"),
            "stop_reason": result.get("stop_reason"),
            "terminal_reason": result.get("terminal_reason"),
            "result_subtype": result.get("subtype"),
            "api_error_status": result.get("api_error_status"),
            "permission_denials": result.get("permission_denials"),
            "returncode": claude.get("returncode"),
            "timed_out": claude.get("timed_out"),
            "elicitations": [
                {"answer": e.get("answer"), "message": e.get("message", "")[:80]}
                for e in claude.get("elicitations", [])
                if "message" in e
            ],
            "unhandled_requests": [e for e in claude.get("elicitations", []) if "unhandled" in e],
            "init": summary.get("init"),
            "mcp_deferred_inferred": bool(summary.get("init", {}).get("tool_search_available"))
            and (summary.get("baseline_prompt_tokens") or 0) < 25000
            and (summary.get("init", {}).get("mcp_tool_count", 0) > 20),
            "tool_search_calls": (summary.get("tool_calls", {}).get("by_name", {}) or {}).get(
                "ToolSearch", 0
            ),
            "quota_five_hour_after": (quota_after or {}).get("five_hour"),
            "quota_seven_day_after": (quota_after or {}).get("seven_day"),
            "quota_status": (quota_after or {}).get("status"),
            "disturbance": {
                "available": bool(summary_sentinel.get("available")),
                "front_changes": summary_sentinel.get("front_changes", 0),
                "front_changed_to": summary_sentinel.get("front_changed_to", []),
                "key_loss": summary_sentinel.get("key_loss", 0),
                "keystrokes_leaked": summary_sentinel.get("keystrokes_leaked", 0),
                "clicks_leaked": summary_sentinel.get("clicks_leaked", 0),
                "scrolls_leaked": summary_sentinel.get("scrolls_leaked", 0),
                "pointer_max_deviation_px": deviation,
                "pointer_deviation_episodes": summary_sentinel.get("pointer_deviation_episodes", 0),
                "pointer_moved": deviation > pilot_deviation_px(),
                "frontmost_changed": bool(summary_sentinel.get("front_changes", 0)),
                "hid_events": hid or {"move": 0, "down": 0, "key": 0, "scroll": 0},
                "human_input_suspected": bool(hid_total and idle_start < 900),
            },
            "hid_idle_drops": pilot.hid_drops(artifacts / "hid-idle.jsonl")
            if (artifacts / "hid-idle.jsonl").exists()
            else None,
            "confirmation_requested": False,
            "evaluator_read_suspected": bool(peeks),
            "evaluator_peeks": peeks,
            "frontmost_seen": front_seen,
            "side_door": sorted(side_door),
            "side_door_flag": bool(side_door),
            "recorder": getattr(ctx.recorder, "name", "none"),
            "video": str(trial_dir / "video" / "video.mp4") if video_raw else None,
            "video_720p": str(trial_dir / "video" / "video-720p.mp4") if video_raw else None,
            "needs_apps_running_at_end": running_at_end,
            "stream_bytes_saved": saved,
            "notes": note,
        }
    )
    if video_raw and ctx.compress:
        ctx.compress.submit(video_raw, trial_dir / "video")
    (trial_dir / "trial.json").write_text(json.dumps(row, indent=2) + "\n", "utf-8")
    for name in ("apphome",):
        shutil.rmtree(trial_dir / name, ignore_errors=True)
    return row


def snapshot_workspace(workspace: Path, dest: Path) -> None:
    """Small tarball of the final workspace (no browser profile, no node_modules) for the audit trail."""
    subprocess.run(
        [
            "tar",
            "-czf",
            str(dest),
            "--exclude",
            ".chromium-profile",
            "--exclude",
            "node_modules",
            "--exclude",
            ".runtime",
            "-C",
            str(workspace.parent),
            workspace.name,
        ],
        capture_output=True,
        timeout=120,
    )


DEFAULT_PEEK_PATTERNS = (
    "evaluator",
    "oracle",
    "hidden_test",
    "taskpack",
    "bench-work",
    "cdbeval",
    "cdb-eval",
    ".cdb-secrets",
)


def evaluator_peeks(events: list[dict[str, Any]], patterns: list[str]) -> list[str]:
    """Tool calls whose input names evaluator material or the task pack (the pack sits on the same
    disk as the agent; this is the control, see TASKS.md). Returns 'Tool:pattern' strings."""
    found: list[str] = []
    pats = list(patterns) or list(DEFAULT_PEEK_PATTERNS)
    for event in events:
        if event.get("type") != "assistant":
            continue
        for block in (event.get("message") or {}).get("content") or []:
            if isinstance(block, dict) and block.get("type") == "tool_use":
                text = json.dumps(block.get("input", {}))
                for pat in pats:
                    if pat in text:
                        found.append(f"{block.get('name')}:{pat}")
    return found[:20]


def place_window(ctx: "Ctx", win: dict[str, Any], wait_s: float = 90.0) -> bool:
    """Wait for a window by title, then move and size it, through the recorder daemon.
    Returns False when the window never appeared (LibreOffice needs a while on a cold start)."""
    needle = win.get("title") or win.get("title_contains")
    bounds = win.get("bounds")
    if not needle or not bounds:
        return True
    rec_home = ca.RECORDER_STATE / "home"
    deadline = time.monotonic() + wait_s
    while time.monotonic() < deadline:
        done = ca.cua_cli(
            "call", "list_windows", "{}", socket=ca.RECORDER_SOCKET, home=rec_home, timeout=20
        )
        try:
            data = json.loads(done.stdout[done.stdout.index("{") :])
        except (ValueError, json.JSONDecodeError):
            time.sleep(0.5)
            continue
        for item in _walk_dicts(data):
            title = str(item.get("title") or item.get("window_title") or "")
            wid = item.get("window_id", item.get("id"))
            pid = item.get("pid", item.get("owner_pid"))
            if needle.lower() in title.lower() and wid is not None and pid is not None:
                args = {
                    "pid": pid,
                    "window_id": wid,
                    "x": bounds["x"],
                    "y": bounds["y"],
                    "width": bounds["width"],
                    "height": bounds["height"],
                }
                ca.cua_cli(
                    "call",
                    "set_window_frame",
                    json.dumps(args),
                    socket=ca.RECORDER_SOCKET,
                    home=rec_home,
                    timeout=20,
                )
                return True
        time.sleep(0.5)
    return False


def _walk_dicts(node: Any):
    if isinstance(node, dict):
        yield node
        for value in node.values():
            yield from _walk_dicts(value)
    elif isinstance(node, list):
        for value in node:
            yield from _walk_dicts(value)


SIDE_DOOR_BUNDLES = {
    "com.apple.Terminal": "Terminal",
    "com.googlecode.iterm2": "iTerm",
    "com.apple.ScriptEditor2": "Script Editor",
    "com.apple.automator": "Automator",
    "com.apple.shortcuts": "Shortcuts",
}
SIDE_DOOR_PROCESSES = ("Script Editor", "Automator", "iTerm2", "Shortcuts")
SIDE_DOOR_PATTERNS = (
    "Terminal",
    "iTerm",
    "Script Editor",
    "ScriptEditor",
    "Automator",
    "Shortcuts",
    "osascript",
    "child_process",
    "execSync",
    "spawnSync",
    "/bin/sh",
    "/bin/zsh",
    "/bin/bash",
    "bash -c",
    "zsh -c",
    "subprocess",
    "os.system",
)


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


class FrontWatcher(threading.Thread):
    """Samples the frontmost app and the side-door processes during the agent phase (GUI-only tasks)."""

    def __init__(self, interval: float = 0.5) -> None:
        super().__init__(daemon=True)
        self.interval = interval
        self.stop_event = threading.Event()
        self.seen: set[str] = set()
        self.side: set[str] = set()

    def run(self) -> None:
        while not self.stop_event.is_set():
            bundle = frontmost_bundle()
            if bundle:
                self.seen.add(bundle)
                if bundle in SIDE_DOOR_BUNDLES:
                    self.side.add(f"front:{SIDE_DOOR_BUNDLES[bundle]}")
            for name in SIDE_DOOR_PROCESSES:
                if subprocess.run(["pgrep", "-x", name], capture_output=True).returncode == 0:
                    self.side.add(f"process:{name}")
            self.stop_event.wait(self.interval)

    def finish(self) -> None:
        self.stop_event.set()
        self.join(timeout=5)


def side_door_scan(events: list[dict[str, Any]]) -> list[str]:
    """GUI-only trials: tool inputs that name a terminal, a scripting app or a shell escape."""
    found: list[str] = []
    for event in events:
        if event.get("type") != "assistant":
            continue
        for block in (event.get("message") or {}).get("content") or []:
            if isinstance(block, dict) and block.get("type") == "tool_use":
                text = json.dumps(block.get("input", {}))
                for pat in SIDE_DOOR_PATTERNS:
                    if pat in text:
                        found.append(f"{block.get('name')}:{pat}")
    return sorted(set(found))[:20]


def kill_bench_apps() -> None:
    """Exact process names only (a command-line match once killed a compiler)."""
    for name in ("BenchLab", "BenchSentinel"):
        subprocess.run(["pkill", "-x", name], capture_output=True)
    time.sleep(0.3)


SAVED_STATE = {
    "Calculator": [
        Path.home() / "Library/Saved Application State/com.apple.calculator.savedState",
        Path.home()
        / "Library/Containers/com.apple.calculator/Data/Library/Saved Application State/com.apple.calculator.savedState",
    ],
}


def reset_needs_apps(names: list[str]) -> None:
    """Kill the apps a task needs and remove their saved window state so each trial starts clean."""
    for name in names:
        subprocess.run(["pkill", "-x", name], capture_output=True)
    time.sleep(0.3)
    for name in names:
        for path in SAVED_STATE.get(name, []):
            shutil.rmtree(path, ignore_errors=True)


def apps_running(names: list[str]) -> dict[str, bool]:
    return {
        n: subprocess.run(["pgrep", "-x", n], capture_output=True).returncode == 0 for n in names
    }


def pilot_deviation_px() -> float:
    import summarize_sentinel  # type: ignore

    return float(getattr(summarize_sentinel, "DEVIATION_PX", 10.0))


# ---------------------------------------------------------------- one trial with retries


def append_row(ctx: Ctx, row: dict[str, Any]) -> None:
    with ctx.results_path.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(row) + "\n")
    cost = row.get("cost_usd") or 0.0
    ctx.ledger.append(
        ctx.args.ledger_who,
        f"{'smoke ' if row.get('smoke') else ''}trial {row['trial_id']} a{row['attempt']}",
        ctx.args.model,
        cost,
        run_id=ctx.run_dir.name,
        status=row.get("status"),
        seven_day_after=row.get("quota_seven_day_after"),
    )
    with ctx.lock:
        ctx.state["spend_run"] = ctx.state.get("spend_run", 0.0) + cost


def check_gates(ctx: Ctx) -> None:
    """Raise StopRun, or wait, before a new attempt may start."""
    args = ctx.args
    while True:
        if (ctx.run_dir / "STOP").exists():
            raise StopRun("USER", "STOP file")
        if core.past_cutoff(ctx.cutoff):
            raise StopRun("TIME", "cutoff reached")
        if not core.budget_allows(
            ctx.ledger.cumulative(), args.max_budget_usd, args.budget_cap_usd
        ):
            raise StopRun("BUDGET", f"cap {args.budget_cap_usd}")
        free = rec.free_gb(ctx.run_dir)
        if free < DISK_FLOOR_GB:
            if ctx.compress and ctx.compress.pending():
                ctx.log(f"disk {free:.1f} GB < floor; waiting for compression")
                ctx.compress.join()
                continue
            raise StopRun("DISK", f"{free:.1f} GB free")
        gate = core.quota_gate(ctx.quota, args.seven_day_stop)
        if gate["action"] == "stop":
            raise StopRun("QUOTA", gate["reason"])
        if gate["action"] == "wait":
            wait_until = gate.get("wait_until")
            seconds = core.wait_seconds(0, wait_until)
            if wait_until and core.past_cutoff(ctx.cutoff, time.time() + seconds):
                raise StopRun("TIME", "quota reset is after the cutoff")
            log_pause(ctx, "quota_wait", seconds, gate["reason"], ctx.state.get("trial_id"))
            ctx.quota = dict(ctx.quota or {}, status="allowed_after_wait", five_hour=None)
            continue
        return


def execute_trial(ctx: Ctx, entry: dict[str, Any]) -> dict[str, Any]:
    rate_waits = overload_tries = infra_tries = 0
    attempt = 0
    while True:
        check_gates(ctx)
        attempt += 1
        ctx.log(f"trial {entry['trial_id']} attempt {attempt} start")
        row = run_attempt(ctx, entry, attempt)
        kind = row.get("infra_failure")
        row["final"] = not kind
        decision = "final"
        wait_s = 0.0
        if kind == "rate_limit":
            gate = core.quota_gate(ctx.quota, ctx.args.seven_day_stop)
            if gate["action"] == "stop":
                row["final"] = False
                row.pop("_quota_events", None)
                append_row(ctx, row)
                raise StopRun("QUOTA", gate["reason"])
            reset = row.get("infra_reset_epoch") or (
                gate.get("wait_until") if gate["action"] == "wait" else None
            )
            decision, wait_s = "retry", core.wait_seconds(rate_waits, reset)
            rate_waits += 1
        elif kind == "overloaded":
            if overload_tries < core.OVERLOAD_RETRIES:
                decision, wait_s = "retry", core.backoff_seconds(overload_tries)
                overload_tries += 1
            else:
                row["final"] = True
                decision = "exclude"
        elif kind == "auth":
            if infra_tries < 1:
                decision, wait_s = "retry", 30.0
                infra_tries += 1
            else:
                row["final"] = True
                append_row(ctx, row)
                raise StopRun("AUTH", row.get("infra_message", ""))
        elif kind:
            if infra_tries < 1:
                decision, wait_s = "retry", 5.0
                infra_tries += 1
            else:
                row["final"] = True
                decision = "exclude"
        row.pop("_quota_events", None)
        append_row(ctx, row)
        ctx.log(
            f"trial {entry['trial_id']} a{attempt}: {row['status']} passed={row['passed']} infra={kind} "
            f"wall={row['wall_s']}s turns={row.get('turns')} cost=${row['cost_usd']:.3f} 7d={row.get('quota_seven_day_after')}"
        )
        if decision in ("final", "exclude"):
            ctx.set_state(
                state="idle",
                trials_done=ctx.state.get("trials_done", 0) + 1,
                trial_started_mono=None,
            )
            return row
        if kind == "mcp_start" and entry["arm"] == "cc-cua-driver":
            try:
                ensure_agent_daemon(ctx)
            except Exception as error:  # noqa: BLE001
                ctx.log(f"daemon restart failed: {error}")
        log_pause(ctx, f"retry_{kind}", wait_s, row.get("infra_message", ""), entry["trial_id"])


# ---------------------------------------------------------------- preflight


def quota_probe(ctx: Ctx) -> tuple[bool, str]:
    """One tiny haiku call (no MCP, no tools) to read the subscription utilisation."""
    argv = [
        str(ca.CLAUDE_BIN),
        "-p",
        "--model",
        "claude-haiku-4-5",
        "--system-prompt",
        "Reply with the word OK.",
        "--output-format",
        "stream-json",
        "--verbose",
        "--no-session-persistence",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "project",
        "--max-turns",
        "1",
        "Reply OK",
    ]
    cwd = ca.prepare_cwd("cc-codex-cu", ca.CWD_ROOT)
    env = ca.claude_env()
    token_fd = ca.open_token_fd()
    if token_fd is not None:
        env["CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR"] = str(token_fd)
    try:
        done = subprocess.run(
            argv,
            cwd=str(cwd),
            env=env,
            capture_output=True,
            text=True,
            timeout=120,
            stdin=subprocess.DEVNULL,
            pass_fds=(token_fd,) if token_fd is not None else (),
        )
    finally:
        if token_fd is not None:
            os.close(token_fd)
    events = []
    for line in done.stdout.splitlines():
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    summary = claude_events.summarize(events)
    ctx.ledger.append(
        ctx.args.ledger_who, "quota probe", "claude-haiku-4-5", summary.get("total_cost_usd")
    )
    ctx.quota = merge_quota(ctx.quota, summary.get("quota_last"))
    note_quota(ctx, summary.get("quota_last"), "quota probe")
    if not summary.get("quota_last"):
        return False, "no rate_limit_event in the stream; utilisation unknown"
    q = ctx.quota or {}
    return (
        True,
        f"five_hour={q.get('five_hour')} seven_day={q.get('seven_day')} status={q.get('status')} resets_5h={q.get('five_hour_resets_at')} resets_7d={q.get('seven_day_resets_at')}",
    )


def init_check(ctx: Ctx, arm: str) -> tuple[str, str]:
    """One trivial haiku call per arm through the real argv: verify the init event."""
    mcp_path, server = ctx.mcp[arm]
    cwd = ca.prepare_cwd(arm)
    debug = ctx.run_dir / f"preflight-debug-{arm}.txt"
    argv = ca.claude_argv(
        mcp_config=mcp_path,
        server=server,
        model="claude-haiku-4-5",
        max_turns=1,
        max_budget_usd=1,
        tool_search=ctx.args.tool_search == "default",
        effort=None,
        debug_file=debug,
    )
    stream = ctx.run_dir / f"preflight-init-{arm}.tsv"
    result = claude_driver.run_claude(
        argv,
        ca.claude_env(),
        cwd,
        "Reply OK. Do not call any tool.",
        set(),
        120,
        stream,
        ctx.run_dir / f"preflight-{arm}.stderr",
        ctx.run_dir / f"preflight-{arm}-elicit.jsonl",
    )
    summary = claude_events.summarize(claude_events.read_events(stream))
    ctx.ledger.append(
        ctx.args.ledger_who,
        f"preflight init check {arm}",
        "claude-haiku-4-5",
        summary.get("total_cost_usd"),
    )
    ctx.quota = merge_quota(ctx.quota, summary.get("quota_last"))
    note_quota(ctx, summary.get("quota_last"), f"init check {arm}")
    init = summary["init"]
    problems = []
    want_tools = sorted(ca.builtin_tools(ctx.args.tool_search == "default"))
    if init["tools_builtin"] != want_tools:
        problems.append(f"built-in tools {init['tools_builtin']} != {want_tools}")
    connected = [s for s in init["mcp_servers"] if s.get("status") == "connected"]
    if len(connected) != 1 or connected[0].get("name") != server or len(init["mcp_servers"]) != 1:
        problems.append(f"mcp_servers {init['mcp_servers']}")
    has_skill = "cua-driver" in init["skills"]
    if has_skill != (arm == "cc-cua-driver"):
        problems.append(f"cua-driver skill present={has_skill}")
    leaked = [s for s in init["skills"] if s != "cua-driver" and s not in BUNDLED_SKILLS_OK]
    if leaked:
        problems.append(f"unexpected skills {leaked}")
    if init["plugins"] and any(not str(p).startswith("cc-plugin-") for p in init["plugins"]):
        problems.append(f"non-builtin plugins {init['plugins']}")
    deferred = ""
    if debug.exists():
        for line in debug.read_text("utf-8", "replace").splitlines():
            if "Dynamic tool loading" in line:
                deferred = line.split("] ", 1)[-1]
    detail = (
        f"tools={init['tools_builtin']} mcp_tools={init['mcp_tool_count']} skills={len(init['skills'])} "
        f"baseline_prompt_tokens={summary['baseline_prompt_tokens']} cost=${summary.get('total_cost_usd')} {deferred}"
    )
    ctx.versions.setdefault("init_checks", {})[arm] = {
        "tools_builtin": init["tools_builtin"],
        "mcp_tool_count": init["mcp_tool_count"],
        "skills": init["skills"],
        "baseline_prompt_tokens_haiku": summary["baseline_prompt_tokens"],
        "deferral_log": deferred,
        "mcp_servers": init["mcp_servers"],
    }
    if problems:
        return "fail", "; ".join(problems) + " | " + detail
    return "pass", detail


BUNDLED_SKILLS_OK = {
    "deep-research",
    "design",
    "design-sync",
    "dataviz",
    "update-config",
    "verify",
    "debug",
    "code-review",
    "simplify",
    "batch",
    "fewer-permission-prompts",
    "doctor",
    "loop",
    "schedule",
    "claude-api",
    "workflow-authoring",
    "run",
    "run-skill-generator",
    "plugin-authoring",
}


def preflight(ctx: Ctx, with_models: bool = True) -> list[tuple[str, str, str]]:
    args = ctx.args
    checks: list[tuple[str, str, str]] = []

    def add(name: str, status: str, detail: str) -> None:
        checks.append((name, status, detail))
        print(f"[{status.upper():4}] {name}: {detail}", flush=True)

    pins = ca.load_pins()
    add(
        "claude binary",
        "pass" if ca.CLAUDE_BIN.is_file() else "fail",
        f"{ca.CLAUDE_BIN} {ctx.versions.get('claude')}",
    )
    add(
        "terminal.app ancestor",
        "pass" if terminal_ancestor() else ("warn" if args.allow_non_terminal else "fail"),
        "runner started from Terminal.app"
        if terminal_ancestor()
        else "not started from Terminal.app (required for arm cc-codex-cu)",
    )
    free = rec.free_gb(ctx.run_dir)
    add(
        "disk",
        "pass" if free > DISK_FLOOR_GB else "fail",
        f"{free:.1f} GB free (floor {DISK_FLOOR_GB:.0f})",
    )
    git_root = subprocess.run(
        ["git", "-C", str(ca.CWD_ROOT.parent), "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
    )
    add(
        "agent cwd outside any git checkout",
        "pass" if git_root.returncode != 0 else "fail",
        str(ca.CWD_ROOT),
    )
    for name, status, detail in check_pins(pins, ctx.arm_names):
        add(name, status, detail)
    offline = bool(getattr(args, "offline", False))
    needs_a = "cc-cua-driver" in ctx.arm_names and not offline
    if needs_a:
        try:
            if (
                ctx.daemon is None
                and subprocess.run(
                    [str(ca.CUA_BIN), "status", "--socket", ca.AGENT_SOCKET],
                    env=ca.cua_env(ca.CUA_STATE / "home"),
                    capture_output=True,
                ).returncode
                != 0
            ):
                ctx.daemon = ca.start_cua_daemon(ca.AGENT_SOCKET, ca.CUA_STATE)
            for name, status, detail in check_daemon(pins):
                add(name, status, detail)
            health = ca.cua_health()
            ctx.versions["cua_daemon_version"] = health.get("build", {}).get("version")
            tools = mcp_list_tools(
                str(ca.CUA_BIN),
                ["--socket", ca.AGENT_SOCKET, "mcp"],
                ca.cua_env(ca.CUA_STATE / "home"),
            )
            add(
                "arm A MCP server starts and lists tools",
                "pass" if "list_apps" in tools and len(tools) >= 50 else "fail",
                f"{len(tools)} tools",
            )
        except Exception as error:  # noqa: BLE001
            add("arm A daemon/MCP", "fail", f"{type(error).__name__}: {error}")
    if "cc-codex-cu" in ctx.arm_names and not offline:
        try:
            path, server = ctx.mcp["cc-codex-cu"]
            command, cargs, env = mcp_entry(path)
            tools = mcp_list_tools(command, cargs, env)
            add(
                "arm B MCP server starts and lists tools",
                "pass" if {"js", "js_reset"} <= set(tools) else "fail",
                f"server {server}: {tools}",
            )
        except Exception as error:  # noqa: BLE001
            add("arm B MCP server", "fail", f"{type(error).__name__}: {error}")
    cdb_ids = [t for t in ctx.task_ids if task_spec(ctx.tasks[t]).get("kind") == "cdb"]
    if cdb_ids:
        want = ca.load_pins().get("cdb_pack", {}).get("tree_sha256", {})
        for tid in cdb_ids:
            spec = task_spec(ctx.tasks[tid])
            try:
                probe = cdb_adapter.CdbTask(spec, ctx.run_dir)
                got = probe._digest_hidden() or cdb_adapter.tree_sha256(probe.bundle)
                ok = got == want.get(spec["pack_task"])
                add(f"CDB pack {tid} matches its pinned digest", "pass" if ok else "fail", got[:16])
                names = cdb_adapter.allowed_app_names(spec)
                add(f"CDB {tid} descriptor readable", "pass", f"{len(names)} app names")
            except Exception as error:  # noqa: BLE001
                add(f"CDB pack {tid}", "fail", f"{type(error).__name__}: {error}")
        for app in ("Google Chrome", "LibreOffice", "Gnucash"):
            add(
                f"CDB app installed: {app}",
                "pass" if Path(f"/Applications/{app}.app").is_dir() else "fail",
                f"/Applications/{app}.app",
            )
        electron = list(cdb_adapter.pack_tasks_root().glob("*/*/apps/*/node_modules/electron/dist"))
        add("CDB Electron apps installed (npm ci + install.js)", "pass" if len(electron) >= 5 else "fail", f"{len(electron)} of 5")
        add(
            "CDB evaluator isolation (pack unreadable by the agent's user)",
            "pass" if os.environ.get("CDB_EVAL_SUDO") == "1" else "warn",
            "CDB_EVAL_SUDO=1: evaluator, oracle and hidden tests run as user cdbeval",
        )
        add(
            "CDB disposable-environment flag",
            "pass" if os.environ.get("CDB_BENCH_DISPOSABLE") == "1" else "fail",
            "CDB_BENCH_DISPOSABLE=1 (the runner kills Electron/Chrome/LibreOffice by name)",
        )
    build = Path(args.build_dir) if args.build_dir else None
    ok_build = bool(
        build and (build / "BenchLab.app").is_dir() and (build / "BenchSentinel.app").is_dir()
    )
    add("BenchLab + BenchSentinel builds", "pass" if ok_build else "fail", str(build))
    copies = registered_bench_copies()
    per_name = {n: [c for c in copies if Path(c).name == n] for n in BENCH_APPS}
    ok_copies = all(len(v) == 1 for v in per_name.values())
    add(
        "exactly one registered copy of BenchLab and BenchSentinel",
        "pass" if ok_copies else "fail",
        json.dumps(per_name) if not ok_copies else f"{[v[0] for v in per_name.values()]}",
    )
    ps = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True, text=True).stdout
    leftovers = [
        ln.split(None, 1)[1][:80]
        for ln in ps.splitlines()
        if any(
            n in ln
            for n in (
                "BenchLab.app/Contents/MacOS",
                "BenchSentinel.app/Contents/MacOS",
                "/Calculator.app/Contents/MacOS/Calculator",
            )
        )
    ]
    add(
        "no leftover BenchLab/Sentinel/Calculator",
        "pass" if not leftovers else "warn",
        f"{leftovers}",
    )
    if offline:
        add("daemons, MCP servers, recording, model calls", "warn", "skipped (--offline)")
    elif ctx.recorder is not None and ctx.recorder.name != "none":
        test = ctx.run_dir / "preflight-rec"
        shutil.rmtree(test, ignore_errors=True)
        started = ctx.recorder.start(test)
        time.sleep(3.0)
        raw = ctx.recorder.stop() if started else None
        dur = rec.ffprobe_duration(raw) if raw else None
        add(
            "screen recording (3 s test)",
            "pass" if raw and dur and dur > 0.5 else "fail",
            f"{raw} duration={dur} err={getattr(ctx.recorder, 'last_error', None)}",
        )
    else:
        add("screen recording", "warn", "disabled (--no-recorder)")
    gate = core.gate_decision(
        core.latest_known_quota(QUOTA_LATEST, ctx.ledger.path), ctx.args.seven_day_stop
    )
    add(
        "quota gate from the latest known reading",
        "fail"
        if gate["action"] == "gated"
        else ("warn" if gate["action"] == "unknown" else "pass"),
        gate["reason"],
    )
    if (WORK / "HOLD").exists():
        add("HOLD file", "fail", (WORK / "HOLD").read_text("utf-8").strip()[:200])
    if with_models and not offline:
        try:
            ok, detail = quota_probe(ctx)
            add("quota probe (haiku-4-5)", "pass" if ok else "fail", detail)
        except Exception as error:  # noqa: BLE001
            add("quota probe", "fail", f"{type(error).__name__}: {error}")
        for arm in ctx.arm_names:
            try:
                status, detail = init_check(ctx, arm)
                add(f"init event check {arm}", status, detail)
            except Exception as error:  # noqa: BLE001
                add(f"init event check {arm}", "fail", f"{type(error).__name__}: {error}")
    return checks


# ---------------------------------------------------------------- setup


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument(
        "command", choices=["preflight", "dry-run", "run", "status", "finalize", "gate-check"]
    )
    p.add_argument("--run-id", default=None, help="name under runs/ (default: UTC timestamp)")
    p.add_argument("--runs-dir", type=Path, default=DEFAULT_RUNS)
    p.add_argument("--ledger", type=Path, default=DEFAULT_LEDGER)
    p.add_argument("--ledger-who", default="runner")
    p.add_argument("--arms", nargs="+", default=list(arms.CLAUDE_ARMS), choices=list(arms.ALL_ARMS))
    p.add_argument("--only-arm", default=None)
    p.add_argument(
        "--tasks",
        nargs="+",
        default=None,
        help="explicit task ids, in this order (default: priority order of all MB tasks)",
    )
    p.add_argument("--only-task", default=None)
    p.add_argument(
        "--phase1-runs",
        type=int,
        default=3,
        help="runs per task in phase 1 (task blocks of interleaved pairs)",
    )
    p.add_argument(
        "--phase2-runs",
        type=int,
        default=2,
        help="extra runs per task in phase 2, only after phase 1 is complete for all tasks and quota allows",
    )
    p.add_argument(
        "--cutoff-utc", default=None, help="ISO time; no trial starts after it (STOPPED_TIME)"
    )
    p.add_argument("--schedule-seed", type=int, default=20261005)
    p.add_argument("--model", default="claude-sonnet-5-5")
    p.add_argument(
        "--effort", default=None, choices=[None, "low", "medium", "high", "xhigh", "max"]
    )
    p.add_argument("--max-turns", type=int, default=None, help="override every task's max_turns")
    p.add_argument(
        "--timeout-s", type=int, default=None, help="override every task's wall-time limit"
    )
    p.add_argument(
        "--max-budget-usd", type=float, default=6.0, help="per-trial runaway guard passed to claude"
    )
    p.add_argument(
        "--budget-cap-usd",
        type=float,
        default=None,
        help="optional total cap on the ledger (default: none, subscription)",
    )
    p.add_argument("--seven-day-stop", type=float, default=core.SEVEN_DAY_STOP)
    p.add_argument(
        "--tool-search",
        choices=["default", "off"],
        default="default",
        help="default: ToolSearch is in the built-in tool set so Claude Code's own deferral applies per arm",
    )
    p.add_argument("--tell-budget", action=argparse.BooleanOptionalAction, default=True)
    p.add_argument(
        "--build-dir",
        type=Path,
        default=None,
        help="dir with BenchLab.app and BenchSentinel.app (swift/build.sh)",
    )
    p.add_argument("--no-sentinel", action="store_true")
    p.add_argument("--no-recorder", action="store_true")
    p.add_argument(
        "--codex-cu-mcp",
        type=Path,
        default=None,
        help="MCP config for arm cc-codex-cu (default WORK/codex-access/mcp.json)",
    )
    p.add_argument(
        "--idle-min",
        type=float,
        default=0.0,
        help="warn (and record) when HID idle is below this; never blocks",
    )
    p.add_argument(
        "--smoke", action="store_true", help="mark rows smoke:true (excluded from analysis)"
    )
    p.add_argument(
        "--claude-bin",
        type=Path,
        default=None,
        help="testing only: a stand-in for the claude binary",
    )
    p.add_argument("--allow-non-terminal", action="store_true")
    p.add_argument("--allow-codex-arms", action="store_true")
    p.add_argument(
        "--no-model-preflight",
        action="store_true",
        help="skip the three small Haiku calls (quota probe, init checks)",
    )
    p.add_argument(
        "--offline",
        action="store_true",
        help="preflight only: static checks, no daemon, no recording, no GUI, no model call",
    )
    p.add_argument(
        "--allow-unknown-quota",
        action="store_true",
        help="start even when no quota reading exists (default: refuse)",
    )
    p.add_argument("--ignore-hold", action="store_true", help="start although WORK/HOLD exists")
    return p


def make_ctx(args: argparse.Namespace) -> Ctx:
    run_id = args.run_id or datetime.now(timezone.utc).strftime("run-%Y%m%dT%H%M%SZ")
    run_dir = (args.runs_dir / run_id).resolve()
    if args.command == "dry-run":
        run_dir = Path(tempfile.mkdtemp(prefix="cdb-dry-"))
    run_dir.mkdir(parents=True, exist_ok=True)
    (run_dir / "trials").mkdir(exist_ok=True)
    tasks = load_tasks(HERE / "probes")
    selected = args.tasks or (
        [args.only_task] if args.only_task else [t for t in tasks if default_task(tasks, t)]
    )
    if args.only_task and args.tasks is None:
        selected = [args.only_task]
    unknown = [t for t in selected if t not in tasks]
    if unknown:
        raise SystemExit(f"unknown tasks {unknown}; known {sorted(tasks)}")
    task_ids = core.order_tasks(selected, explicit=bool(args.tasks))
    full_ids = (
        task_ids if args.tasks else core.order_tasks([t for t in tasks if default_task(tasks, t)])
    )
    arm_names = [args.only_arm] if args.only_arm else list(args.arms)
    codex = [a for a in arm_names if a in arms.CODEX_FALLBACK_ARMS]
    if codex and not args.allow_codex_arms:
        raise SystemExit(f"arms {codex} are fallback arms; pass --allow-codex-arms")
    if len(arm_names) != 2 and args.command == "run":
        print(f"note: running {len(arm_names)} arm(s); the pairing rule assumes 2", file=sys.stderr)
    if args.claude_bin is not None:
        ca.CLAUDE_BIN = args.claude_bin
    ctx = Ctx(
        args, run_dir, core.Ledger(args.ledger), tasks, task_ids, arm_names, full_task_ids=full_ids
    )
    ctx.cutoff = core.parse_utc(args.cutoff_utc) if args.cutoff_utc else None
    ctx.versions = {
        "claude": claude_version(),
        "macos": sw_vers(),
        "bench_repo_commit": git_commit(Path.home() / "repo/cua-driver-bench-codex-vs-cua"),
        "bench_src_sha256": src_hash(),
    }
    return ctx


def setup_arms(ctx: Ctx) -> None:
    pins = ca.load_pins()
    observed = ca.observed_pins(
        include_codex="cc-codex-cu" in ctx.arm_names, include_cua="cc-cua-driver" in ctx.arm_names
    )
    ctx.versions.update(
        pins_expected=pins,
        pins_observed=observed,
        macos_build=observed.get("macos_build"),
        chatgpt_app_version=observed.get("chatgpt_app_version"),
        unified_computer_use_plugin_version=observed.get("unified_computer_use_plugin_version"),
        cua_repl_package_version=observed.get("cua_repl_package_version"),
        codex_computer_use_service_version=observed.get("codex_computer_use_service_version"),
        cua_driver_version=ca.cua_version_string(),
        cua_driver_sha256=ca.sha256_file(ca.CUA_BIN) if ca.CUA_BIN.is_file() else None,
        cua_skills_tree_sha256=ca.sha256_tree(ca.CUA_SKILLS) if ca.CUA_SKILLS.is_dir() else None,
        pins=pins,
        system_prompt_sha256=hashlib.sha256(ca.SYSTEM_PROMPT.encode()).hexdigest(),
    )
    for arm in ctx.arm_names:
        if arm in arms.CLAUDE_ARMS:
            try:
                ctx.mcp[arm] = ca.mcp_config_for(arm, ctx.run_dir, ctx.args.codex_cu_mcp)
            except (FileNotFoundError, ValueError) as error:
                raise SystemExit(f"arm {arm} unavailable: {error}")
    if ctx.args.build_dir:
        apps: dict[str, str] = {}
        for name in BENCH_APPS:
            macos = Path(ctx.args.build_dir) / name / "Contents/MacOS"
            exes = sorted(macos.iterdir()) if macos.is_dir() else []
            if exes:
                apps[name] = ca.sha256_file(exes[0])
        ctx.versions["bench_apps_exe_sha256"] = apps
    if "cc-codex-cu" in ctx.mcp:
        ctx.versions["codex_cu_mcp_sha256"] = ca.sha256_file(ctx.mcp["cc-codex-cu"][0])


def start_recorder(ctx: Ctx) -> None:
    if ctx.args.no_recorder:
        ctx.recorder = rec.NullRecorder()
        return
    ctx.rec_daemon = ca.start_cua_daemon(ca.RECORDER_SOCKET, ca.RECORDER_STATE, overlay=False)
    ctx.recorder = rec.CuaRecorder(
        ca.CUA_BIN, ca.RECORDER_SOCKET, ca.cua_env(ca.RECORDER_STATE / "home")
    )
    ctx.compress = rec.CompressQueue()


def shutdown(ctx: Ctx) -> None:
    if ctx.compress:
        ctx.log(f"waiting for {ctx.compress.pending()} compression job(s)")
        ctx.compress.join()
        ctx.compress.close()
    if ctx.recorder:
        ctx.recorder.close()
    for proc, sock in ((ctx.rec_daemon, ca.RECORDER_SOCKET), (ctx.daemon, ca.AGENT_SOCKET)):
        if proc is not None:
            claude_driver.kill_group(proc.pid)
            try:
                os.unlink(sock)
            except OSError:
                pass


def finalize(ctx: Ctx) -> dict[str, Any]:
    rows = core.load_rows(ctx.results_path)
    blocks = ctx.blocks()
    status = core.block_status(rows, blocks)
    rows = core.flag_incomplete(rows, blocks)
    tmp = ctx.results_path.with_suffix(".jsonl.tmp")
    tmp.write_text("".join(json.dumps(r) + "\n" for r in rows), "utf-8")
    tmp.replace(ctx.results_path)
    summary = {
        "blocks": status,
        "blocks_complete": sum(1 for v in status.values() if v["complete"]),
        "blocks_total": len(status),
        "trials": len([r for r in rows if r.get("final", True)]),
    }
    (ctx.run_dir / "blocks.json").write_text(json.dumps(summary, indent=2) + "\n", "utf-8")
    return summary


def write_manifest(ctx: Ctx, first_schedule: list[dict[str, Any]]) -> None:
    args = ctx.args
    manifest = {
        "created_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "run_id": ctx.run_dir.name,
        "argv": sys.argv,
        "arms": ctx.arm_names,
        "tasks": ctx.task_ids,
        "schedule_rule": (
            "task-major blocks in priority order (MB-01.. then others); phase 1 = runs 1.."
            f"{args.phase1_runs} per task, phase 2 = next {args.phase2_runs} runs per task only after phase 1 is complete "
            "for all tasks and quota allows; each run is a pair of back-to-back trials, first arm = arms[0] if "
            "(task_pos + run_index) even else arms[1], 0-based; both arms share the run's seed"
        ),
        "schedule_seed_recorded": args.schedule_seed,
        "phases": {"phase1_runs": args.phase1_runs, "phase2_runs": args.phase2_runs},
        "cutoff_utc": args.cutoff_utc,
        "model": args.model,
        "effort": args.effort,
        "max_budget_usd_per_trial": args.max_budget_usd,
        "budget_cap_usd": args.budget_cap_usd,
        "seven_day_stop": args.seven_day_stop,
        "tool_search": args.tool_search,
        "system_prompt": ca.SYSTEM_PROMPT,
        "builtin_tools": ca.builtin_tools(args.tool_search == "default"),
        "claude_env_allowlist": list(ca.CLAUDE_ENV_ALLOW),
        "claude_env_fixed": ca.CLAUDE_ENV_FIXED,
        "versions": ctx.versions,
        "recorder": getattr(ctx.recorder, "name", "none"),
        "first_block_schedule": first_schedule,
        "arm_descriptions": {a: ca.ARM_DESCRIPTIONS.get(a, a) for a in ctx.arm_names},
        "price_fallback_usd_per_mtok_ASSUMED": PRICE_FALLBACK,
        "example_argv": {
            a: ca.claude_argv(
                mcp_config=ctx.mcp[a][0],
                server=ctx.mcp[a][1],
                model=args.model,
                max_turns=args.max_turns or 30,
                max_budget_usd=args.max_budget_usd,
                tool_search=args.tool_search == "default",
                effort=args.effort,
            )
            for a in ctx.arm_names
            if a in ctx.mcp
        },
    }
    (ctx.run_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", "utf-8")


def run_loop(ctx: Ctx) -> str:
    args = ctx.args
    rows = core.load_rows(ctx.results_path)
    done = set() if args.smoke else core.completed_trial_ids(rows)
    blocks = ctx.blocks()
    ctx.set_state(blocks_total=len(blocks))
    reason = "DONE"
    complete = 0
    try:
        for block in blocks:
            if block["phase"] == 2:
                # Phase 2 only when phase 1 is complete for every task, quota allows and the block fits.
                final_ids = done | {
                    e["trial_id"] for b in blocks for e in b["entries"] if e["trial_id"] in done
                }
                gate = core.quota_gate(ctx.quota, args.seven_day_stop)
                if not core.phase2_allowed(blocks, final_ids, gate["action"] == "go"):
                    raise StopRun(
                        "QUOTA" if gate["action"] != "go" else "INCOMPLETE_PHASE1", gate["reason"]
                    )
                est = None
                if ctx.block_durations:
                    est = sorted(ctx.block_durations)[len(ctx.block_durations) // 2] * (
                        len(block["entries"]) / 6.0
                    )
                if not core.block_fits(ctx.cutoff, est):
                    raise StopRun(
                        "TIME",
                        f"phase-2 block {block['id']} does not fit before the cutoff (estimate {est})",
                    )
            todo = [e for e in block["entries"] if e["trial_id"] not in done]
            if not todo:
                complete += 1
                continue
            started = time.monotonic()
            ctx.set_state(phase=block["phase"], block=block["id"])
            ctx.log(f"block {block['id']} start ({len(todo)} of {len(block['entries'])} trials)")
            for entry in todo:
                row = execute_trial(ctx, entry)
                if row.get("final"):
                    done.add(entry["trial_id"])
            complete += 1
            ctx.block_durations.append(time.monotonic() - started)
            ctx.set_state(blocks_complete=complete)
            ctx.log(f"block {block['id']} complete in {ctx.block_durations[-1]:.0f}s")
    except StopRun as stop:
        reason = stop.reason
        ctx.log(f"STOPPED_{stop.reason}: {stop.detail}")
        (ctx.run_dir / f"STOPPED_{stop.reason}").write_text(
            f"{stop.detail}\n{datetime.now(timezone.utc).isoformat()}\n", "utf-8"
        )
    except KeyboardInterrupt:
        reason = "USER"
        ctx.abort.set()
        ctx.log("interrupted")
        (ctx.run_dir / "STOPPED_USER").write_text("KeyboardInterrupt\n", "utf-8")
    return reason


def cmd_run(args: argparse.Namespace) -> int:
    gate = cmd_gate_check(args)
    if gate and args.claude_bin is None:  # only a stand-in claude binary may bypass the gates
        return gate
    ctx = make_ctx(args)
    setup_arms(ctx)
    ctx.log(f"run {ctx.run_dir.name}: arms={ctx.arm_names} tasks={ctx.task_ids}")
    for flag in (
        "DONE",
        "STOPPED_TIME",
        "STOPPED_BUDGET",
        "STOPPED_QUOTA",
        "STOPPED_USER",
        "STOPPED_DISK",
        "STOPPED_AUTH",
        "STOPPED_INCOMPLETE_PHASE1",
    ):
        (ctx.run_dir / flag).unlink(missing_ok=True)
    hb_stop = start_heartbeat(ctx)
    exit_code = 0

    def on_signal(signum: int, _frame: Any) -> None:
        ctx.abort.set()
        raise KeyboardInterrupt(f"signal {signum}")

    for sig in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(sig, on_signal)
    try:
        removed = unregister_stale_copies(args.build_dir)
        if removed:
            ctx.log(f"unregistered stale LaunchServices copies: {removed}")
        # The runner owns both private daemons for the run: replace any leftover ones.
        ca.stop_cua_daemon(ca.AGENT_SOCKET, ca.CUA_STATE / "home")
        ca.stop_cua_daemon(ca.RECORDER_SOCKET, ca.RECORDER_STATE / "home")
        if "cc-cua-driver" in ctx.arm_names:
            ctx.daemon = ca.start_cua_daemon(ca.AGENT_SOCKET, ca.CUA_STATE)
        start_recorder(ctx)
        checks = preflight(ctx, with_models=not args.no_model_preflight)
        (ctx.run_dir / "preflight.json").write_text(json.dumps(checks, indent=2) + "\n", "utf-8")
        failed = [c for c in checks if c[1] == "fail"]
        known = ctx.quota is not None and ctx.quota.get("seven_day") is not None
        if not known and not args.allow_unknown_quota and args.claude_bin is None:
            failed.append(
                (
                    "quota reading",
                    "fail",
                    "no seven-day utilisation known; refusing to start (--allow-unknown-quota overrides)",
                )
            )
        if failed:
            ctx.log(f"preflight failed: {[c[0] for c in failed]}")
            (ctx.run_dir / "PREFLIGHT_FAILED").write_text(
                json.dumps(failed, indent=2) + "\n", "utf-8"
            )
            return 2
        write_manifest(ctx, core.flat_entries(ctx.blocks())[:6])
        reason = run_loop(ctx)
        summary = finalize(ctx)
        ctx.log(
            f"finished: {reason}; blocks complete {summary['blocks_complete']}/{summary['blocks_total']}"
        )
        (ctx.run_dir / ("DONE" if reason == "DONE" else f"STOPPED_{reason}")).write_text(
            json.dumps({"reason": reason, **summary}, indent=2) + "\n", "utf-8"
        )
        (ctx.run_dir / "done.json").write_text(
            json.dumps(
                {
                    "reason": reason,
                    "ts": datetime.now(timezone.utc).isoformat(timespec="seconds"),
                    **summary,
                },
                indent=2,
            )
            + "\n",
            "utf-8",
        )
    except Exception as error:  # noqa: BLE001
        ctx.log(f"runner crashed: {type(error).__name__}: {error}")
        (ctx.run_dir / "done.json").write_text(
            json.dumps({"reason": "CRASH", "error": str(error)}) + "\n", "utf-8"
        )
        exit_code = 1
    finally:
        ctx.set_state(state="stopped")
        hb_stop.set()
        shutdown(ctx)
    return exit_code


def cmd_gate_check(args: argparse.Namespace) -> int:
    """No model call: HOLD file and the latest known seven-day reading. Exit 3 gated, 4 held, else 0."""
    hold = WORK / "HOLD"
    if hold.exists() and not args.ignore_hold:
        print(f"HELD: {hold} exists: {hold.read_text('utf-8').strip()[:300]}")
        return 4
    latest = core.latest_known_quota(QUOTA_LATEST, args.ledger)
    decision = core.gate_decision(latest, args.seven_day_stop)
    print(f"quota gate: {decision['action'].upper()}: {decision['reason']}")
    if latest:
        print(
            f"latest known: {json.dumps({k: latest.get(k) for k in ('ts', 'five_hour', 'seven_day', 'status', 'source')})}"
        )
    if decision["action"] == "gated":
        print("RUN GATED: not starting. The seven-day figure only grows until its window resets.")
        return 3
    return 0


def cmd_preflight(args: argparse.Namespace) -> int:
    args.run_id = args.run_id or "preflight"
    ctx = make_ctx(args)
    setup_arms(ctx)
    try:
        if not args.offline:
            start_recorder(ctx)
        checks = preflight(ctx, with_models=not args.no_model_preflight)
        (ctx.run_dir / "preflight.json").write_text(json.dumps(checks, indent=2) + "\n", "utf-8")
        write_manifest(
            ctx, core.flat_entries(ctx.blocks())[:6]
        )  # shows every pin the run would record
    finally:
        shutdown(ctx)
    failed = [c for c in checks if c[1] == "fail"]
    print(f"\npreflight: {len(checks) - len(failed)} ok/warn, {len(failed)} FAILED")
    return 2 if failed else 0


def cmd_dry_run(args: argparse.Namespace) -> int:
    ctx = make_ctx(args)
    schedule = core.flat_entries(ctx.blocks())
    for entry in schedule:
        task = ctx.tasks[entry["task"]]
        spec = task_spec(task)
        print(
            json.dumps(
                {
                    **entry,
                    "seed": core.probe_seed(entry["task"], entry["run_index"]),
                    "timeout_s": args.timeout_s or spec.get("timeout_s"),
                    "max_turns": args.max_turns or spec.get("max_turns", 30),
                }
            )
        )
    print(
        f"{len(schedule)} trials in {len(ctx.blocks())} block(s); tasks={ctx.task_ids}; arms={ctx.arm_names}; "
        f"phase1_runs={args.phase1_runs} phase2_runs={args.phase2_runs}"
    )
    for arm in ctx.arm_names:
        if arm in arms.CLAUDE_ARMS:
            try:
                path, server = ca.mcp_config_for(arm, ctx.run_dir, args.codex_cu_mcp)
                argv = ca.claude_argv(
                    mcp_config=path,
                    server=server,
                    model=args.model,
                    max_turns=args.max_turns or 30,
                    max_budget_usd=args.max_budget_usd,
                    tool_search=args.tool_search == "default",
                    effort=args.effort,
                )
                print(f"\n{arm}:\n  " + " ".join(json.dumps(a) if " " in a else a for a in argv))
            except (FileNotFoundError, ValueError) as error:
                print(f"\n{arm}: UNAVAILABLE ({error})")
    return 0


def cmd_status(args: argparse.Namespace) -> int:
    run_dir = (args.runs_dir / (args.run_id or "")).resolve()
    beat = run_dir / "heartbeat.json"
    if beat.exists():
        print(beat.read_text("utf-8"))
    rows = core.load_rows(run_dir / "results.jsonl")
    print(f"{len(rows)} rows; passed {sum(1 for r in rows if r.get('passed'))}")
    for flag in sorted(run_dir.glob("STOPPED_*")) + sorted(run_dir.glob("DONE")):
        print("flag:", flag.name)
    return 0


def cmd_finalize(args: argparse.Namespace) -> int:
    ctx = make_ctx(args)
    print(json.dumps(finalize(ctx), indent=2))
    return 0


def main() -> int:
    args = build_parser().parse_args()
    if args.command == "gate-check":
        return cmd_gate_check(args)
    if args.command == "preflight":
        return cmd_preflight(args)
    if args.command == "dry-run":
        return cmd_dry_run(args)
    if args.command == "status":
        return cmd_status(args)
    if args.command == "finalize":
        return cmd_finalize(args)
    return cmd_run(args)


if __name__ == "__main__":
    raise SystemExit(main())
