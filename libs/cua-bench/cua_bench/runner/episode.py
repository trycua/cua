"""One task episode: setup, agent or oracle, evaluate, save the trace.

Shared by the unified runner (in-process, against a cua-sandbox sandbox) and
the standalone ``cua_bench.batch.solver``. It
prints the same markers the result readers look for
(``✓ Evaluation result: [...]``, ``✓ Task N completed successfully!``).
"""

from __future__ import annotations

import asyncio
import importlib
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Optional


@dataclass
class AgentOptions:
    """How to solve a task: the oracle, or an agent (optionally with a model)."""

    oracle: bool = True
    agent: Optional[str] = None
    agent_import_path: Optional[str] = None
    model: Optional[str] = None
    max_steps: Optional[int] = None
    dump: bool = False  # setup + evaluate only
    save_pngs: bool = False
    filter_events: Optional[list[str]] = None

    @property
    def label(self) -> str:
        if self.dump:
            return "dump"
        if self.oracle:
            return "oracle"
        return self.agent or self.agent_import_path or "agent"


@dataclass
class EpisodeResult:
    success: bool
    evaluation: Any = None
    reward: Optional[float] = None
    error: Optional[str] = None
    failure_mode: Optional[str] = None
    extra: dict = field(default_factory=dict)
    #: Phase -> (started, finished) as aware datetimes: "setup", "agent", "verifier".
    timing: dict = field(default_factory=dict)
    #: The task description (for ATIF and result files).
    description: Optional[str] = None


def _now():
    from datetime import datetime, timezone

    return datetime.now(timezone.utc)


class EpisodeError(RuntimeError):
    pass


def reward_of(evaluation: Any) -> Optional[float]:
    """Scalar reward from an evaluate() result (a number or a list of numbers)."""
    if evaluation is None:
        return None
    if isinstance(evaluation, bool):
        return 1.0 if evaluation else 0.0
    if isinstance(evaluation, (int, float)):
        return float(evaluation)
    if isinstance(evaluation, (list, tuple)) and evaluation:
        values = [reward_of(v) for v in evaluation]
        values = [v for v in values if v is not None]
        return sum(values) / len(values) if values else None
    return None


def load_agent_class(opts: AgentOptions) -> Any:
    if opts.agent_import_path:
        if ":" not in opts.agent_import_path:
            raise EpisodeError("--agent-import-path must look like 'package.module:ClassName'")
        module_path, class_name = opts.agent_import_path.split(":", 1)
        return getattr(importlib.import_module(module_path), class_name)
    if opts.agent:
        from cua_bench.agents import get_agent, list_agents

        agent_class = get_agent(opts.agent)
        if agent_class is None:
            raise EpisodeError(
                f"unknown agent {opts.agent!r} (available: {', '.join(list_agents())})"
            )
        return agent_class
    return None


#: Trace screenshots are best effort: a daemon-less image has no screen API
#: and must not stall the episode.
SCREENSHOT_TIMEOUT_S = 20.0


async def _screenshot(session: Any) -> Optional[bytes]:
    if getattr(session, "_cb_no_screen", False):
        return None
    try:
        return await asyncio.wait_for(session.screenshot(), SCREENSHOT_TIMEOUT_S)
    except Exception as error:  # noqa: BLE001 - daemon-less images have no screen API
        print(f"Warning: screenshot unavailable, tracing without images ({error!r})")
        try:
            session._cb_no_screen = True
        except Exception:  # noqa: BLE001
            pass
        return None


def _record(env: Any, name: str, data: dict, shots: Optional[list] = None) -> None:
    try:
        env.tracing.record(name, data, [s for s in (shots or []) if s])
    except Exception as error:  # noqa: BLE001 - tracing never fails a task
        print(f"Warning: failed to record {name} event: {error}")


async def run_episode(
    env: Any,
    task_index: int,
    opts: AgentOptions,
    output_dir: Path,
    *,
    session: Any = None,
) -> EpisodeResult:
    """Run one variant of ``env`` and save its trace under ``output_dir``.

    ``session`` is a connected desktop session; ``None`` opens the task's
    sandbox through ``env.reset`` (and ``env.close`` releases it).
    """
    output_dir.mkdir(parents=True, exist_ok=True)
    env.headless = True
    try:
        tid = env.tracing.start()
        print(f"Tracing started. trajectory_id={tid}")
    except Exception:  # noqa: BLE001
        print("Failed to start tracing; continuing without trace.")

    timing: dict = {}
    setup_started = _now()
    if session is None:
        _, task_cfg = await env.reset(task_id=task_index)
        session = env.session
        print(f"✓ Session ready: {task_cfg.description}")
    else:
        if env.tasks_config_fn is None:
            raise EpisodeError("no @cb.tasks_config function found")
        tasks = env.tasks_config_fn()
        if task_index >= len(tasks):
            raise EpisodeError(f"variant {task_index} out of range ({len(tasks)} variants)")
        task_cfg = tasks[task_index]
        env.current_task = task_cfg
        env.session = session
        print(f"Running task {task_index}: {task_cfg.description}")
        from .desktop import wait_for_desktop

        waited = time.perf_counter()
        if await wait_for_desktop(session) == "ready":
            print(f"✓ Desktop ready in {time.perf_counter() - waited:.2f}s")
        t0 = time.perf_counter()
        if env.setup_task_fn:
            await env.setup_task_fn(task_cfg, session)
        await asyncio.sleep(2.0)
        shot = await _screenshot(session)
        elapsed = time.perf_counter() - t0
        print(f"✓ Setup complete in {elapsed:.2f}s")
        _record(
            env,
            "reset",
            {"task": repr(task_cfg), "task_index": task_index, "elapsed": elapsed},
            [shot],
        )

    timing["setup"] = (setup_started, _now())
    result = EpisodeResult(success=True, timing=timing, description=task_cfg.description)
    agent_started = _now()
    if opts.dump:
        print("ℹ Dump mode: skipping solver")
    elif opts.oracle:
        if env.solve_task_fn:
            await env.solve_task_fn(task_cfg, session)
            shot = await _screenshot(session)
            print("✓ Solution complete")
            _record(env, "solve", {"completed": True}, [shot])
        else:
            print("Warning: No @cb.solve_task function found")
    else:
        agent_class = load_agent_class(opts)
        if agent_class is None:
            print("ℹ Eval mode without agent: skipping to evaluation")
        else:
            kwargs: dict[str, Any] = {}
            if opts.model:
                kwargs["model"] = opts.model
            if opts.max_steps is not None:
                kwargs["max_steps"] = opts.max_steps
            agent = agent_class(**kwargs)
            logging_dir = output_dir / f"task_{task_index}_agent_logs"
            logging_dir.mkdir(exist_ok=True, parents=True)
            print(f"Running agent: {agent.name()}")
            agent_result = await agent.perform_task(
                task_description=task_cfg.description,
                session=session,
                logging_dir=logging_dir,
                tracer=env.tracing,
            )
            print(f"✓ Agent complete: {agent_result}")
            from cua_bench.agents.base import FailureMode

            if agent_result.failure_mode not in (FailureMode.UNSET, FailureMode.NONE):
                result.success = False
                result.failure_mode = str(agent_result.failure_mode)
                print(f"✗ Agent failed with failure mode: {agent_result.failure_mode}")

    timing["agent"] = (agent_started, _now())
    if env.evaluate_task_fn:
        verify_started = _now()
        evaluation = await env.evaluate_task_fn(task_cfg, session)
        timing["verifier"] = (verify_started, _now())
        result.evaluation = evaluation
        result.reward = reward_of(evaluation)
        print(f"✓ Evaluation result: {evaluation}")
        _record(env, "evaluate", {"result": evaluation})

    trace_dir = output_dir / f"task_{task_index}_trace"
    try:
        env.tracing.save_to_disk(
            str(trace_dir),
            save_pngs=opts.save_pngs,
            image_dir=str(output_dir / "imgs"),
            filter_events=opts.filter_events,
        )
        print(f"✓ Trace saved to {trace_dir}")
    except Exception as error:  # noqa: BLE001 - the task result stands without a trace
        print(f"Warning: Failed to save trace (task result unaffected): {error}")

    try:
        from ..results import write_atif

        rows = list(getattr(env.tracing, "_rows", []) or [])
        path = write_atif(
            rows,
            output_dir,
            session_id=getattr(env.tracing, "trajectory_id", None),
            agent_name=opts.label,
            model_name=opts.model,
            description=task_cfg.description,
            evaluation=result.evaluation,
            reward=result.reward,
        )
        print(f"✓ ATIF trajectory saved to {path}")
    except Exception as error:  # noqa: BLE001 - optional export
        print(f"Warning: Failed to write trajectory.json: {error}")

    return result
