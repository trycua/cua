"""Run task variants on a Target, in parallel, in this process.

Every variant gets its own sandbox, its own log file and its own
``result.json``; ``Target.concurrency`` bounds how many run
at once. In the cloud, variants with the same image share one managed pool
sized for the batch (``max_pool_size = min(concurrency, variants)``), so the
pool scales up from zero for the batch and back down afterwards.

Cancellation (Ctrl-C, ``cb run stop``) cancels the running variants; each
one's ``async with`` releases its sandbox or claim on the way out. If the
process dies instead, cloud claims expire after their TTL.
"""

from __future__ import annotations

import asyncio
import json
import time
import traceback
from contextlib import asynccontextmanager
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable, Optional, Protocol

from ..sandboxes import Progress, explain_error, image_facts, open_sandbox
from ..targets import EnvSpec, PoolPlan, Target, TargetError, plan_claims
from .episode import AgentOptions, run_episode
from .output import routed_stdio, task_log


@dataclass
class Job:
    task_path: Path
    variant: int
    spec: EnvSpec
    session_id: str
    output_dir: Path
    #: Repeat index for --attempts (0 = the first; pass@k uses all of them).
    attempt: int = 0

    @property
    def name(self) -> str:
        suffix = f" a{self.attempt}" if self.attempt else ""
        return f"{self.task_path.name} v{self.variant}{suffix}"


#: Version of the ``result.json`` / ``summary.json`` layout. New keys are
#: additive only; a rename or removal bumps it.
RESULT_SCHEMA_VERSION = 1


@dataclass
class JobResult:
    """One variant's outcome: the cua-bench fields of its ``result.json``."""

    #: The session id (also the variant's log and trace key).
    session_id: str
    #: Task directory name.
    task: str
    #: Variant index (``--variant-id``).
    variant: int
    #: ``completed``, ``failed`` or ``cancelled``.
    status: str  # completed | failed | cancelled
    #: The mean of the evaluation's numbers (``true`` counts as 1), or ``null``
    #: when evaluation did not run.
    reward: Optional[float] = None
    #: What ``@cb.evaluate_task`` returned (commonly a list of floats).
    evaluation: Any = None
    #: The error message when the variant failed.
    error: Optional[str] = None
    #: Wall time of the variant in seconds.
    duration_s: float = 0.0
    #: Where it ran: ``local`` or ``cloud``.
    on: str = "local"
    #: What ran it, ``<location>-<engine>`` (``local-gvisor``, ``local-qemu``,
    #: ``cloud-gvisor``, ``cloud-kubevirt``, ``cloud-pool:<name>``).
    backend: str = ""
    #: The image as the task or ``--image`` named it.
    image: Optional[str] = None
    #: The Fleet pool that served the claim (cloud runs).
    pool: Optional[str] = None
    #: The variant's output directory.
    output_dir: str = ""
    #: Extra values the task or agent reported.
    extra: dict = field(default_factory=dict)
    # What ran (additive, schema_version 1): container|vm, the engine, the
    # image as requested/resolved, its variant (rootfs|containerdisk|lume),
    # the pinned repo@sha256 when the SDK reports it, and the architecture.
    #: ``container`` or ``vm``.
    kind: Optional[str] = None
    #: The engine (``gvisor``, ``runc``, ``qemu``, ``lume``, ``kubevirt``)
    #: when one was chosen; ``None``: the SDK picked.
    runtime: Optional[str] = None
    #: The resolved image reference.
    image_ref: Optional[str] = None
    #: The image variant that ran: ``rootfs``, ``containerdisk`` or ``lume``.
    image_variant: Optional[str] = None
    #: The pinned ``repo@sha256:...`` when the SDK reports it.
    image_digest: Optional[str] = None
    #: Guest architecture, when known.
    arch: Optional[str] = None
    #: The ``--attempts`` repeat index (0 for the first).
    attempt: int = 0
    #: How many infrastructure retries (``--retries``) the variant took.
    retries: int = 0
    #: The sandbox's qualified ref (``local:<name>``, ``cloud:<name>``), when known.
    sandbox: Optional[str] = None

    @property
    def ok(self) -> bool:
        return self.status == "completed"


class Reporter(Protocol):
    def plan(self, target: Target, jobs: list[Job], plans: list[PoolPlan]) -> None: ...
    def started(self, job: Job, index: int, total: int) -> None: ...
    def progress(self, job: Job, event: Progress) -> None: ...
    def finished(self, result: JobResult, index: int, total: int) -> None: ...


class NullReporter:
    def plan(self, target, jobs, plans) -> None:  # noqa: D401
        pass

    def started(self, job, index, total) -> None:
        pass

    def progress(self, job, event) -> None:
        pass

    def finished(self, result, index, total) -> None:
        pass


StatusFn = Callable[[Job, str, dict], None]
EnvFactory = Callable[[str], Any]
Opener = Callable[..., Any]


def _default_env_factory(path: str) -> Any:
    from cua_bench import make

    return make(path)


def _sandbox_ref(sandbox: Any) -> Optional[str]:
    """``sb.id`` (the qualified ref every cua surface prints), else its name."""
    try:
        ref = getattr(sandbox, "id", None)
    except Exception:  # noqa: BLE001 - informational only
        ref = None
    ref = ref if isinstance(ref, str) and ref else None
    name = getattr(sandbox, "name", None)
    return ref or (name if isinstance(name, str) else None)


def _attach_session(sandbox: Any, spec: EnvSpec) -> Any:
    from cua_bench.computers.remote import RemoteDesktopSession

    return RemoteDesktopSession.attach(
        sandbox, os_type=spec.os_type, width=spec.width, height=spec.height
    )


@asynccontextmanager
async def _no_sandbox(spec: EnvSpec, target: Target, **_: Any):
    """Dataset tasks: nothing to start or release."""
    yield None


def _dataset_session(sandbox: Any, spec: EnvSpec) -> Any:
    from cua_bench.computers.dataset import DatasetSession

    return DatasetSession(width=spec.width, height=spec.height)


class BatchRunner:
    def __init__(
        self,
        target: Target,
        opts: AgentOptions,
        *,
        reporter: Optional[Reporter] = None,
        on_status: Optional[StatusFn] = None,
        opener: Opener = open_sandbox,
        env_factory: EnvFactory = _default_env_factory,
        attach: Callable[[Any, EnvSpec], Any] = _attach_session,
        retries: int = 0,
        retry_backoff_s: float = 2.0,
    ) -> None:
        self.target = target
        self.retries = max(0, int(retries))
        self.retry_backoff_s = retry_backoff_s
        self.opts = opts
        self.reporter = reporter or NullReporter()
        self.on_status = on_status
        self._opener = opener
        self._env_factory = env_factory
        self._attach = attach
        # Cloud: the first claim per pool key goes alone until its pool exists,
        # so concurrent first-use claims never race to create the same pool.
        self._pool_gates: dict[tuple, asyncio.Event] = {}

    def plans(self, jobs: list[Job]) -> list[PoolPlan]:
        return plan_claims(((job.session_id, job.spec) for job in jobs), self.target.concurrency)

    async def run(self, jobs: list[Job]) -> list[JobResult]:
        plans = self.plans(jobs)
        pool_size = {plan.spec.pool_key: plan.max_pool_size for plan in plans}
        self.reporter.plan(self.target, jobs, plans)
        semaphore = asyncio.Semaphore(self.target.concurrency)
        total = len(jobs)
        finished = 0

        async def one(index: int, job: Job) -> JobResult:
            nonlocal finished
            async with semaphore:
                self.reporter.started(job, index, total)
                result = await self.run_job(job, max_pool_size=pool_size.get(job.spec.pool_key, 1))
            finished += 1
            self.reporter.finished(result, finished, total)
            return result

        with routed_stdio():
            tasks = [asyncio.create_task(one(i + 1, job)) for i, job in enumerate(jobs)]
            try:
                return list(await asyncio.gather(*tasks))
            except asyncio.CancelledError:
                for task in tasks:
                    task.cancel()
                # Let every variant run its cleanup (claim release) before leaving.
                await asyncio.gather(*tasks, return_exceptions=True)
                raise

    def _status(self, job: Job, status: str, **fields: Any) -> None:
        if self.on_status is not None:
            try:
                self.on_status(job, status, fields)
            except Exception:  # noqa: BLE001 - bookkeeping never fails a task
                pass

    async def run_job(self, job: Job, *, max_pool_size: int = 1) -> JobResult:
        """Run one variant; retry infrastructure failures up to ``retries`` times.

        A retry starts over in a fresh sandbox. Only failures before the task
        was evaluated retry (sandbox start, connection, setup errors); an agent
        failure, a finished evaluation or a cancellation never does.
        """
        retries = 0
        while True:
            result = await self._run_once(job, max_pool_size=max_pool_size, retries=retries)
            if not (getattr(result, "_retryable", False) and retries < self.retries):
                return result
            retries += 1
            delay = min(60.0, self.retry_backoff_s * 2 ** (retries - 1))
            with task_log(job.output_dir / "run.log"):
                print(f"\n↻ Retrying {job.name} ({retries}/{self.retries}) in {delay:.0f}s")
            await asyncio.sleep(delay)

    async def _run_once(self, job: Job, *, max_pool_size: int, retries: int) -> JobResult:
        target, spec = self.target, job.spec
        result = JobResult(
            session_id=job.session_id,
            task=job.task_path.name,
            variant=job.variant,
            status="failed",
            on=target.on,
            backend=spec.backend(target.on),
            image=spec.image,
            output_dir=str(job.output_dir),
            kind=spec.kind,
            runtime=spec.runtime,
            image_ref=spec.image,
            image_variant=spec.image_variant,
            attempt=job.attempt,
            retries=retries,
        )
        from ..results import _now_utc

        result._started_at = _now_utc()  # type: ignore[attr-defined]
        result._setup_started = None  # type: ignore[attr-defined]
        result._timing = {}  # type: ignore[attr-defined]
        result._error_obj = None  # type: ignore[attr-defined]
        started = time.monotonic()
        job.output_dir.mkdir(parents=True, exist_ok=True)
        self._status(job, "starting")
        pool_seen: dict[str, Optional[str]] = {"pool": None}

        gate: Optional[asyncio.Event] = None
        opener, attach = self._opener, self._attach
        if not spec.needs_sandbox:
            opener, attach = _no_sandbox, _dataset_session
        if target.cloud and spec.needs_sandbox:
            existing = self._pool_gates.get(spec.pool_key)
            if existing is None:
                gate = self._pool_gates[spec.pool_key] = asyncio.Event()
            else:
                await existing.wait()
        loop = asyncio.get_running_loop()

        def open_gate() -> None:
            if gate is not None and not gate.is_set():
                loop.call_soon_threadsafe(gate.set)

        def on_progress(event: Progress) -> None:
            if event.pool:
                pool_seen["pool"] = event.pool
            if event.stage != "pool":  # past pool lookup/creation
                open_gate()
            print(f"[sandbox] {event.stage}: {event.message}")
            self.reporter.progress(job, event)

        with task_log(job.output_dir / "run.log"):
            print(f"== {job.name} on {target.on} ({result.backend}) ==")
            print(f"Image: {spec.image_label}")
            try:
                env = self._env_factory(str(job.task_path))
                result._setup_started = _now_utc()  # type: ignore[attr-defined]
                async with opener(
                    spec, target, max_pool_size=max_pool_size, on_progress=on_progress
                ) as sandbox:
                    open_gate()
                    result.pool = pool_seen["pool"] or getattr(sandbox, "pool_name", None)
                    result.sandbox = _sandbox_ref(sandbox)
                    facts = image_facts(sandbox, spec)
                    for key, value in facts.items():
                        setattr(result, key, value)
                    if facts["image_digest"]:
                        print(f"Image digest: {facts['image_digest']} ({facts['image_variant']})")
                    self._status(
                        job,
                        "running",
                        sandbox=result.sandbox,
                        pool=result.pool,
                        **{k: v for k, v in facts.items() if v is not None},
                    )
                    session = attach(sandbox, spec)
                    try:
                        episode = await run_episode(
                            env, job.variant, self.opts, job.output_dir, session=session
                        )
                    finally:
                        await session.close()
                result.reward = episode.reward
                result.evaluation = episode.evaluation
                result._timing = episode.timing  # type: ignore[attr-defined]
                if episode.success:
                    result.status = "completed"
                    print(f"\n✓ Task {job.variant} completed successfully!")
                else:
                    result.error = f"agent failure: {episode.failure_mode}"
                    print(f"\n✗ Task failed: {result.error}")
            except asyncio.CancelledError:
                open_gate()
                result.status = "cancelled"
                result.error = "cancelled"
                print("\n✗ Task cancelled (sandbox released)")
                self._finish(job, result, started)
                raise
            except Exception as error:  # noqa: BLE001 - one variant never stops the batch
                open_gate()
                result.error = explain_error(error, spec, target)
                result._error_obj = error  # type: ignore[attr-defined]
                result._retryable = not isinstance(error, TargetError)  # type: ignore[attr-defined]
                traceback.print_exc()
                print(f"\n✗ Task failed: {result.error}")
        self._finish(job, result, started)
        return result

    def _finish(self, job: Job, result: JobResult, started: float) -> None:
        from ..results import _now_utc, harbor_trial_fields

        result.duration_s = round(time.monotonic() - started, 2)
        payload = {"schema_version": RESULT_SCHEMA_VERSION, **asdict(result)}
        # Harbor TrialResult fields, additive (harbor tooling reads them).
        payload.update(
            harbor_trial_fields(
                task=result.task,
                variant=result.variant,
                attempt=result.attempt,
                output_dir=result.output_dir,
                reward=result.reward,
                evaluation=result.evaluation,
                error=getattr(result, "_error_obj", None),
                started_at=getattr(result, "_started_at", None) or _now_utc(),
                finished_at=_now_utc(),
                setup_started=getattr(result, "_setup_started", None),
                timing=getattr(result, "_timing", {}) or {},
                agent_label=self.opts.label,
                model=self.opts.model,
            )
        )
        try:
            json.dumps(payload["evaluation"])
        except (TypeError, ValueError):
            payload["evaluation"] = repr(result.evaluation)
        try:
            (job.output_dir / "result.json").write_text(json.dumps(payload, indent=2))
        except OSError:
            pass
        fields = {"reward": result.reward, "error": result.error, "pool": result.pool}
        self._status(job, result.status, **{k: v for k, v in fields.items() if v is not None})
