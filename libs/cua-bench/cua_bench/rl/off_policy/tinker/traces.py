"""Load cua-bench HuggingFace Dataset traces into structured Episode / Step objects.

A single cua-bench task execution produces one HF Dataset directory containing
rows of tracing events emitted by ``cua_bench.tracing.Tracing``:

    reset       - initial screenshot after task setup
    agent_step  - model output + post-action screenshot at each step
    evaluate    - terminal reward returned by ``evaluate_task_fn``

The reward is sparse: only the last step of an episode carries the terminal
reward; all preceding steps get 0.0.

The pre-action screenshot for step N is the post-action screenshot of step N-1
(or the reset screenshot for step 0).  Both screenshots are stored on Step so
that callers can choose which visual context to feed the model.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

from PIL import Image


@dataclass
class Step:
    step_num: int
    # Screenshot visible to the model *before* it produced this action.
    pre_screenshot: Image.Image
    # Screenshot captured *after* the action was executed.
    post_screenshot: Image.Image
    # Raw text reconstructed from the ComputerAgent output items (thinking + action).
    action_text: str
    # Sparse reward: 0.0 for non-terminal steps; terminal_reward for the last step.
    reward: float


@dataclass
class Episode:
    trajectory_id: str
    task_description: str
    steps: list[Step]
    terminal_reward: float


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _extract_action_text(output_items: list[dict]) -> str:
    """Reconstruct printable text from ComputerAgent SDK output items.

    The SDK returns a list of typed blocks:
      - ``{"type": "message", "content": [{"text": "..."}]}``
      - ``{"type": "tool_use", "name": "click", "input": {...}}``

    We join all text content and JSON-serialised tool calls so the tokenizer
    receives a single string that faithfully represents the model's response.
    """
    parts: list[str] = []
    for item in output_items:
        kind = item.get("type")
        if kind == "message":
            for block in item.get("content", []):
                if isinstance(block, dict) and "text" in block:
                    parts.append(block["text"])
        elif kind == "tool_use":
            parts.append(
                json.dumps({"tool": item.get("name"), "input": item.get("input")})
            )
    # Fallback: if nothing matched, serialise the raw list so we always have
    # *something* to tokenise.
    return "\n".join(parts) if parts else json.dumps(output_items)


def _as_pil(img) -> Optional[Image.Image]:
    """Coerce an HF Dataset image value to PIL, or return None."""
    if img is None:
        return None
    if isinstance(img, Image.Image):
        return img
    # HF sometimes returns a dict with a "bytes" key when the image was saved
    # as a raw buffer rather than a decoded PIL image.
    if isinstance(img, dict) and "bytes" in img:
        import io
        return Image.open(io.BytesIO(img["bytes"])).convert("RGB")
    return None


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def load_episode(trace_dir: str | Path) -> Optional[Episode]:
    """Load a single task trace directory into an Episode.

    Returns ``None`` if the trace is missing the ``evaluate`` event (i.e. the
    task did not finish cleanly).
    """
    from datasets import load_from_disk

    ds = load_from_disk(str(trace_dir))

    # Group rows by event type for easy lookup.
    by_event: dict[str, list] = {}
    for row in ds:
        by_event.setdefault(row["event_name"], []).append(row)

    if "evaluate" not in by_event:
        return None

    terminal_reward = float(
        json.loads(by_event["evaluate"][0]["data_json"]).get("result", 0.0)
    )

    # Task description and initial screenshot come from the reset event.
    task_description = ""
    reset_screenshot: Optional[Image.Image] = None
    for reset_row in by_event.get("reset", []):
        reset_data = json.loads(reset_row["data_json"])
        task_description = reset_data.get("task", "")
        images = reset_row.get("data_images") or []
        if images:
            reset_screenshot = _as_pil(images[0])
        break

    # Sort agent steps by the step counter stored in data_json.
    agent_steps = sorted(
        by_event.get("agent_step", []),
        key=lambda r: json.loads(r["data_json"]).get("step", 0),
    )

    steps: list[Step] = []
    prev_screenshot = reset_screenshot  # carries forward between steps

    for i, row in enumerate(agent_steps):
        data = json.loads(row["data_json"])
        images = row.get("data_images") or []
        post_screenshot = _as_pil(images[0]) if images else None

        # We need a valid pre-screenshot to build a useful training datum.
        if prev_screenshot is None:
            # Skip steps without visual context and advance the cursor.
            if post_screenshot is not None:
                prev_screenshot = post_screenshot
            continue

        is_terminal = i == len(agent_steps) - 1
        steps.append(
            Step(
                step_num=data.get("step", i + 1),
                pre_screenshot=prev_screenshot,
                post_screenshot=post_screenshot or prev_screenshot,
                action_text=_extract_action_text(data.get("output", [])),
                reward=terminal_reward if is_terminal else 0.0,
            )
        )

        if post_screenshot is not None:
            prev_screenshot = post_screenshot

    trajectory_id = ds[0]["trajectory_id"] if len(ds) > 0 else ""
    return Episode(
        trajectory_id=trajectory_id,
        task_description=task_description,
        steps=steps,
        terminal_reward=terminal_reward,
    )


def load_run(run_dir: str | Path) -> list[Episode]:
    """Load all task trace directories from a cua-bench run output directory.

    Skips trace directories that are missing the evaluate event or whose
    dataset cannot be read (e.g. a task that crashed mid-run).
    """
    run_path = Path(run_dir)
    episodes: list[Episode] = []
    for trace_dir in sorted(run_path.glob("task_*_trace")):
        try:
            ep = load_episode(trace_dir)
            if ep is not None:
                episodes.append(ep)
        except Exception as exc:
            # Non-fatal: log and continue so one bad trace doesn't abort the epoch.
            print(f"[traces] Warning: could not load {trace_dir}: {exc}")
    return episodes
