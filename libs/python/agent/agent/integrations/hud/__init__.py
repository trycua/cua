"""HUD integration for ComputerAgent."""

import logging
from typing import Any, Optional, Dict
from hud import run_job as hud_run_job

from .agent import ComputerAgent
from .adapter import ComputerAgentAdapter
from .computer_handler import HUDComputerHandler


async def run_job(
    model: str,
    task_or_taskset: Any,
    job_name: str,
    # Job kwargs
    auto_reply_question: bool = False,
    adapter_cls: Any = None,
    adapter_kwargs: Optional[Dict[str, Any]] = None,
    max_steps_per_task: int = 20,
    run_parallel: bool = True,
    job_metadata: Optional[Dict[str, Any]] = None,
    show_progress: bool = True,
    max_concurrent_env_creations: Optional[int] = 30,  # Limits gym.make calls
    max_concurrent_agent_predictions: Optional[int] = None,  # No limit on LLM calls
    max_concurrent_tasks: Optional[int] = 30,  # Limits overall task concurrency
    **agent_kwargs: Any
) -> Any:
    """
    Run a job using ComputerAgent with the specified model.
    
    Args:
        model: Model string for ComputerAgent (e.g., "anthropic/claude-3-5-sonnet-20241022")
        task_or_taskset: Task or TaskSet to run
        job_name: Name for the job
        auto_reply_question: Whether to auto-reply to questions
        adapter_cls: Custom adapter class (defaults to ComputerAgentAdapter)
        adapter_kwargs: Additional kwargs for the adapter
        max_steps_per_task: Maximum steps per task
        run_parallel: Whether to run tasks in parallel
        job_metadata: Additional metadata for the job
        show_progress: Whether to show progress
        max_concurrent_env_creations: Max concurrent environment creations
        max_concurrent_agent_predictions: Max concurrent agent predictions
        max_concurrent_tasks: Max concurrent tasks
        **agent_kwargs: Additional kwargs to pass to ComputerAgent
    
    Returns:
        Job instance from HUD
    """
    # combine verbose and verbosity kwargs
    if "verbose" in agent_kwargs:
        agent_kwargs["verbosity"] = logging.INFO
        del agent_kwargs["verbose"]
    verbose = True if agent_kwargs.get("verbosity", logging.WARNING) > logging.INFO else False
    
    # run job
    return await hud_run_job(
        agent_cls=ComputerAgent,
        agent_kwargs={"model": model, **agent_kwargs},
        task_or_taskset=task_or_taskset,
        job_name=job_name,
        auto_reply_question=auto_reply_question,
        adapter_cls=adapter_cls,
        adapter_kwargs=adapter_kwargs,
        max_steps_per_task=max_steps_per_task,
        run_parallel=run_parallel,
        job_metadata=job_metadata,
        show_progress=show_progress,
        verbose=verbose,
        max_concurrent_env_creations=max_concurrent_env_creations,
        max_concurrent_agent_predictions=max_concurrent_agent_predictions,
        max_concurrent_tasks=max_concurrent_tasks
    )


__all__ = ["ComputerAgent", "ComputerAgentAdapter", "HUDComputerHandler", "run_job"]