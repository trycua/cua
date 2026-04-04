from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

from PIL import Image


@dataclass
class Step:
    step_num: int
    # Screenshot (observation) visible to the model at this step.
    screenshot: Optional[Image.Image]
    # Raw text reconstructed from the ComputerAgent output items (thinking + action).
    action_text: str


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

    The SDK returns a list of typed blocks such as:
      - ``{"type": "message", "content": [{"type": "output_text", "text": "..."}]}``
      - ``{"type": "computer_call", "action": {...}, "call_id": "..."}``
      - ``{"type": "tool_use", "name": "click", "input": {...}}``

    We join all text content and JSON-serialised tool/computer calls so the
    tokenizer receives a single string that faithfully represents the model's
    response.
    """
    parts: list[str] = []
    for item in output_items:
        kind = item.get("type")
        if kind == "message":
            for block in item.get("content", []):
                if isinstance(block, dict):
                    text = block.get("text")
                    if text:
                        parts.append(text)
        elif kind == "computer_call":
            parts.append(json.dumps({"action": item.get("action")}))
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

    raw_result = json.loads(by_event["evaluate"][0]["data_json"]).get("result", 0.0)
    # result may be a list (e.g. [0.0]) or a scalar
    if isinstance(raw_result, list):
        terminal_reward = float(raw_result[0]) if raw_result else 0.0
    else:
        terminal_reward = float(raw_result)

    # Task description comes from the reset event.
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
    prev_screenshot = reset_screenshot

    for i, row in enumerate(agent_steps):
        data = json.loads(row["data_json"])
        images = row.get("data_images") or []
        post_screenshot = _as_pil(images[0]) if images else None

        steps.append(
            Step(
                step_num=data.get("step", i + 1),
                screenshot=prev_screenshot,
                action_text=_extract_action_text(data.get("output", [])),
            )
        )

        # Advance screenshot cursor: use post_screenshot if available.
        if post_screenshot is not None:
            prev_screenshot = post_screenshot

    trajectory_id = ds[0]["trajectory_id"] if len(ds) > 0 and ds[0]["trajectory_id"] is not None else ""
    assert isinstance(trajectory_id, str), f"trajectory_id is not a string: {trajectory_id}"

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
    for trace_dir in sorted(run_path.glob("*/task_*_trace")):
        try:
            ep = load_episode(trace_dir)
            if ep is not None:
                episodes.append(ep)
        except Exception as exc:
            # Non-fatal: log and continue so one bad trace doesn't abort the epoch.
            print(f"[traces] Warning: could not load {trace_dir}: {exc}")
    return episodes


def load_runs(run_dirs: list[str | Path]) -> list[Episode]:
    """Load episodes from multiple run directories and merge them.

    This is needed for GRPO where group_size > 1 requires multiple
    trajectories per task, collected across repeated rollouts.
    """
    all_episodes: list[Episode] = []
    for run_dir in run_dirs:
        all_episodes.extend(load_run(run_dir))
    return all_episodes
