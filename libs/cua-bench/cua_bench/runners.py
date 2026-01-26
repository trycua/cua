"""Benchmark runner functions for cua-bench.

This module provides programmatic interfaces for running benchmarks and
interactive environments, using the core gym interface (make, reset, step, evaluate).
"""

import asyncio
import fnmatch
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

from .core import Task, make
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
                return TaskResult(
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
                done = isinstance(action, DoneAction)

        # Evaluate
        if env.evaluate_task_fn is not None:
            result = await env.evaluate()
            if isinstance(result, (int, float)):
                reward = float(result)
            elif isinstance(result, list) and len(result) > 0:
                reward = float(result[0])
            elif isinstance(result, dict) and "reward" in result:
                reward = float(result["reward"])

        return TaskResult(
            task_path=str(env_path),
            variant_id=task_index,
            success=reward >= 0.5,  # Common threshold
            reward=reward,
            steps=step_count,
        )

    except Exception as e:
        return TaskResult(
            task_path=str(env_path),
            variant_id=task_index,
            success=False,
            reward=0.0,
            steps=0,
            error=str(e),
        )
    finally:
        if env is not None:
            try:
                await env.close()
            except Exception:
                pass


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
    start_time = time.time()

    # Validate dataset path
    if not dataset_path.exists():
        raise FileNotFoundError(f"Dataset not found: {dataset_path}")

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
        except Exception:
            variant_count = 1

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

    for result in results:
        if isinstance(result, Exception):
            task_results.append(
                {
                    "task_path": "unknown",
                    "variant_id": -1,
                    "success": False,
                    "reward": 0.0,
                    "steps": 0,
                    "error": str(result),
                }
            )
            rewards.append(0.0)
        else:
            task_results.append(
                {
                    "task_path": result.task_path,
                    "variant_id": result.variant_id,
                    "success": result.success,
                    "reward": result.reward,
                    "steps": result.steps,
                    "error": result.error,
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
