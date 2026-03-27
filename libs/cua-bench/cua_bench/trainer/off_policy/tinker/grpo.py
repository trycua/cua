from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from tinker import SamplingClient, TrainingClient
    import tinker.types as tt

    from .traces import Episode

from PIL import Image

from tinker_cookbook import renderers

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


@dataclass
class GRPOConfig:
    # Discount factor (kept for compatibility; not used with trajectory-level rewards).
    gamma: float = 1.0
    # AdamW hyperparameters.
    learning_rate: float = 1e-5
    beta1: float = 0.9
    beta2: float = 0.95
    eps: float = 1e-8
    # PPO clip thresholds (kept for potential future clipped variants).
    clip_low: float = 0.8
    clip_high: float = 1.2
    # Maximum number of recent images to keep per trajectory message.
    max_images: int = 3


# ---------------------------------------------------------------------------
# Advantage estimation (GRPO-style: trajectory-level, group-relative)
# ---------------------------------------------------------------------------


def _group_episodes_by_task(episodes: list["Episode"]) -> dict[str, list["Episode"]]:
    """Group episodes by their task_description (the 'task' that was rolled out)."""
    groups: dict[str, list["Episode"]] = {}
    for ep in episodes:
        groups.setdefault(ep.task_description, []).append(ep)
    return groups


def _trajectory_advantage(
    episode: "Episode",
    task_episodes: list["Episode"],
) -> float:
    """GRPO-style advantage: reward - mean(rewards across trajectories for this task)."""
    mean_reward = sum(e.terminal_reward for e in task_episodes) / len(task_episodes)
    return episode.terminal_reward - mean_reward


# ---------------------------------------------------------------------------
# Message construction (latest N images from trajectory)
# ---------------------------------------------------------------------------


def _build_trajectory_messages(
    episode: "Episode",
    max_images: int = 3,
) -> list[dict]:
    """Build a chat-style message list for the full trajectory.

    Keeps the latest ``max_images`` screenshots so the prompt stays bounded.
    Each step contributes its screenshot (the observation the model saw)
    and the action_text it produced.  The task description is included as the
    initial user instruction.
    """
    # Collect (image, action_text) pairs from steps that have screenshots
    step_entries: list[tuple[Image.Image, str]] = []
    for step in episode.steps:
        if step.screenshot is not None:
            step_entries.append((step.screenshot, step.action_text))

    # Keep only the latest max_images steps
    if len(step_entries) > max_images:
        step_entries = step_entries[-max_images:]

    # Build the message list
    content_parts: list[dict] = []

    # Task instruction as text
    content_parts.append({"type": "text", "text": episode.task_description})

    # Add image + action pairs
    for img, action_text in step_entries:
        content_parts.append({"type": "image", "image": img.convert("RGB")})
        content_parts.append({"type": "text", "text": action_text})

    messages = [{"role": "user", "content": content_parts}]
    return messages


# ---------------------------------------------------------------------------
# Datum construction (matches GRPO reference pattern)
# ---------------------------------------------------------------------------


def _build_datum(
    prompt: "tt.ModelInput",
    advantage: float,
    ref_logprobs: list[float],
    action_ids: list[int],
) -> Optional["tt.Datum"]:
    """Assemble a single Tinker Datum for one trajectory.

    Layout follows the GRPO reference:
        model_input = prompt.append(action_tokens)
        target_tokens = [0]*ob_len + action_ids
        logprobs      = [0]*ob_len + ref_logprobs
        advantages    = [0]*ob_len + [advantage]*action_len
    """
    import torch
    import tinker.types as tt
    from tinker.types.tensor_data import TensorData

    if not action_ids:
        return None

    ob_len = prompt.length - 1

    # Full model input: prompt + action tokens (shifted by 1 for next-token prediction)
    model_input = prompt.append(tt.EncodedTextChunk(tokens=action_ids[:-1]))

    # Aligned training arrays (padded over the observation prefix)
    target_tokens = [0] * ob_len + action_ids
    padded_logprobs = [0.0] * ob_len + ref_logprobs
    padded_advantages = [0.0] * ob_len + [advantage] * len(action_ids)

    assert model_input.length == len(target_tokens) == len(padded_logprobs) == len(padded_advantages), (
        f"model_input.length: {model_input.length}, "
        f"len(target_tokens): {len(target_tokens)}, "
        f"len(padded_logprobs): {len(padded_logprobs)}, "
        f"len(padded_advantages): {len(padded_advantages)}"
    )

    return tt.Datum(
        model_input=model_input,
        loss_fn_inputs={
            "target_tokens": TensorData.from_torch(torch.tensor(target_tokens)),
            "logprobs": TensorData.from_torch(torch.tensor(padded_logprobs)),
            "advantages": TensorData.from_torch(torch.tensor(padded_advantages)),
        },
    )


# ---------------------------------------------------------------------------
# Batch construction (GRPO: trajectory-level, group-relative advantage)
# ---------------------------------------------------------------------------


def build_batch(
    renderer: "renderers.Renderer",
    episodes: list["Episode"],
    sampling_client: "SamplingClient",
    sampling_params: "tt.SamplingParams",
    tokenizer,
    gamma: float = 1.0,
    max_images: int = 3,
) -> list["tt.Datum"]:
    """Convert episodes into a training batch (GRPO-style).

    For each task, episodes are grouped and advantage is computed as:
        advantage = episode.terminal_reward - mean(task_rewards)

    Each trajectory is converted to a message list (with the latest
    ``max_images`` screenshots), rendered into a prompt via the renderer,
    and then used to build a Datum.

    Parameters
    ----------
    renderer:
        Tinker cookbook renderer for converting messages to ModelInput prompts.
    episodes:
        Collected rollout episodes from ``traces.load_run``.
    sampling_client:
        Tinker ``SamplingClient`` from the current policy snapshot.
    sampling_params:
        Sampling parameters for reference log-prob computation.
    tokenizer:
        HuggingFace tokenizer matching the base model.
    gamma:
        Not used directly (trajectory-level rewards), kept for API compat.
    max_images:
        Maximum number of recent images to include per trajectory.

    Returns
    -------
    List of ``tinker.types.Datum`` ready for ``train_step``.
    """
    task_groups = _group_episodes_by_task(episodes)

    # --- Build prompts and submit reference log-prob requests in bulk ---
    # Each entry: (episode, advantage, prompt, action_ids, future)
    pending: list[tuple] = []

    for task_desc, task_episodes in task_groups.items():
        for ep in task_episodes:
            advantage = _trajectory_advantage(ep, task_episodes)

            # Skip if advantage is exactly zero (all same reward in group)
            if advantage == 0.0:
                continue

            # Build trajectory-level messages with latest N images
            messages = _build_trajectory_messages(ep, max_images=max_images)

            # Use renderer to convert messages into a ModelInput prompt
            prompt = renderer.build_generation_prompt(messages)

            # Tokenize the last action text as the "action" to train on
            last_step = ep.steps[-1] if ep.steps else None
            if last_step is None:
                continue
            action_ids = tokenizer.encode(last_step.action_text, add_special_tokens=False)
            if not action_ids:
                continue

            # Submit async — do NOT call .result() yet; batch all futures first.
            future = sampling_client.sample(
                prompt=prompt,
                num_samples=1,
                sampling_params=sampling_params,
            )
            pending.append((ep, advantage, prompt, action_ids, future))

    # --- Collect futures and build datums ---
    datums: list = []
    skipped_zero_advantage = 0

    for ep, advantage, prompt, action_ids, future in pending:
        sample_result = future.result()
        # Extract logprobs from the sampling response
        ref_logprobs: list[float] = []
        if sample_result.sequences:
            seq = sample_result.sequences[0]
            if seq.logprobs is not None:
                ref_logprobs = seq.logprobs

        # Pad or truncate ref_logprobs to match action_ids length
        if len(ref_logprobs) < len(action_ids):
            ref_logprobs = ref_logprobs + [0.0] * (len(action_ids) - len(ref_logprobs))
        elif len(ref_logprobs) > len(action_ids):
            ref_logprobs = ref_logprobs[: len(action_ids)]

        datum = _build_datum(prompt, advantage, ref_logprobs, action_ids)
        if datum is not None:
            datums.append(datum)

    if skipped_zero_advantage > 0:
        print(f"[grpo] Skipped {skipped_zero_advantage} datums with zero advantage")

    return datums


# ---------------------------------------------------------------------------
# Training step (follows GRPO reference pattern)
# ---------------------------------------------------------------------------


def train_step(
    training_client: "TrainingClient",
    datums: list["tt.Datum"],
    config: GRPOConfig,
) -> dict[str, float]:
    """Execute one gradient update via Tinker using importance_sampling loss.

    Submits ``forward_backward`` and ``optim_step`` back-to-back before
    blocking on either.  Tinker processes operations in discrete ~10 s
    clock cycles; submitting both futures in the same cycle avoids paying
    a full cycle penalty for the optimiser step.

    Returns
    -------
    Dict with ``"loss"`` and ``"n_datums"`` for logging.
    """
    import tinker.types as tt

    if not datums:
        return {"loss": 0.0, "n_datums": 0}

    adam_params = tt.AdamParams(
        learning_rate=config.learning_rate,
        beta1=config.beta1,
        beta2=config.beta2,
        eps=config.eps,
    )

    # Submit both futures before blocking — lands in same Tinker cycle
    fwd_bwd_future = training_client.forward_backward(
        datums, loss_fn="importance_sampling"
    )
    optim_step_future = training_client.optim_step(adam_params)

    # Block and collect results
    fwd_bwd_result = fwd_bwd_future.result()
    optim_result = optim_step_future.result()

    loss = getattr(fwd_bwd_result, "loss", float("nan"))

    metrics: dict[str, float] = {"loss": loss, "n_datums": len(datums)}
    if optim_result.metrics:
        metrics.update(optim_result.metrics)

    return metrics
