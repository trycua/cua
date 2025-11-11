"""HUD integration: dataset runners and MCP-based computer agent export.

This module exposes helpers to evaluate HUD-compatible datasets and exports
the MCP-compatible computer agent implementation.

Exports:
- run_single_task(dataset, ...)
- run_full_dataset(dataset, ...)
- MCPComputerAgent
"""

import time
from typing import Any, Optional

from agent.computers import is_agent_computer
from datasets import Dataset, load_dataset
from hud import trace
from hud.datasets import Task, run_dataset

from .agent import MCPComputerAgent

# ---------------------------------------------------------------------------
# Single-task runner
# ---------------------------------------------------------------------------


async def run_single_task(
    dataset: str | Dataset | list[dict[str, Any]],
    *,
    task_id: int = 0,
    model: str | None = None,
    allowed_tools: list[str] | None = None,
    # === ComputerAgent kwargs ===
    tools: list[Any] | None = None,
    custom_loop: Any | None = None,
    only_n_most_recent_images: int | None = None,
    callbacks: list[Any] | None = None,
    instructions: str | None = None,
    verbosity: int | None = None,
    trajectory_dir: str | dict | None = None,
    max_retries: int | None = 3,
    screenshot_delay: float | int = 0.5,
    use_prompt_caching: bool | None = False,
    max_trajectory_budget: float | dict | None = None,
    telemetry_enabled: bool | None = True,
) -> None:
    """Load one task from the dataset and execute it with MCPComputerAgent."""

    # Load dataset and pick a sample
    if isinstance(dataset, str):
        dataset = load_dataset(dataset, split="train")  # type: ignore[arg-type]
    elif isinstance(dataset, list):
        dataset = dataset
    else:
        dataset = dataset["train"]

    sample_task = dataset[task_id]  # type: ignore[index]
    task_prompt = sample_task.get("prompt", f"Task {sample_task.get('id', 0)}")  # type: ignore[attr-defined]

    # Filter any existing Computer tools
    # The eval framework will add its own Computer tool per task
    if tools:
        tools = [tool for tool in tools if not is_agent_computer(tool)]

    with trace(name=task_prompt):
        task = Task(**sample_task)  # type: ignore[arg-type]

        agent = MCPComputerAgent(
            model=model or "computer-use-preview",
            allowed_tools=allowed_tools or ["openai_computer"],
            # === ComputerAgent kwargs passthrough ===
            tools=tools,
            custom_loop=custom_loop,
            only_n_most_recent_images=only_n_most_recent_images,
            callbacks=callbacks,
            instructions=instructions,
            verbosity=verbosity,
            trajectory_dir=trajectory_dir,
            max_retries=max_retries,
            screenshot_delay=screenshot_delay,
            use_prompt_caching=use_prompt_caching,
            max_trajectory_budget=max_trajectory_budget,
            telemetry_enabled=telemetry_enabled,
        )
        print(f"Running: {task_prompt}")
        result = await agent.run(task, max_steps=10)
        print(f"✅ Reward: {result.reward}")


# ---------------------------------------------------------------------------
# Full-dataset runner
# ---------------------------------------------------------------------------


async def run_full_dataset(
    dataset: str | Dataset | list[dict[str, Any]],
    *,
    job_name: Optional[str] = None,
    model: str | None = None,
    allowed_tools: list[str] | None = None,
    max_concurrent: int = 30,
    max_steps: int = 50,
    split: str = "train",
    trajectory_dir: str | dict | None = None,
    # === ComputerAgent kwargs ===
    tools: list[Any] | None = None,
    custom_loop: Any | None = None,
    only_n_most_recent_images: int | None = 5,
    callbacks: list[Any] | None = None,
    instructions: str | None = None,
    verbosity: int | None = None,
    max_retries: int | None = 3,
    screenshot_delay: float | int = 0.5,
    use_prompt_caching: bool | None = False,
    max_trajectory_budget: float | dict | None = None,
    telemetry_enabled: bool | None = True,
) -> list[Any]:
    """Run evaluation across the entire dataset using hud.datasets.run_dataset."""

    # Run with our MCP-based agent class.
    if isinstance(dataset, str):
        dataset_name = dataset.split("/")[-1]
        job_name = job_name or f"Evaluation {dataset_name}"
        dataset = load_dataset(dataset, split=split)  # type: ignore[arg-type]
    else:
        dataset_name = "custom"
        job_name = job_name or f"Evaluation {time.strftime('%H:%M %Y-%m-%d')}"

    # Filter any existing Computer tools
    # The eval framework will add its own Computer tool per task
    if tools:
        tools = [tool for tool in tools if not is_agent_computer(tool)]

    # Execute evaluation
    return await run_dataset(
        name=job_name,
        dataset=dataset,
        agent_class=MCPComputerAgent,
        agent_config={
            "model": model,
            "allowed_tools": allowed_tools,
            "trajectory_dir": trajectory_dir,
            # === ComputerAgent kwargs passthrough ===
            "tools": tools,
            "custom_loop": custom_loop,
            "only_n_most_recent_images": only_n_most_recent_images,
            "callbacks": callbacks,
            "instructions": instructions,
            "verbosity": verbosity,
            "max_retries": max_retries,
            "screenshot_delay": screenshot_delay,
            "use_prompt_caching": use_prompt_caching,
            "max_trajectory_budget": max_trajectory_budget,
            "telemetry_enabled": telemetry_enabled,
        },
        max_concurrent=max_concurrent,
        metadata={"dataset": dataset_name},
        max_steps=max_steps,
        auto_respond=True,
    )


__all__ = [
    "run_single_task",
    "run_full_dataset",
    "MCPComputerAgent",
]
