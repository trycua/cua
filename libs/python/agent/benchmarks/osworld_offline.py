#!/usr/bin/env python3
"""
Offline OSWorld-G evaluation runner.

Implements run_offline_dataset(dataset, split, **agent_kwargs) that evaluates a ComputerAgent
against an offline dataset by wiring a mock computer tool that surfaces the row image via
screenshot() and tracks clicks against the provided bounding box.

Dataset default: "MMInstruction/OSWorld-G"
Each row is expected to have keys:
  - "instructions": str               # instruction text
  - "image": PIL.Image.Image          # screenshot image
  - "image_size": Tuple[int, int]     # (width, height)
  - "box_coordinates": Tuple[int,int,int,int]  # (x, y, w, h)

The mock tool exposes methods: screenshot, click, get_dimensions, get_environment.
We run agent.run(instructions) and consume up to 5 steps.
We mark success if any click landed within the target bbox.
"""

from __future__ import annotations

import asyncio
import base64
import io
from dataclasses import dataclass
from typing import Any, Dict, List, Optional, Tuple

from datasets import load_dataset
from PIL import Image

# Import ComputerAgent (v0.4.x style)
from agent import ComputerAgent  # type: ignore

@dataclass
class RowContext:
    image: Image.Image
    image_size: Tuple[int, int]
    bbox_xywh: Tuple[int, int, int, int]


def make_offline_computer(row_ctx: RowContext) -> Tuple[Dict[str, Any], Dict[str, Any]]:
    """
    Create a dict-based mock computer tool compatible with CustomComputerHandler.

    Returns a tuple of (functions_dict, state_dict) where state_dict captures
    click_history and clicked_in_bbox for post-run evaluation.
    """
    state: Dict[str, Any] = {
        "click_history": [],
        "clicked_in_bbox": False,
    }

    # Functions expected by CustomComputerHandler
    def screenshot_func() -> Image.Image:
        # Return PIL Image; handler converts to base64
        return row_ctx.image

    def click_func(x: int, y: int, button: str = "left") -> None:
        state["click_history"].append((int(x), int(y)))
        bx, by, bw, bh = row_ctx.bbox_xywh
        if bx <= x <= bx + bw and by <= y <= by + bh:
            state["clicked_in_bbox"] = True

    def get_environment_func() -> str:
        return "linux"

    def get_dimensions_func() -> Tuple[int, int]:
        return (int(row_ctx.image_size[0]), int(row_ctx.image_size[1]))

    functions: Dict[str, Any] = {
        "screenshot": screenshot_func,
        # For getters, CustomComputerHandler checks both 'get_{attr}' and '{attr}'
        "environment": get_environment_func,
        "dimensions": get_dimensions_func,
        "click": click_func,
    }

    return functions, state


async def run_offline_dataset(
    dataset: str = "MMInstruction/OSWorld-G",
    split: str = "test",
    max_turns: int = 5,
    **agent_kwargs: Any,
) -> Dict[str, Any]:
    """
    Evaluate a ComputerAgent on an offline dataset with a mock computer tool.

    Args:
      dataset: HF dataset path or local dataset identifier (default: "MMInstruction/OSWorld-G")
      split: dataset split to use (default: "test")
      max_turns: maximum number of agent steps to consume per example (default: 5)
      **agent_kwargs: any ComputerAgent kwargs (e.g., model="...", etc.)
                      If 'tools' is provided, the mock computer will be appended.

    Returns:
      A dict with summary metrics and per-row results.
    """
    # Load dataset
    ds = load_dataset(dataset, split=split)

    # Prepare results
    total = 0
    successes = 0
    per_row: List[Dict[str, Any]] = []

    for idx, row in enumerate(ds):
        total += 1
        # Extract expected fields
        # Some datasets package image as PIL already; if not, convert
        img: Image.Image = row["image"]
        if not isinstance(img, Image.Image):
            # Attempt to convert if the dataset returns arrays
            img = Image.fromarray(img)
        img_size: Tuple[int, int] = tuple(row["image_size"])  # (w, h)
        bbox_xywh: Tuple[int, int, int, int] = tuple(row["box_coordinates"])  # (x,y,w,h)
        instruction: str = row["instructions"]

        row_ctx = RowContext(image=img, image_size=(int(img_size[0]), int(img_size[1])), bbox_xywh=(
            int(bbox_xywh[0]), int(bbox_xywh[1]), int(bbox_xywh[2]), int(bbox_xywh[3])
        ))
        functions, state = make_offline_computer(row_ctx)

        # Build tools list from kwargs + our mock
        tools = list(agent_kwargs.get("tools", []))
        tools.append(functions)

        # Build agent kwargs: ensure tools
        ak = dict(agent_kwargs)
        ak["tools"] = tools

        # Instantiate agent
        agent = ComputerAgent(**ak)  # expects model=..., tools=[...]

        # Run the agent for up to max_turns steps
        # The API supports passing a plain string or a message list; follow the user's instruction style.
        step_count = 0
        async for _ in agent.run(instruction):  # type: ignore
            step_count += 1
            if step_count >= max_turns:
                break

        success = bool(state["clicked_in_bbox"])  # type: ignore
        if success:
            successes += 1

        per_row.append({
            "index": idx,
            "instruction": instruction,
            "clicked_in_bbox": success,
            "num_turns": step_count,
            "clicks": list(state["click_history"]),  # type: ignore
            "bbox_xywh": list(bbox_xywh),
            "image_size": list(img_size),
        })

    accuracy = successes / total if total else 0.0
    summary = {
        "dataset": dataset,
        "split": split,
        "total": total,
        "successes": successes,
        "accuracy": accuracy,
    }

    return {"summary": summary, "results": per_row}


async def _demo():
    """Simple demo entrypoint for quick local testing via `python osworld_offline.py`."""
    # Example: minimal required kwargs include a model string
    out = await run_offline_dataset(
        dataset="MMInstruction/OSWorld-G",
        split="test",
        model="anthropic/claude-3-5-sonnet-20241022",
    )
    print(out["summary"])  # noqa: T201


if __name__ == "__main__":
    asyncio.run(_demo())
