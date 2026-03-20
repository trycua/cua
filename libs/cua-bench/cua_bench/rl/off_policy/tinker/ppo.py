"""Compute PPO training updates from cua-bench episodes via the Tinker API.

Responsibility: given a list of ``Episode`` objects and a Tinker
``TrainingClient``, execute one PPO gradient step and return training stats.

Pipeline
--------
1. Compute per-step discounted returns with a global mean baseline
   (Monte Carlo advantage, no value network required).
2. For each step, tokenise the action text and call
   ``ref_client.compute_logprobs_async`` to obtain the reference policy's
   log-probabilities (needed for the PPO importance-sampling ratio).
3. Assemble Tinker ``Datum`` objects: multimodal model input
   (text prompt + screenshot image + action tokens) with per-token advantage
   weights and reference log-probs.
4. Submit ``forward_backward_async`` + ``optim_step_async`` back-to-back
   without awaiting between them so both land in the same Tinker ~10 s clock
   cycle, maximising GPU utilisation.

Simplifications (documented for future work)
---------------------------------------------
- The reference log-prob computation uses text-only context (no image prefix)
  as an approximation.  A full VLM reference computation would pass the
  pre-action screenshot through the model as well.
- Advantages are normalised with a global mean across the entire batch; a
  per-task baseline would be more accurate when task difficulties differ greatly.
- Negative advantages are zero-clamped on the per-token weight rather than
  discarded, so the datum still participates in the batch norm computation.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    # Imported at runtime inside each function to avoid hard dependency at
    # import time (tinker may not be installed in all environments).
    from tinker import TrainingClient, SamplingClient
    import tinker.types as tt
    from transformers import PreTrainedTokenizerBase

    from .traces import Episode, Step


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


@dataclass
class PPOConfig:
    # Discount factor for Monte Carlo returns.
    gamma: float = 0.99
    # PPO clip thresholds applied to the importance-sampling ratio.
    clip_low: float = 0.8
    clip_high: float = 1.2
    # AdamW hyperparameters.
    learning_rate: float = 1e-5
    betas: tuple[float, float] = (0.9, 0.999)
    weight_decay: float = 0.01
    grad_clip: float = 1.0


# ---------------------------------------------------------------------------
# Advantage estimation
# ---------------------------------------------------------------------------


def _discounted_returns(rewards: list[float], gamma: float) -> list[float]:
    """Compute per-step Monte Carlo discounted returns (backward pass)."""
    G = 0.0
    returns: list[float] = []
    for r in reversed(rewards):
        G = r + gamma * G
        returns.insert(0, G)
    return returns


def _global_mean_baseline(episodes: list, gamma: float) -> float:
    """Compute the mean return across all steps and episodes for baseline subtraction."""
    total, count = 0.0, 0
    for ep in episodes:
        rewards = [s.reward for s in ep.steps]
        for G in _discounted_returns(rewards, gamma):
            total += G
            count += 1
    return total / count if count else 0.0


# ---------------------------------------------------------------------------
# Datum construction
# ---------------------------------------------------------------------------


def _build_datum(
    step: "Step",
    advantage: float,
    ref_logprobs: list[float],
    action_ids: list[int],
    tokenizer: "PreTrainedTokenizerBase",
) -> Optional["tt.Datum"]:
    """Assemble a single Tinker Datum for one agent step.

    Model input layout (token sequence fed to the GPU):
        [system_tokens]  [ImageChunk]  [action_tokens]
                                       ↑ loss computed here

    The per-token weight is ``max(0, advantage)`` so steps with negative
    advantage still participate in the batch norm but contribute no gradient
    push in the positive direction.  The reference log-probs are passed
    alongside so Tinker's PPO loss can compute the IS ratio
    π_θ(a|s) / π_ref(a|s) for each token.
    """
    import tinker.types as tt

    if not action_ids:
        return None

    system_ids = tokenizer.encode(
        "You are a computer-use assistant. Complete the task shown on screen.",
        add_special_tokens=True,
    )

    model_input = tt.ModelInput(
        chunks=[
            tt.EncodedTextChunk(token_ids=system_ids),
            tt.ImageChunk(image=step.pre_screenshot),
            tt.EncodedTextChunk(token_ids=action_ids),
        ]
    )

    per_token_weight = max(0.0, advantage)
    weights = [per_token_weight] * len(action_ids)

    loss_fn_inputs = tt.LossFnInputs(
        target_token_ids=action_ids,
        weights=weights,
        reference_logprobs=ref_logprobs,
    )

    return tt.Datum(model_input=model_input, loss_fn_inputs=loss_fn_inputs)


# ---------------------------------------------------------------------------
# Batch construction
# ---------------------------------------------------------------------------


def build_batch(
    episodes: list["Episode"],
    ref_client: "SamplingClient",
    tokenizer: "PreTrainedTokenizerBase",
    gamma: float = 0.99,
) -> list["tt.Datum"]:
    """Convert a list of episodes into a PPO training batch.

    Reference log-probs are fetched in bulk using async calls to keep all
    requests within the same Tinker processing cycle.  The ``ref_client``
    must remain frozen (never updated) so it represents the sampling policy
    used during rollout collection; updating it would invalidate the IS ratio.

    Parameters
    ----------
    episodes:
        Collected rollout episodes from ``traces.load_run``.
    ref_client:
        Frozen Tinker ``SamplingClient`` representing the rollout policy.
    tokenizer:
        HuggingFace tokenizer matching the base model.
    gamma:
        Discount factor for Monte Carlo return computation.

    Returns
    -------
    List of ``tinker.types.Datum`` objects ready to pass to ``train_step``.
    """
    baseline = _global_mean_baseline(episodes, gamma)

    # --- Tokenise all steps and submit reference log-prob requests in bulk ---
    # We pre-tokenise here so we can skip empty sequences before making API
    # calls, then submit all futures before collecting any results.

    system_ids = tokenizer.encode(
        "You are a computer-use assistant. Complete the task shown on screen.",
        add_special_tokens=True,
    )

    # Each entry: (step, advantage, action_ids, future_ref_logprobs)
    pending: list[tuple] = []

    for ep in episodes:
        rewards = [s.reward for s in ep.steps]
        returns = _discounted_returns(rewards, gamma)
        for step, G in zip(ep.steps, returns):
            advantage = G - baseline
            action_ids = tokenizer.encode(step.action_text, add_special_tokens=False)
            if not action_ids:
                continue
            # Submit async; do NOT call .result() yet — batch all futures first.
            future = ref_client.compute_logprobs_async(
                prompt_token_ids=system_ids,
                target_token_ids=action_ids,
            )
            pending.append((step, advantage, action_ids, future))

    # --- Collect reference log-probs and build datums ---
    datums: list = []
    for step, advantage, action_ids, future in pending:
        ref_logprobs = future.result()
        datum = _build_datum(step, advantage, ref_logprobs, action_ids, tokenizer)
        if datum is not None:
            datums.append(datum)

    return datums


# ---------------------------------------------------------------------------
# Training step
# ---------------------------------------------------------------------------


def train_step(
    training_client: "TrainingClient",
    datums: list["tt.Datum"],
    config: PPOConfig,
) -> dict[str, float]:
    """Execute one PPO gradient update via Tinker.

    Submits ``forward_backward_async`` and ``optim_step_async`` back-to-back
    without awaiting between them.  Tinker processes operations in discrete
    ~10 s clock cycles; submitting both futures before blocking on either
    ensures they land in the same cycle and avoids paying a full cycle penalty
    for the optimiser step.

    Returns
    -------
    Dict with ``"loss"`` and ``"n_datums"`` for logging.
    """
    import tinker.types as tt

    if not datums:
        return {"loss": 0.0, "n_datums": 0}

    fwdbwd_future = training_client.forward_backward_async(
        data=datums,
        loss_fn="ppo",
        loss_fn_params=tt.PpoParams(
            clip_low_threshold=config.clip_low,
            clip_high_threshold=config.clip_high,
        ),
    )

    optim_future = training_client.optim_step_async(
        tt.AdamParams(
            learning_rate=config.learning_rate,
            betas=config.betas,
            weight_decay=config.weight_decay,
            grad_clip=config.grad_clip,
        )
    )

    # Now block: collect both results to surface any server-side errors.
    fwdbwd_result = fwdbwd_future.result()
    optim_future.result()

    loss = getattr(fwdbwd_result, "loss", float("nan"))
    return {"loss": loss, "n_datums": len(datums)}
