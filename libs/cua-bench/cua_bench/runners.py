"""Benchmark runner functions for cua-bench.

This module provides programmatic interfaces for running benchmarks and
interactive environments, using the core gym interface (make, reset, step, evaluate).
"""

import asyncio
import fnmatch
import hashlib
import json
import math
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

from .core import Task, make
from .dataset_manifest import verify_dataset
from .environment import Environment
from .types import Action, DoneAction


@dataclass
class BenchmarkResult:
    """Result of a benchmark run.

    Attributes:
        run_id: Unique identifier for this run
        task_results: List of individual task results
        total_tasks: Total number of tasks in the benchmark
        success_count: Number of successful tasks
        failed_count: Number of failed tasks
        avg_reward: Average reward across all tasks
        duration_seconds: Total duration of the benchmark
        output_dir: Output directory for results (if any)
    """

    run_id: str
    task_results: List[Dict[str, Any]]
    total_tasks: int
    success_count: int
    failed_count: int
    avg_reward: float
    duration_seconds: float
    output_dir: Optional[str] = None
    dataset_manifest_sha256: Optional[str] = None


@dataclass
class TaskResult:
    """Result of a single task execution.

    Attributes:
        task_path: Path to the task
        variant_id: Task variant index
        success: Whether the task succeeded
        reward: Reward from evaluation
        steps: Number of steps taken
        error: Error message if failed
    """

    task_path: str
    variant_id: int
    success: bool
    reward: float
    steps: int
    error: Optional[str] = None
    action_trace_digest: Optional[str] = None
    action_trace_events: Optional[List[Dict[str, Any]]] = None


async def run_single_task(
    env_path: Path,
    task_index: int = 0,
    *,
    split: str = "train",
    agent_fn: Optional[Callable[[bytes, Task], Action]] = None,
    max_steps: int = 100,
    oracle: bool = False,
) -> TaskResult:
    """Run a single task using the gym interface.

    This function uses the core gym interface (make, reset, step, evaluate)
    to run a task with either an agent function or the oracle solver.

    Args:
        env_path: Path to the task environment directory
        task_index: Task variant index (default: 0)
        split: Dataset split (default: "train")
        agent_fn: Optional agent function that takes (screenshot, task_config)
                  and returns an Action. If None and oracle=False, returns after setup.
        max_steps: Maximum steps per task (default: 100)
        oracle: Run oracle/solver mode (default: False)

    Returns:
        TaskResult with execution results

    Example:
        # Run with oracle
        result = await run_single_task(Path("./task"), oracle=True)

        # Run with custom agent
        def my_agent(screenshot: bytes, task: Task) -> Action:
            return DoneAction()  # Simple agent that immediately finishes

        result = await run_single_task(Path("./task"), agent_fn=my_agent)
    """
    env = None
    step_count = 0
    action_trace = []
    outcome = None

    def finish(**kwargs):
        nonlocal outcome
        outcome = TaskResult(**kwargs)
        if action_trace:
            serialized = json.dumps(action_trace, sort_keys=True, separators=(",", ":"))
            outcome.action_trace_digest = hashlib.sha256(serialized.encode("utf-8")).hexdigest()
            outcome.action_trace_events = list(action_trace)
        return outcome

    try:
        # Create environment using gym interface
        env = make(str(env_path), split=split)
        env.max_steps = max_steps

        # Reset environment
        screenshot, task_cfg = await env.reset(task_id=task_index)

        step_count = 0
        reward = 0.0

        if oracle:
            # Run oracle solver
            if env.solve_task_fn is not None:
                await env.solve()
                step_count = env.step_count
            else:
                return finish(
                    task_path=str(env_path),
                    variant_id=task_index,
                    success=False,
                    reward=0.0,
                    steps=0,
                    error="No solve_task_fn defined for oracle mode",
                )
        elif agent_fn is not None:
            # Run agent
            done = False
            while not done and step_count < max_steps:
                action = agent_fn(screenshot, task_cfg)
                screenshot = await env.step(action)
                step_count += 1
                # Privacy-minimal event chain: never persist screenshots, action
                # arguments, typed text, passwords or arbitrary agent payloads.
                action_trace.append({"step": step_count, "action_type": type(action).__name__})
                done = isinstance(action, DoneAction)

        # A setup-only run must never be counted as successful agent execution.
        if not oracle and agent_fn is None:
            return finish(
                task_path=str(env_path),
                variant_id=task_index,
                success=False,
                reward=0.0,
                steps=step_count,
                error="No agent_fn supplied; task was not executed",
            )

        # Evaluate
        if env.evaluate_task_fn is not None:
            result = await env.evaluate()
            if isinstance(result, (int, float)) and not isinstance(result, bool):
                raw_reward = result
            elif isinstance(result, list) and len(result) > 0:
                raw_reward = result[0]
            elif isinstance(result, dict) and "reward" in result:
                raw_reward = result["reward"]
            else:
                raise ValueError("Evaluator returned an unsupported reward format")
            if isinstance(raw_reward, bool) or not isinstance(raw_reward, (int, float)):
                raise ValueError("Evaluator reward must be a numeric value, not a boolean or string")
            reward = float(raw_reward)
            if not math.isfinite(reward) or not 0.0 <= reward <= 1.0:
                raise ValueError(f"Evaluator reward must be finite and within [0, 1], got {reward!r}")
        else:
            raise ValueError("Task has no evaluator; cannot produce a verified score")

        return finish(
            task_path=str(env_path),
            variant_id=task_index,
            success=reward >= 0.5,  # Common threshold
            reward=reward,
            steps=step_count,
        )

    except Exception as e:
        return finish(
            task_path=str(env_path),
            variant_id=task_index,
            success=False,
            reward=0.0,
            steps=step_count,
            error=str(e),
        )
    finally:
        if env is not None:
            try:
                await env.close()
            except Exception as close_error:
                # A task cannot be reported as successful if its environment was
                # not cleanly released; preserve its completed execution steps.
                if outcome is not None:
                    outcome.success = False
                    outcome.reward = 0.0
                    detail = f"Environment cleanup failed: {close_error}"
                    outcome.error = f"{outcome.error}; {detail}" if outcome.error else detail


async def run_benchmark(
    dataset_path: Path,
    *,
    agent_fn: Optional[Callable[[bytes, Task], Action]] = None,
    max_steps: int = 100,
    max_parallel: int = 4,
    oracle: bool = False,
    max_variants: Optional[int] = None,
    task_filter: Optional[str] = None,
    split: str = "train",
    dataset_manifest: Optional[Path] = None,
) -> BenchmarkResult:
    """Run a benchmark on a dataset using the gym interface.

    This function runs multiple tasks in parallel using the core gym interface
    (make, reset, step, evaluate).

    Args:
        dataset_path: Path to the dataset directory
        agent_fn: Optional agent function that takes (screenshot, task_config)
                  and returns an Action. Required if oracle=False.
        max_steps: Maximum steps per task (default: 100)
        max_parallel: Maximum parallel workers (default: 4)
        oracle: Run oracle/solver mode (default: False)
        max_variants: Maximum variants per task (optional)
        task_filter: Glob pattern to filter tasks (optional)
        split: Dataset split (default: "train")
        dataset_manifest: Optional manifest JSON to verify before importing or running tasks.

    Returns:
        BenchmarkResult with run statistics and task results

    Example:
        # Run oracle benchmark
        result = await run_benchmark(
            Path("./datasets/cua-bench-basic"),
            oracle=True,
            max_parallel=8,
        )
        print(f"Success rate: {result.success_count / result.total_tasks:.2%}")

        # Run with custom agent
        def random_agent(screenshot: bytes, task: Task) -> Action:
            import random
            return random.choice([
                ClickAction(x=random.randint(0, 1920), y=random.randint(0, 1080)),
                DoneAction(),
            ])

        result = await run_benchmark(
            Path("./datasets/my-dataset"),
            agent_fn=random_agent,
            max_parallel=4,
        )
    """
    if not isinstance(max_parallel, int) or isinstance(max_parallel, bool) or max_parallel < 1:
        raise ValueError("max_parallel must be a positive integer")
    if not isinstance(max_steps, int) or isinstance(max_steps, bool) or max_steps < 1:
        raise ValueError("max_steps must be a positive integer")
    if max_variants is not None and (
        not isinstance(max_variants, int)
        or isinstance(max_variants, bool)
        or max_variants < 1
    ):
        raise ValueError("max_variants must be a positive integer when provided")

    start_time = time.time()

    # Validate dataset path
    if not dataset_path.exists():
        raise FileNotFoundError(f"Dataset not found: {dataset_path}")

    # Verify pinned dataset bytes before importing task definitions or launching workers.
    manifest_sha256 = None
    if dataset_manifest is not None:
        manifest_bytes = Path(dataset_manifest).read_bytes()
        manifest = json.loads(manifest_bytes)
        verify_dataset(dataset_path, manifest)
        manifest_sha256 = hashlib.sha256(
            json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode("utf-8")
        ).hexdigest()

    # Discover tasks in dataset
    tasks: List[Path] = []
    if (dataset_path / "main.py").exists():
        tasks.append(dataset_path)
    else:
        for task_dir in sorted(dataset_path.iterdir()):
            if task_dir.is_dir() and (task_dir / "main.py").exists():
                tasks.append(task_dir)

    if not tasks:
        raise ValueError(f"No tasks found in dataset: {dataset_path}")

    # Apply task filter if specified
    if task_filter:
        tasks = [t for t in tasks if fnmatch.fnmatch(t.name, task_filter)]
        if not tasks:
            raise ValueError(f"No tasks match filter: {task_filter}")

    # Expand tasks to (task_path, variant_id) tuples
    task_variants: List[Tuple[Path, int]] = []
    for task_path in tasks:
        try:
            env = make(str(task_path), split=split)
            if env.tasks_config_fn:
                variant_count = len(env.tasks_config_fn())
            else:
                variant_count = 1
        except Exception as exc:
            raise ValueError(
                f"Failed to load task variants for {task_path} (split={split!r}): {exc}"
            ) from exc

        if variant_count < 1:
            raise ValueError(
                f"Task {task_path} (split={split!r}) has no variants; "
                "refusing to report an incomplete benchmark"
            )

        if max_variants:
            variant_count = min(variant_count, max_variants)

        for variant_id in range(variant_count):
            task_variants.append((task_path, variant_id))

    # Generate run ID
    run_id = f"run-{uuid.uuid4().hex[:8]}"

    # Run tasks with parallelism control
    semaphore = asyncio.Semaphore(max_parallel)

    async def run_with_semaphore(task_path: Path, variant_id: int) -> TaskResult:
        async with semaphore:
            return await run_single_task(
                task_path,
                task_index=variant_id,
                split=split,
                agent_fn=agent_fn,
                max_steps=max_steps,
                oracle=oracle,
            )

    # Create and run all tasks
    coroutines = [
        run_with_semaphore(task_path, variant_id) for task_path, variant_id in task_variants
    ]
    results = await asyncio.gather(*coroutines, return_exceptions=True)

    # Process results
    task_results: List[Dict[str, Any]] = []
    rewards: List[float] = []

    for (task_path, variant_id), result in zip(task_variants, results):
        if isinstance(result, BaseException):
            task_results.append(
                {
                    "task_path": str(task_path),
                    "variant_id": variant_id,
                    "success": False,
                    "reward": 0.0,
                    "steps": 0,
                    "error": str(result),
                }
            )
            rewards.append(0.0)
        else:
            # Validate worker provenance against the scheduled task before scoring.
            invalid = (
                not isinstance(result, TaskResult)
                or result.task_path != str(task_path)
                or result.variant_id != variant_id
                or type(result.success) is not bool
                or not isinstance(result.reward, (int, float))
                or isinstance(result.reward, bool)
                or not math.isfinite(result.reward)
                or not 0.0 <= result.reward <= 1.0
                or not isinstance(result.steps, int)
                or isinstance(result.steps, bool)
                or result.steps < 0
                or (
                    result.action_trace_digest is not None
                    and (
                        not isinstance(result.action_trace_digest, str)
                        or len(result.action_trace_digest) != 64
                        or any(c not in "0123456789abcdef" for c in result.action_trace_digest)
                        or result.steps == 0
                    )
                )
                or (
                    result.action_trace_digest is not None
                    and (
                        not isinstance(result.action_trace_events, list)
                        or len(result.action_trace_events) != result.steps
                        or any(
                            not isinstance(event, dict)
                            or set(event) != {"step", "action_type"}
                            or type(event["step"]) is not int
                            or event["step"] != index
                            or not isinstance(event["action_type"], str)
                            or not event["action_type"].isidentifier()
                            for index, event in enumerate(result.action_trace_events, start=1)
                        )
                        or hashlib.sha256(
                            json.dumps(result.action_trace_events, sort_keys=True, separators=(",", ":")).encode("utf-8")
                        ).hexdigest() != result.action_trace_digest
                    )
                )
                or (result.action_trace_digest is None and result.action_trace_events is not None)
                or (bool(result.error) and (result.success or result.reward != 0.0))
                or (not result.error and result.success != (result.reward >= 0.5))
            )
            if invalid:
                task_results.append(
                    {
                        "task_path": str(task_path),
                        "variant_id": variant_id,
                        "success": False,
                        "reward": 0.0,
                        "steps": 0,
                        "error": "Invalid or misattributed worker result",
                    }
                )
                rewards.append(0.0)
                continue
            task_results.append(
                {
                    "task_path": str(task_path),
                    "variant_id": variant_id,
                    "success": result.success,
                    "reward": result.reward,
                    "steps": result.steps,
                    "error": result.error,
                    "action_trace_digest": result.action_trace_digest,
                    "action_trace_events": result.action_trace_events,
                }
            )
            rewards.append(result.reward)

    # Calculate statistics
    success_count = sum(1 for r in task_results if r.get("success", False))
    failed_count = len(task_results) - success_count
    avg_reward = sum(rewards) / len(rewards) if rewards else 0.0
    duration_seconds = time.time() - start_time

    return BenchmarkResult(
        run_id=run_id,
        task_results=task_results,
        total_tasks=len(task_results),
        success_count=success_count,
        failed_count=failed_count,
        avg_reward=avg_reward,
        duration_seconds=duration_seconds,
        dataset_manifest_sha256=manifest_sha256,
    )


async def run_interactive(
    env_path: Path,
    task_index: int = 0,
    *,
    split: str = "train",
    headless: bool = False,
) -> Tuple[Environment, bytes, Task]:
    """Run an environment interactively using the gym interface.

    This function sets up an environment for interactive use, returning
    the environment instance, initial screenshot, and task configuration.

    Args:
        env_path: Path to the environment directory
        task_index: Task variant index (default: 0)
        split: Dataset split (default: "train")
        headless: Run in headless mode (default: False)

    Returns:
        Tuple of (env, screenshot, task_config)
        - env: Environment instance (caller should call env.close() when done)
        - screenshot: Initial screenshot bytes
        - task_config: Task configuration

    Example:
        env, screenshot, task_cfg = await run_interactive(Path("./task"))
        print(f"Task: {task_cfg.description}")

        # Execute actions...
        screenshot = await env.step(ClickAction(x=100, y=200))

        # Evaluate
        reward = await env.evaluate()
        print(f"Reward: {reward}")

        # Cleanup
        await env.close()
    """
    # Create environment
    env = make(str(env_path), split=split)
    env.headless = headless

    # Reset and get initial state
    screenshot, task_cfg = await env.reset(task_id=task_index)

    return env, screenshot, task_cfg
