from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from tinker import SamplingClient, TrainingClient
    import tinker.types as tt

    from .traces import Episode

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


def _retained_steps(episode: "Episode", max_images: int = 3) -> list:
    """Return the latest ``max_images`` steps that have a screenshot."""
    steps = [s for s in episode.steps if s.screenshot is not None]
    if len(steps) > max_images:
        steps = steps[-max_images:]
    return steps


def _build_messages_up_to_obs(
    episode: "Episode",
    steps: list,
    up_to: int,
) -> list[dict]:
    """Build messages including obs+actions for steps[:up_to] and only the
    observation (screenshot) at steps[up_to].  The action at ``up_to`` is
    excluded — it becomes the training target.
    """
    content: list[dict] = [{"type": "text", "text": episode.task_description}]
    for k, step in enumerate(steps):
        if k < up_to:
            content.append({"type": "image", "image": step.screenshot.convert("RGB")})
            content.append({"type": "text", "text": step.action_text})
        elif k == up_to:
            content.append({"type": "image", "image": step.screenshot.convert("RGB")})
            break
    return [{"role": "user", "content": content}]


# ---------------------------------------------------------------------------
# Datum construction (multi-turn, per-token masking)
# ---------------------------------------------------------------------------


def _build_datum(
    model_input: "tt.ModelInput",
    target_tokens: list[int],
    ref_logprobs: list[float],
    advantages: list[float],
) -> Optional["tt.Datum"]:
    """Assemble a single Tinker Datum for one trajectory.

    Unlike the single-action variant, this accepts pre-computed per-token
    arrays so that callers can mask observation tokens (advantage = 0) and
    only train on action tokens scattered throughout the sequence.

    All four arrays must have the same length (``model_input.length``).
    """
    import tinker.types as tt
    import torch
    from tinker.types.tensor_data import TensorData

    if not any(a != 0.0 for a in advantages):
        return None

    assert model_input.length == len(target_tokens) == len(ref_logprobs) == len(advantages), (
        f"model_input.length: {model_input.length}, "
        f"len(target_tokens): {len(target_tokens)}, "
        f"len(ref_logprobs): {len(ref_logprobs)}, "
        f"len(advantages): {len(advantages)}"
    )

    return tt.Datum(
        model_input=model_input,
        loss_fn_inputs={
            "target_tokens": TensorData.from_torch(torch.tensor(target_tokens)),
            "logprobs": TensorData.from_torch(torch.tensor(ref_logprobs)),
            "advantages": TensorData.from_torch(torch.tensor(advantages)),
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
    """Convert episodes into a training batch (GRPO-style, multi-turn).

    For each task, episodes are grouped and advantage is computed as:
        advantage = episode.terminal_reward - mean(task_rewards)

    Every step's action tokens are trained on (not just the last step).
    Observation / system-prompt tokens are masked (advantage = 0).
    One datum is produced per trajectory.
    """

    task_groups = _group_episodes_by_task(episodes)

    # --- Phase 1: compute action spans and submit ref-logprob futures ---
    # Each entry stores everything needed to build a datum once futures resolve.
    PendingEntry = tuple  # (ep, advantage, steps, action_spans, base_prompt, future)
    pending: list[PendingEntry] = []

    for task_desc, task_episodes in task_groups.items():
        for ep in task_episodes:
            advantage = _trajectory_advantage(ep, task_episodes)

            steps = _retained_steps(ep, max_images=max_images)
            if not steps:
                continue

            # Compute action token spans via incremental rendering.
            # For each step k, render up to step k's observation to find
            # where its action tokens begin in the full sequence.
            action_spans: list[tuple[int, list[int]]] = []  # (target_start, action_ids)
            for k, step in enumerate(steps):
                obs_msgs = _build_messages_up_to_obs(ep, steps, k)
                obs_prompt = renderer.build_generation_prompt(obs_msgs)
                action_ids_k = tokenizer.encode(step.action_text, add_special_tokens=False)
                if action_ids_k:
                    # In the target array, position i predicts token i+1.
                    # The model at position (obs_len - 1) predicts the first
                    # action token, so the target starts at obs_len - 1.
                    action_spans.append((obs_prompt.length - 1, action_ids_k))

            if not action_spans:
                continue

            # Build model_input: TODO: check whether we need to render everything up to the last observation
            # (which includes all prior obs+actions), then append the last
            # step's action tokens (minus the final one for next-token shift).
            # base_prompt = renderer.build_generation_prompt(
            #     _build_messages_up_to_obs(ep, steps, len(steps) - 1)
            # )
            # last_action_ids = action_spans[-1][1]
            # model_input = base_prompt.append(
            #     tt.EncodedTextChunk(tokens=last_action_ids[:-1])
            # )

            # # Submit ref-logprob request with the full trajectory prompt
            # # (same pattern as the original single-action code).
            # future = sampling_client.sample(
            #     prompt=model_input,
            #     num_samples=1,
            #     sampling_params=sampling_params,
            # )
            # pending.append(
            #     (ep, advantage, steps, action_spans, model_input, future)
            # )

    # --- Phase 2: collect futures and assemble datums ---
    datums: list[tt.Datum] = []

    for ep, advantage, steps, action_spans, model_input, future in pending:
        total_len = model_input.length

        # Retrieve reference logprobs
        sample_result = future.result()
        ref_logprobs_raw: list[float] = []
        if sample_result.sequences:
            seq = sample_result.sequences[0]
            if seq.logprobs is not None:
                ref_logprobs_raw = list(seq.logprobs)

        # Pad / truncate to total action token count
        total_action_tokens = sum(len(aids) for _, aids in action_spans)
        if len(ref_logprobs_raw) < total_action_tokens:
            ref_logprobs_raw += [0.0] * (total_action_tokens - len(ref_logprobs_raw))
        elif len(ref_logprobs_raw) > total_action_tokens:
            ref_logprobs_raw = ref_logprobs_raw[:total_action_tokens]

        # Build per-token arrays (observation positions stay zero = masked)
        target_tokens = [0] * total_len
        per_token_logprobs = [0.0] * total_len
        per_token_advantages = [0.0] * total_len

        lp_offset = 0
        for target_start, action_ids_k in action_spans:
            for t, aid in enumerate(action_ids_k):
                pos = target_start + t
                if 0 <= pos < total_len:
                    target_tokens[pos] = aid
                    per_token_logprobs[pos] = ref_logprobs_raw[lp_offset + t]
                    per_token_advantages[pos] = advantage
            lp_offset += len(action_ids_k)

        datum = _build_datum(model_input, target_tokens, per_token_logprobs, per_token_advantages)
        if datum is not None:
            datums.append(datum)

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
        learning_rate=config.learning_rate, beta1=config.beta1, beta2=config.beta2, eps=config.eps
    )

    # Submit both futures before blocking — lands in same Tinker cycle
    fwd_bwd_future = training_client.forward_backward(datums, loss_fn="importance_sampling")
    optim_step_future = training_client.optim_step(adam_params)

    # Block and collect results
    fwd_bwd_result = fwd_bwd_future.result()
    optim_result = optim_step_future.result()

    loss = getattr(fwd_bwd_result, "loss", float("nan"))

    metrics: dict[str, float] = {"loss": loss, "n_datums": len(datums)}
    if optim_result.metrics:
        metrics.update(optim_result.metrics)

    return metrics
