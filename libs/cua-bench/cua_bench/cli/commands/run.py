"""``cb run``: run tasks locally or in the cloud, and inspect runs.

Usage:
    cb run <task|dataset> [--on local|cloud] [--kind container|vm] [--runtime <engine>] ...
    cb run task <path>          # one task (one variant, --variant-id)
    cb run dataset <path|name>  # every task and variant, in parallel
    cb run list | info <id> | watch <id> | logs <id> | stop <id>

Both targets use the same runner: each variant gets a cua-sandbox sandbox
(local gVisor container / QEMU / Lume, or a claim on a managed Fleet pool),
the task's setup, the oracle or agent, and evaluate() run in this process,
and results land in ``$XDG_DATA_HOME/cua-bench/runs/<run id>/``.
"""

from __future__ import annotations

import asyncio
import fnmatch
import json
import os
import signal
import subprocess
import sys
import time
import uuid
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any, Optional

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"

FINAL = ("completed", "failed", "cancelled", "stopped")


def _get_runs_dir() -> Path:
    """Default runs output directory (XDG compliant)."""
    xdg_data = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return Path(xdg_data) / "cua-bench" / "runs"


def _get_run_output_dir(run_id: str) -> Path:
    return _get_runs_dir() / run_id


def generate_run_id() -> str:
    return uuid.uuid4().hex[:8]


# =============================================================================
# Target flags (shared by run task / run dataset / interact)
# =============================================================================


def add_target_args(parser) -> None:
    """``--on``, ``--kind``, ``--runtime``, image and resource flags."""
    import argparse

    from cua_bench.targets import LEGACY_PLATFORMS, ON_CHOICES

    group = parser.add_argument_group("execution target")
    group.add_argument(
        "--on",
        choices=list(ON_CHOICES),
        default=None,
        help="Where sandboxes run: local (this machine), cloud, your own cloud account "
        "(aws, gcp, modal; connect it first with `cua cloud connect <provider>`), or a "
        "contrib provider (e2b, daytona; needs a cua SDK built with contrib and the provider's key). "
        "Default: CUA_DEFAULT_ON, else `cua config set default.on`, else local",
    )
    group.add_argument(
        "--kind",
        choices=["auto", "container", "vm"],
        default=None,
        help="What kind of sandbox: container or vm. Default auto: Linux as a container "
        "unless the task or image says VM; Windows, macOS and Android are VM-only. "
        "Default: CUA_DEFAULT_KIND, else `cua config set default.kind`",
    )
    group.add_argument(
        "--runtime",
        choices=["auto", "gvisor", "runc", "qemu", "lume", "kubevirt"],
        default=None,
        help="Which engine: gvisor or runc (local containers), qemu or lume (local VMs), "
        "gvisor or kubevirt (cloud containers, VMs). Implies its kind. Default auto: the "
        "SDK picks (CUA_DEFAULT_RUNTIME, else `cua config set default.runtime`)",
    )
    group.add_argument(
        "--image",
        help="Image for every task (overrides the task's setup_config.image): an OS alias "
        "(linux, windows, macos:tahoe), ghcr.io/trycua/<os>, any registry ref or a digest, "
        "or pool:<name> for an existing Fleet pool. Env: CUA_BENCH_IMAGE",
    )
    group.add_argument("--cpu", type=int, help="vCPUs per sandbox")
    group.add_argument("--memory", help="Memory per sandbox (e.g. 4096, 4G)")
    group.add_argument(
        "--warm",
        action="store_true",
        help="cloud: keep one replica ready when the managed pool is first created",
    )
    group.add_argument(
        "--claim-ttl",
        dest="claim_ttl",
        help="cloud: how long a claim outlives a crashed run (default 15m; renewed while running)",
    )
    # cua-bench 0.2.11 flags, deprecated (a notice names the new flags).
    group.add_argument(
        "--platform",
        dest="platform",
        choices=list(LEGACY_PLATFORMS),
        default=None,
        help=argparse.SUPPRESS,
    )
    group.add_argument(
        "--provider-type",
        dest="provider_type",
        default=None,
        help=argparse.SUPPRESS,
    )


def _check_provider_type(value) -> None:
    """``--provider-type`` (0.2.11): ``native`` is the only provider now (a
    deprecated no-op); ``simulated`` was removed."""
    if value is None:
        return
    import warnings

    from cua_bench.targets import TargetError

    word = str(value).strip().lower()
    if word in ("native", "computer"):
        message = "--provider-type is deprecated and has no effect: every task runs natively"
        warnings.warn(message, DeprecationWarning, stacklevel=3)
        print(f"warning: {message}", file=sys.stderr)
        return
    if word in ("simulated", "webtop"):
        raise TargetError(
            f"--provider-type {word}: the simulated provider was removed in cua-bench 0.3; "
            "tasks run in a real sandbox (drop --provider-type)"
        )
    raise TargetError(f"--provider-type {value!r} is not supported (deprecated; drop it)")


def target_from_args(args):
    from cua_bench.targets import resolve_target

    _check_provider_type(getattr(args, "provider_type", None))
    return resolve_target(
        getattr(args, "on", None),
        getattr(args, "kind", None),
        runtime=getattr(args, "runtime", None),
        platform=getattr(args, "platform", None),
        image=getattr(args, "image", None),
        cpu=getattr(args, "cpu", None),
        memory=getattr(args, "memory", None),
        concurrency=getattr(args, "max_parallel", None) or 1,
        warm=getattr(args, "warm", False),
        claim_ttl=getattr(args, "claim_ttl", None),
    )


# =============================================================================
# Console progress
# =============================================================================


class ConsoleReporter:
    """Plan, per-variant status lines and pool scaling notices."""

    WAIT_EVERY_S = 30.0

    def __init__(self, stream=None, log_file: Optional[Path] = None) -> None:
        self.stream = stream or sys.stdout
        self.log_file = log_file
        self._cold_pools: set[str] = set()
        self._last_wait: dict[str, float] = {}
        self._color = bool(getattr(self.stream, "isatty", lambda: False)())

    def _c(self, color: str, text: str) -> str:
        return f"{color}{text}{RESET}" if self._color else text

    def line(self, text: str) -> None:
        self.stream.write(text + "\n")
        self.stream.flush()
        if self.log_file is not None:
            import re

            try:
                with open(self.log_file, "a", encoding="utf-8") as handle:
                    handle.write(re.sub(r"\033\[[0-9;]*m", "", text) + "\n")
            except OSError:
                pass

    def plan(self, target, jobs, plans) -> None:
        sandboxed = sum(1 for job in jobs if job.spec.needs_sandbox)
        backends = sorted({job.spec.backend(target.on) for job in jobs})
        self.line(
            f"{len(jobs)} variant(s) on {self._c(BOLD, target.on)} "
            f"({', '.join(backends)}), up to {target.concurrency} at a time"
        )
        for plan in plans:
            where = "managed pool" if target.cloud else "local sandboxes"
            self.line(
                f"  {plan.spec.image_label}  {plan.tasks} variant(s), {where} "
                f"max {plan.max_pool_size}"
            )
        if target.cloud and sandboxed:
            self.line(
                self._c(
                    GREY,
                    "  Managed pools autoscale from zero: an idle pool takes about a minute "
                    "(VMs several) to start its first sandbox, and scales back down after "
                    "the run.",
                )
            )

    def started(self, job, index, total) -> None:
        what = "acquiring sandbox" if job.spec.needs_sandbox else "no environment (dataset)"
        self.line(self._c(GREY, f"[{index}/{total}] ▸ {job.name}: {what}"))

    def progress(self, job, event) -> None:
        if event.stage == "cold_start":
            key = event.pool or "pool"
            if key not in self._cold_pools:
                self._cold_pools.add(key)
                self.line(
                    self._c(
                        YELLOW, f"  pool {key} is scaling up from zero (about 1 min, VMs longer)"
                    )
                )
        elif event.stage == "ready":
            elapsed = f" ({event.elapsed:.0f}s)" if event.elapsed else ""
            self.line(self._c(GREY, f"  ▸ {job.name}: sandbox ready{elapsed}"))
        elif event.stage == "waiting":
            now = time.monotonic()
            if now - self._last_wait.get(job.session_id, 0.0) >= self.WAIT_EVERY_S:
                self._last_wait[job.session_id] = now
                self.line(self._c(GREY, f"  ▸ {job.name}: {event.message}"))
        elif event.stage == "pool" and event.message:
            self.line(self._c(GREY, f"  {event.message}"))

    def finished(self, result, index, total) -> None:
        reward = "-" if result.reward is None else f"{result.reward:g}"
        head = f"[{index}/{total}]"
        name = f"{result.task} v{result.variant}"
        if result.ok:
            self.line(
                self._c(GREEN, f"{head} ✓ {name}") + f" reward={reward} ({result.duration_s:.0f}s)"
            )
        else:
            first = (result.error or result.status).splitlines()[0]
            self.line(self._c(RED, f"{head} ✗ {name}") + f" {result.status}: {first}")


# =============================================================================
# Building jobs
# =============================================================================


def _discover_tasks(path: Path) -> list[Path]:
    if (path / "main.py").exists():
        return [path]
    tasks = [d for d in sorted(path.iterdir()) if d.is_dir() and (d / "main.py").exists()]
    if not tasks and any((d / "task.toml").exists() for d in path.iterdir() if d.is_dir()):
        raise FileNotFoundError(
            f"{path} holds Harbor tasks (task.toml); cua-bench runs main.py tasks, and the "
            "Harbor task adapter is not part of this release"
        )
    return tasks


def _task_variants(task_path: Path) -> list[Any]:
    from cua_bench import make

    env = make(str(task_path))
    if env.tasks_config_fn is None:
        return [None]
    return list(env.tasks_config_fn()) or [None]


def build_jobs(
    args, run_id: str, output_dir: Path, target, kind: str, variant_resolver=None
) -> list:
    """Jobs for ``cb run task`` (one variant) or ``cb run dataset`` (all).

    ``variant_resolver(ref, os_type)`` reads an image's variant index for
    ``--kind auto`` (default: the SDK resolver, once per image).
    """
    from cua_bench.runner import Job
    from cua_bench.sandboxes import cached_index_runtime
    from cua_bench.targets import resolve_env_spec

    if variant_resolver is None:
        variant_resolver = cached_index_runtime()

    from .registry import resolve_dataset

    #: The registry entry's desktop image for tasks that name none.
    default_image = None
    if kind == "task":
        task_path = Path(args.task_path)
        if not (task_path / "main.py").exists():
            raise FileNotFoundError(f"Task not found (no main.py): {task_path}")
        variants = _task_variants(task_path)
        index = getattr(args, "variant_id", 0) or 0
        if index >= len(variants):
            raise ValueError(f"--variant-id {index} out of range ({len(variants)} variants)")
        selected = [(task_path, index, variants[index])]
    else:
        dataset_path = Path(args.dataset_path)
        if not dataset_path.exists():
            resolved, default_image = resolve_dataset(args.dataset_path)
            if not resolved:
                raise FileNotFoundError(f"Dataset not found: {args.dataset_path}")
            dataset_path = resolved
        tasks = _discover_tasks(dataset_path)
        task_filter = getattr(args, "task_filter", None)
        if task_filter:
            patterns = [p.strip() for p in task_filter.split(",")]
            tasks = [t for t in tasks if any(fnmatch.fnmatch(t.name, p) for p in patterns)]
        if not tasks:
            raise FileNotFoundError(f"No tasks found in {dataset_path}")
        max_variants = getattr(args, "max_variants", None)
        selected = []
        for task_path in tasks:
            variants = _task_variants(task_path)
            if max_variants:
                variants = variants[:max_variants]
            selected += [(task_path, i, cfg) for i, cfg in enumerate(variants)]

    attempts = int(getattr(args, "attempts", 1) or 1)
    if attempts < 1:
        raise ValueError("--attempts must be at least 1")
    if int(getattr(args, "retries", 0) or 0) < 0:
        raise ValueError("--retries must be 0 or more")
    jobs = []
    for task_path, variant, cfg in selected:
        spec = resolve_env_spec(
            getattr(cfg, "computer", None),
            target,
            variant_resolver=variant_resolver,
            default_image=default_image,
        )
        for attempt in range(attempts):
            # Attempt 0 keeps the single-attempt layout and ids.
            suffix = f"_a{attempt}" if attempt else ""
            name = f"{task_path.name}_v{variant}{suffix}"
            session_id = f"task-{run_id}-{task_path.name}-v{variant}" + (
                f"-a{attempt}" if attempt else ""
            )
            if kind == "task" and getattr(args, "session_id", None) and not attempt:
                session_id = args.session_id
            jobs.append(
                Job(
                    task_path=task_path.resolve(),
                    variant=variant,
                    spec=spec,
                    session_id=session_id,
                    output_dir=output_dir / name,
                    attempt=attempt,
                )
            )
    from cua_bench.targets import check_requirements

    # Adapters declare what they need (KVM for local VMs, API keys ...):
    # fail before any sandbox is claimed.
    check_requirements((job.spec for job in jobs), target)
    return jobs


def agent_options(args):
    from cua_bench.runner import AgentOptions

    agent_name = getattr(args, "agent", None)
    import_path = getattr(args, "agent_import_path", None)
    config_loader = getattr(args, "_config_loader", None)
    if agent_name and config_loader:
        entry = config_loader.get_agent_by_name(agent_name)
        if entry is not None:
            if entry.is_docker_agent():
                raise ValueError(
                    f"agent {agent_name!r} is a Docker image agent; cb run executes agents "
                    "in-process now. Give it an import_path in .cua/agents.yaml instead."
                )
            if entry.import_path:
                import_path = entry.import_path
    if getattr(args, "noop", False):
        if agent_name or import_path or getattr(args, "oracle", False):
            raise ValueError("--noop runs no solver: drop --agent/--agent-import-path/--oracle")
        return AgentOptions(oracle=False, dump=True, max_steps=getattr(args, "max_steps", None))
    oracle = bool(getattr(args, "oracle", False)) or not (agent_name or import_path)
    return AgentOptions(
        oracle=oracle,
        agent=None if oracle else agent_name,
        agent_import_path=None if oracle else import_path,
        model=getattr(args, "model", None),
        max_steps=getattr(args, "max_steps", None),
    )


# =============================================================================
# cb run task / cb run dataset
# =============================================================================


def _load_dotenv() -> None:
    from dotenv import load_dotenv

    env_file = Path.cwd() / ".env"
    if env_file.exists():
        load_dotenv(env_file)
        print(f"{GREY}Loaded environment from: {env_file}{RESET}")


def cmd_run_task(args) -> int:
    return _cmd_execute(args, "task")


def cmd_run_dataset(args) -> int:
    return _cmd_execute(args, "dataset")


def _cmd_execute(args, kind: str) -> int:
    from cua_bench.sandboxes import CloudAuthError, cloud_auth_source
    from cua_bench.sessions import manager
    from cua_bench.targets import TargetError

    _load_dotenv()
    args = _apply_config_defaults_for_task(args)

    if getattr(args, "dev_paths", None):
        print(
            f"{GREY}--with is not needed anymore: tasks and agents run in this Python "
            f"environment (pip install -e the package instead).{RESET}"
        )

    run_id = getattr(args, "run_id", None) or generate_run_id()
    output_dir = Path(getattr(args, "output_dir", None) or _get_run_output_dir(run_id))

    try:
        target = target_from_args(args)
        opts = agent_options(args)
        jobs = build_jobs(args, run_id, output_dir, target, kind)
    except (TargetError, FileNotFoundError, ValueError) as error:
        print(f"{RED}Error: {error}{RESET}")
        return 1

    if getattr(args, "dry_run", False):
        return _print_dry_run(target, jobs)

    auth = None
    if target.cloud and any(job.spec.needs_sandbox for job in jobs):
        try:
            auth = cloud_auth_source()
        except CloudAuthError as error:
            print(f"{RED}{error}{RESET}")
            return 1

    agent_display = opts.label
    output_dir.mkdir(parents=True, exist_ok=True)
    for job in jobs:
        manager.add_session(
            {
                "session_id": job.session_id,
                "run_id": run_id,
                "location": target.on,
                "env_path": str(job.task_path),
                "task_index": job.variant,
                "kind": job.spec.kind,
                "runtime": job.spec.runtime,
                "image_variant": job.spec.image_variant,
                "backend": job.spec.backend(target.on),
                "image": job.spec.image_label,
                "agent": agent_display,
                "model": getattr(args, "model", None) or "-",
                "output_dir": str(job.output_dir),
                "status": "queued",
            }
        )

    print(f"\n  {BOLD}Run ID:{RESET}  {run_id}")
    print(f"  {BOLD}Output:{RESET}  {output_dir}")
    if auth is not None:
        print(f"  {BOLD}Auth:{RESET}    {auth}")

    if getattr(args, "detach", False):
        return _spawn_detached(args, run_id, output_dir, jobs)

    (output_dir / "run.pid").write_text(str(os.getpid()))
    reporter = ConsoleReporter(log_file=output_dir / "run.log")
    taskset = _telemetry_taskset(args, kind)
    try:
        results = asyncio.run(
            _run_batch(
                target, opts, jobs, reporter, run_id, retries=getattr(args, "retries", 0) or 0
            )
        )
    except KeyboardInterrupt:
        _track_run_completed(taskset, None, len(jobs), "cancelled")
        print(
            f"\n{YELLOW}Interrupted: sandboxes released (cloud claims expire on their TTL "
            f"if a release did not finish).{RESET}"
        )
        return 130
    except BaseException:
        _track_run_completed(taskset, None, len(jobs), "error")
        raise
    finally:
        try:
            (output_dir / "run.pid").unlink()
        except OSError:
            pass
    code = _summarize(results, output_dir, kind, args)
    _track_run_completed(taskset, _aggregate_score(results), len(results), "ok")
    _results_housekeeping(args, output_dir)
    return code


def _telemetry_taskset(args, kind: str) -> Optional[str]:
    """The dataset name (or a task's parent directory name) for telemetry;
    cua_bench.telemetry maps anything not in the bundled registry to "custom"."""
    if kind == "dataset":
        return getattr(args, "dataset_path", None)
    task_path = getattr(args, "task_path", None)
    return Path(task_path).resolve().parent.name if task_path else None


def _aggregate_score(results) -> Optional[float]:
    """Mean reward in 0..1 over results that have one, else None."""
    rewards = [r.reward for r in results if getattr(r, "reward", None) is not None]
    if not rewards:
        return None
    try:
        return sum(float(x) for x in rewards) / len(rewards)
    except (TypeError, ValueError):
        return None


def _track_run_completed(taskset, score, task_count, outcome) -> None:
    try:
        from cua_bench.telemetry import track_bench_run_completed

        track_bench_run_completed(taskset, score, task_count, outcome)
    except Exception:
        pass


def _results_housekeeping(args, output_dir: Path) -> None:
    """Opt-in retention of old runs, then a warning when results grow large.

    Runs are only deleted when retention is configured (flags or the
    CUA_BENCH_KEEP_RUNS / CUA_BENCH_MAX_AGE_DAYS / CUA_BENCH_MAX_RESULTS_SIZE
    environment); the run that just finished is never deleted."""
    from cua_bench import retention

    # Retention applies to the runs directory; with --output-dir, to its
    # parent only when it holds nothing but runs (the default layout).
    runs_dir = _get_runs_dir()
    custom = getattr(args, "output_dir", None)
    try:
        policy = retention.Retention.from_env().merged(
            retention.Retention(
                keep_runs=getattr(args, "keep_runs", None),
                max_age_days=getattr(args, "max_age", None),
                max_bytes=(
                    retention.parse_size(args.max_results_size)
                    if getattr(args, "max_results_size", None)
                    else None
                ),
            )
        )
    except ValueError as error:
        print(f"{YELLOW}Retention not applied: {error}{RESET}")
        policy = retention.Retention()
    if policy.enabled:
        if custom:
            print(
                f"{GREY}Retention applies to {runs_dir}; results in {output_dir} "
                f"(--output-dir) are left alone.{RESET}"
            )
        else:
            removed = retention.apply(runs_dir, policy, protect=[output_dir])
            if removed:
                print(f"{GREY}Removed {len(removed)} old run(s) (retention).{RESET}")
    warning = retention.size_warning(Path(custom) if custom else runs_dir)
    if warning:
        print(f"{YELLOW}{warning}{RESET}")


async def _run_batch(target, opts, jobs, reporter, run_id, retries: int = 0):
    from cua_bench.runner import BatchRunner
    from cua_bench.runner.output import routed_stdio
    from cua_bench.sessions import manager

    def on_status(job, status, fields):
        update = {"status": status, **fields}
        if status in ("starting", "running"):
            update["pid"] = os.getpid()
        manager.update_session(job.session_id, update)

    loop = asyncio.get_running_loop()
    main = asyncio.current_task()
    try:
        loop.add_signal_handler(signal.SIGTERM, main.cancel)
    except (NotImplementedError, RuntimeError):
        pass

    with routed_stdio() as console:
        reporter.stream = console
        runner = BatchRunner(target, opts, reporter=reporter, on_status=on_status, retries=retries)
        try:
            return await runner.run(jobs)
        except asyncio.CancelledError:
            for job in jobs:
                session = manager.get_session(job.session_id) or {}
                if session.get("status") not in FINAL:
                    manager.update_session(job.session_id, {"status": "cancelled"})
            raise KeyboardInterrupt from None


def _summarize(results, output_dir: Path, kind: str, args) -> int:
    from cua_bench.results import pass_at_k

    ok = [r for r in results if r.ok]
    failed = [r for r in results if not r.ok]
    rewards = [r.reward for r in results if r.reward is not None]
    from cua_bench.runner.batch_runner import RESULT_SCHEMA_VERSION

    targets: Counter = Counter(
        (r.on, r.kind, r.runtime, r.image_variant, r.image_digest or r.image_ref)
        for r in results
    )
    summary = {
        "schema_version": RESULT_SCHEMA_VERSION,
        "total": len(results),
        "completed": len(ok),
        "failed": len(failed),
        "avg_reward": (sum(rewards) / len(rewards)) if rewards else None,
        "results": [
            {
                "task": r.task,
                "variant": r.variant,
                "status": r.status,
                "reward": r.reward,
                "pool": r.pool,
                "duration_s": r.duration_s,
                "error": r.error,
                "kind": r.kind,
                "runtime": r.runtime,
                "image_variant": r.image_variant,
                "image_digest": r.image_digest,
                "attempt": r.attempt,
                "retries": r.retries,
            }
            for r in results
        ],
        # Harbor-compatible job stats (additive): errored = no reward (the run broke).
        "n_total_trials": len(results),
        "stats": {
            "n_completed": len(ok),
            "n_errored": sum(1 for r in results if r.reward is None and not r.ok),
            "n_retries": sum(r.retries for r in results),
        },
        "pass_at_k": pass_at_k(results),
        # Where the variants ran: one row per (on, kind, runtime, variant, image).
        "targets": [
            {
                "on": on,
                "kind": kind,
                "runtime": rt,
                "image_variant": var,
                "image": img,
                "count": n,
            }
            for (on, kind, rt, var, img), n in sorted(
                targets.items(), key=lambda kv: str(kv[0])
            )
        ],
    }
    (output_dir / "summary.json").write_text(json.dumps(summary, indent=2))
    print()
    avg = "-" if summary["avg_reward"] is None else f"{summary['avg_reward']:.3f}"
    color = GREEN if not failed else RED
    print(f"{color}{len(ok)}/{len(results)} completed{RESET}, avg reward {avg}")
    if summary["pass_at_k"]:
        ks = ", ".join(f"pass@{k}={v:.3f}" for k, v in summary["pass_at_k"].items())
        print(f"  {ks}")
    for r in failed:
        attempt = f" a{r.attempt}" if r.attempt else ""
        print(
            f"  {RED}✗ {r.task} v{r.variant}{attempt}{RESET}: "
            f"{(r.error or r.status).splitlines()[0]}"
        )
        print(f"    {GREY}log: {Path(r.output_dir) / 'run.log'}{RESET}")
    print(f"Results: {output_dir}")
    return 0 if not failed else 1


def _print_dry_run(target, jobs) -> int:
    """``--dry-run``: what each variant would run on, without starting anything."""
    from cua_bench.targets import plan_claims

    reporter = ConsoleReporter()
    reporter.plan(
        target, jobs, plan_claims(((j.session_id, j.spec) for j in jobs), target.concurrency)
    )
    where = "Fleet" if target.cloud else "local"
    for job in jobs:
        spec = job.spec
        note = {
            "cli": "--kind/--runtime",
            "platform": "--platform",
            "task": "task",
            "index": "image index",
            "default": "default",
        }
        source = note.get(spec.kind_source, spec.kind_source)
        reporter.line(
            f"  {job.name}: {spec.image_label} -> {spec.kind} ({spec.image_variant}) "
            f"on {where} via {spec.backend(target.on)} [{source}]"
        )
    reporter.line(f"{GREY}Dry run: no sandbox was started.{RESET}")
    return 0


def _spawn_detached(args, run_id: str, output_dir: Path, jobs) -> int:
    argv = [a for a in getattr(args, "_argv", sys.argv[1:]) if a not in ("--detach", "-d")]
    cmd = [sys.executable, "-m", "cua_bench.cli.main", *argv, "--run-id", run_id]
    if not any(a == "--output-dir" or a.startswith("--output-dir=") for a in argv):
        cmd += ["--output-dir", str(output_dir)]
    env = {**os.environ, "PYTHONIOENCODING": "utf-8", "CUA_BENCH_NO_BANNER": "1"}
    with open(output_dir / "console.log", "w", encoding="utf-8") as log:
        proc = subprocess.Popen(
            cmd, stdout=log, stderr=subprocess.STDOUT, start_new_session=True, env=env
        )
    (output_dir / "run.pid").write_text(str(proc.pid))
    print(f"\n{GREEN}✓ Started {len(jobs)} variant(s) in the background (pid {proc.pid}){RESET}")
    print(f"\n  cb run watch {run_id}   {GREY}# live status{RESET}")
    print(f"  cb run logs {run_id}    {GREY}# logs{RESET}")
    print(f"  cb run stop {run_id}    {GREY}# stop and release sandboxes{RESET}")
    return 0


# =============================================================================
# Run inspection
# =============================================================================


def _run_sessions(run_id: str) -> list[dict]:
    from cua_bench.sessions import list_sessions

    return [s for s in list_sessions() if s.get("run_id") == run_id]


def _dataset_of(sessions: list[dict]) -> str:
    env_path = sessions[0].get("env_path", "") if sessions else ""
    if not env_path:
        return "-"
    parts = Path(env_path).parts
    if "datasets" in parts and parts.index("datasets") + 1 < len(parts):
        return parts[parts.index("datasets") + 1]
    return Path(env_path).parent.name


def _rows(sessions: list[dict]) -> list[dict]:
    from cua_bench.sessions import session_status

    rows = []
    for session in sessions:
        info = session_status(session)
        rows.append(
            {
                "session_id": session.get("session_id", "?"),
                "environment": Path(session.get("env_path", "?")).name,
                "variant": str(session.get("task_index", 0)),
                "on": session.get("location") or session.get("on") or session.get("provider") or "-",
                "status": info["status"],
                "reward": info["reward"],
                "output_dir": session.get("output_dir"),
            }
        )
    return rows


def _fmt_reward(reward: Optional[float]) -> str:
    return "-" if reward is None else f"{reward:g}"


def cmd_list(args) -> int:
    from cua_bench.sessions import list_sessions

    runs: dict[str, list[dict]] = defaultdict(list)
    for session in list_sessions():
        if session.get("run_id"):
            runs[session["run_id"]].append(session)
    if not runs:
        print(f"{GREY}No runs found.{RESET}\n\nStart one with:\n  cb run <task|dataset>")
        return 0
    print(
        f"{BOLD}{'RUN ID':<10}  {'ON':<6}  {'DATASET':<24}  {'AGENT':<12}  "
        f"{'STATUS':<34}  AVG REWARD{RESET}"
    )
    for run_id, sessions in runs.items():
        rows = _rows(sessions)
        counts = Counter(row["status"] for row in rows)
        rewards = [row["reward"] for row in rows if row["reward"] is not None]
        avg = f"{sum(rewards) / len(rewards):.3f}" if rewards else "-"
        status = " ".join(f"{k}({v})" for k, v in sorted(counts.items()))
        print(
            f"{run_id:<10}  {rows[0]['on']:<6}  {_dataset_of(sessions)[:24]:<24}  "
            f"{(sessions[0].get('agent') or '-')[:12]:<12}  {status[:34]:<34}  {avg}"
        )
    print(f"\n{GREY}cb run info <id> | watch <id> | logs <id> | stop <id>{RESET}")
    return 0


def cmd_info(args) -> int:
    sessions = _run_sessions(args.run_id)
    if not sessions:
        print(f"{RED}Error: No sessions found for run: {args.run_id}{RESET}")
        return 1
    rows = _rows(sessions)
    done = sum(1 for row in rows if row["status"] in FINAL)
    rewards = [row["reward"] for row in rows if row["reward"] is not None]
    first = sessions[0]
    print(f"\n{BOLD}Run: {args.run_id}{RESET}")
    print(f"Dataset:  {_dataset_of(sessions)}")
    where = first.get("location") or first.get("on") or "-"
    print(f"Target:   {where} ({first.get('backend', '-')})")
    print(f"Image:    {first.get('image', '-')}")
    if first.get("kind") or first.get("runtime"):
        variant = first.get("image_variant") or "-"
        engine = f", {first['runtime']}" if first.get("runtime") else ""
        print(f"Kind:     {first.get('kind') or '-'}{engine} ({variant})")
    if first.get("image_digest"):
        print(f"Digest:   {first.get('image_digest')}")
    print(f"Agent:    {first.get('agent') or '-'}")
    print(f"Model:    {first.get('model') or '-'}")
    print(f"Progress: {done}/{len(rows)}")
    if rewards:
        print(f"Avg reward: {sum(rewards) / len(rewards):.3f}")
    out = Path(first.get("output_dir", "")).parent if first.get("output_dir") else None
    if out:
        print(f"Output:   {out}")
    print(f"\n{BOLD}{'SESSION ID':<48}  {'VARIANT':<7}  {'STATUS':<10}  REWARD{RESET}")
    for row in rows:
        print(
            f"{row['session_id'][:48]:<48}  {row['variant']:<7}  {row['status']:<10}  "
            f"{_fmt_reward(row['reward'])}"
        )
    return 0


def cmd_watch(args) -> int:
    try:
        from rich.console import Console
        from rich.live import Live
        from rich.table import Table
    except ImportError:
        print(f"{RED}Error: 'rich' is required for watch mode.{RESET}")
        return 1
    run_id = args.run_id
    if not _run_sessions(run_id):
        print(f"{RED}No sessions found for run: {run_id}{RESET}")
        return 1
    console = Console()

    def table() -> tuple[Table, bool]:
        rows = _rows(_run_sessions(run_id))
        done = sum(1 for row in rows if row["status"] in FINAL)
        t = Table(title=f"run {run_id}: {done}/{len(rows)} done", expand=True, box=None)
        for column in ("ENVIRONMENT", "VARIANT", "STATUS", "REWARD"):
            t.add_column(column)
        style = {"completed": "cyan", "failed": "red", "cancelled": "red", "running": "green"}
        for row in rows:
            t.add_row(
                row["environment"],
                row["variant"],
                f"[{style.get(row['status'], 'yellow')}]{row['status']}[/]",
                _fmt_reward(row["reward"]),
            )
        return t, done == len(rows)

    try:
        with Live(console=console, refresh_per_second=2) as live:
            for _ in range(24 * 3600 * 2):  # bounded: at most a day of polling
                current, finished = table()
                live.update(current)
                if finished:
                    break
                time.sleep(0.5)
    except KeyboardInterrupt:
        console.print(f"[yellow]Detached. Resume with: cb run watch {run_id}[/yellow]")
    return 0


def cmd_stop(args) -> int:
    from cua_bench.sessions import manager

    run_id = args.run_id
    pid_file = _get_run_output_dir(run_id) / "run.pid"
    sessions = _run_sessions(run_id)
    if not pid_file.exists() and sessions and sessions[0].get("output_dir"):
        pid_file = Path(sessions[0]["output_dir"]).parent / "run.pid"
    if pid_file.exists():
        try:
            pid = int(pid_file.read_text().strip())
            os.kill(pid, signal.SIGTERM)
            print(f"{CYAN}Stopping run {run_id} (pid {pid}); it releases its sandboxes...{RESET}")
            for _ in range(120):  # up to 60 s for a clean release
                try:
                    os.kill(pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.5)
            else:
                print(f"{YELLOW}Still running after 60s; cloud claims expire on their TTL.{RESET}")
        except (ValueError, ProcessLookupError):
            pass
    for session in sessions:
        if session.get("status") not in FINAL:
            manager.update_session(session["session_id"], {"status": "cancelled"})
    print(f"{GREEN}✓ Run stopped{RESET}")
    return 0


def cmd_logs(args) -> int:
    from cua_bench.sessions import get_session, session_logs

    identifier = args.identifier
    tail = getattr(args, "tail", None)
    session = get_session(identifier)
    sessions = [session] if session else _run_sessions(identifier)
    if not sessions:
        print(f"{RED}Error: no run or session {identifier}{RESET}")
        return 1
    for session in sessions:
        print(f"\n{'=' * 60}\nSession: {session.get('session_id')}\n{'=' * 60}")
        print(session_logs(session, tail=tail) or f"{GREY}(no log yet){RESET}")
    return 0


# =============================================================================
# Config defaults
# =============================================================================


def _apply_config_defaults_for_task(args):
    """Apply .cua/config.yaml defaults for unset CLI arguments."""
    from cua_bench.config import ConfigLoader, detect_env_type

    path_arg = getattr(args, "task_path", None) or getattr(args, "dataset_path", None)
    search_path = Path(path_arg).resolve() if path_arg else Path.cwd()
    if not search_path.exists():
        search_path = Path.cwd()
    config_loader = ConfigLoader(search_path)
    if config_loader.find_config_dir():
        print(f"{GREY}Found config at: {config_loader.find_config_dir()}{RESET}")
    env_type = detect_env_type(str(path_arg)) if path_arg else None
    keys = ("agent", "agent_import_path", "model", "max_steps", "output_dir")
    effective = config_loader.get_effective_config(
        {k: getattr(args, k, None) for k in keys}, env_type
    )
    for key in keys:
        if not getattr(args, key, None) and effective.get(key):
            setattr(args, key, effective[key])
    args._config_loader = config_loader
    return args


def execute(args):
    commands = {
        "task": cmd_run_task,
        "dataset": cmd_run_dataset,
        "list": cmd_list,
        "watch": cmd_watch,
        "stop": cmd_stop,
        "logs": cmd_logs,
        "info": cmd_info,
    }
    command = commands.get(getattr(args, "run_command", None))
    if command is None:
        print(f"{YELLOW}Usage:{RESET}")
        print("  cb run <task|dataset> [--on local|cloud]   Run tasks")
        print("  cb run list | info <id> | watch <id> | logs <id> | stop <id>")
        return 1
    return command(args)
