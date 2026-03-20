"""Main off-policy RL training loop using Tinker.

Entry point:  ``python -m cua_bench.rl.off_policy.tinker.loop``

The loop repeats until ``epochs`` is exhausted:

    1. **Rollout** — run ``cb run dataset`` against the current vLLM policy.
    2. **Load traces** — read HF Dataset traces from the run output directory.
    3. **Train** — compute PPO advantages, build Tinker Datums, run one update.
    4. **Sync** — push updated LoRA weights back to vLLM for the next rollout.
    5. **Checkpoint** — every ``checkpoint_every`` epochs, persist full state.

Training can be resumed from a Tinker checkpoint by setting ``resume_from``.
"""

from __future__ import annotations

import argparse
import os
import time
from dataclasses import dataclass, field
from typing import Optional

from .ppo import PPOConfig


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


@dataclass
class TrainingConfig:
    # --- Model ---
    base_model: str = "Qwen/Qwen3-8B"
    lora_rank: int = 16
    lora_seed: int = 42

    # --- Agent / rollout ---
    tasks_path: str = "tasks/"
    # Agent name passed to ``cb run dataset --agent``.
    agent: str = "opencua"
    # litellm model string that vLLM serves (must contain "OpenCUA" for the
    # SDK to route through the composed-grounded loop).
    model: str = "openai/opencua"
    max_steps_per_episode: int = 50
    vllm_base_url: str = "http://localhost:30000/v1"

    # --- Training loop ---
    epochs: int = 20
    # Save a full state checkpoint (weights + optimiser) every N epochs.
    checkpoint_every: int = 5

    # --- PPO hyperparameters ---
    ppo: PPOConfig = field(default_factory=PPOConfig)

    # --- Resume ---
    # tinker:// path to a previously saved checkpoint.  When set, training
    # resumes with both weights *and* optimiser state restored.
    resume_from: Optional[str] = None

    # --- Tinker ---
    # Falls back to the TINKER_API_KEY environment variable if None.
    tinker_api_key: Optional[str] = None

    # --- Rollout subprocess environment ---
    # Extra vars forwarded to the ``cb run dataset`` subprocess.
    # Use this to pass OPENCUA_BASE_URL, OPENAI_API_KEY, etc.
    rollout_env: dict[str, str] = field(default_factory=dict)


# ---------------------------------------------------------------------------
# Training loop
# ---------------------------------------------------------------------------


def run(config: TrainingConfig) -> None:
    """Execute the full RL training loop described in the module docstring.

    All Tinker imports are deferred to this function so the module can be
    imported in environments where ``tinker`` is not installed.
    """
    from tinker import ServiceClient
    import tinker.types as tt
    from transformers import AutoTokenizer

    from . import traces, rollout, ppo, checkpoints

    api_key = config.tinker_api_key or os.environ.get("TINKER_API_KEY")
    if not api_key:
        raise ValueError(
            "Tinker API key not found. "
            "Set TINKER_API_KEY in the environment or pass it via TrainingConfig."
        )

    service_client = ServiceClient(api_key=api_key)

    # --- Set up training client ---
    lora_config = tt.LoraConfig(rank=config.lora_rank, seed=config.lora_seed)

    if config.resume_from:
        print(f"[loop] Resuming training from checkpoint: {config.resume_from}")
        training_client = service_client.create_training_client_from_state_with_optimizer(
            checkpoint_path=config.resume_from
        )
    else:
        print(f"[loop] Starting fresh LoRA training on {config.base_model}")
        training_client = service_client.create_lora_training_client(
            base_model=config.base_model,
            lora_config=lora_config,
        )

    # --- Frozen reference client ---
    # Created once from the initial checkpoint (or base model) and never
    # updated.  It represents the policy that collected the rollouts and is
    # used to compute the PPO importance-sampling ratio π_θ / π_ref.
    ref_client = service_client.create_sampling_client(
        base_model=config.resume_from or config.base_model
    )

    # --- Tokenizer ---
    # Loaded directly from HuggingFace to avoid an extra Tinker API call.
    tokenizer = AutoTokenizer.from_pretrained(
        config.base_model, trust_remote_code=True
    )

    rest_client = service_client.create_rest_client()

    # --- Main loop ---
    for epoch in range(1, config.epochs + 1):
        t_epoch = time.monotonic()
        print(f"\n[loop] === Epoch {epoch}/{config.epochs} ===")

        # 1. Rollout
        print("[loop] Collecting rollouts...")
        run_id, run_dir = rollout.run_rollouts(
            tasks_path=config.tasks_path,
            agent=config.agent,
            model=config.model,
            max_steps=config.max_steps_per_episode,
            extra_env=config.rollout_env or None,
        )

        # 2. Load traces
        episodes = traces.load_run(run_dir)
        if not episodes:
            print(f"[loop] No valid episodes found in {run_dir}. Skipping epoch.")
            continue

        rewards = [ep.terminal_reward for ep in episodes]
        avg_reward = sum(rewards) / len(rewards)
        print(
            f"[loop] {len(episodes)} episodes loaded "
            f"| avg_reward={avg_reward:.4f} "
            f"| run={run_id}"
        )

        # 3. Build datums and execute PPO update
        print("[loop] Building PPO batch...")
        datums = ppo.build_batch(
            episodes=episodes,
            ref_client=ref_client,
            tokenizer=tokenizer,
            gamma=config.ppo.gamma,
        )

        print(f"[loop] Training on {len(datums)} datums...")
        stats = ppo.train_step(training_client, datums, config.ppo)

        elapsed = time.monotonic() - t_epoch
        print(
            f"[loop] loss={stats['loss']:.4f} "
            f"| n_datums={stats['n_datums']} "
            f"| t={elapsed:.1f}s"
        )

        # 4. Sync updated weights to vLLM for the next rollout round
        print("[loop] Syncing weights to vLLM...")
        checkpoints.sync_to_vllm(
            training_client, rest_client, config.vllm_base_url
        )

        # 5. Periodic full-state checkpoint
        if epoch % config.checkpoint_every == 0:
            checkpoints.save(training_client, name=f"epoch-{epoch}")

    # Final checkpoint so the trained model is always persisted.
    final_path = checkpoints.save(training_client, name="final")
    print(f"\n[loop] Training complete. Final checkpoint: {final_path}")


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Off-policy RL training loop for OpenCUA using Tinker.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    p.add_argument("--base-model", default="Qwen/Qwen3-8B")
    p.add_argument("--lora-rank", type=int, default=16)
    p.add_argument("--tasks-path", default="tasks/")
    p.add_argument("--agent", default="opencua")
    p.add_argument("--model", default="openai/opencua",
                   help="litellm model string forwarded to cb run (must contain 'OpenCUA')")
    p.add_argument("--max-steps", type=int, default=50)
    p.add_argument("--vllm-url", default="http://localhost:30000/v1")
    p.add_argument("--epochs", type=int, default=20)
    p.add_argument("--checkpoint-every", type=int, default=5)
    p.add_argument("--gamma", type=float, default=0.99)
    p.add_argument("--lr", type=float, default=1e-5)
    p.add_argument("--clip-low", type=float, default=0.8)
    p.add_argument("--clip-high", type=float, default=1.2)
    p.add_argument("--resume-from", default=None,
                   help="tinker:// checkpoint path to resume from")
    return p


if __name__ == "__main__":
    args = _build_parser().parse_args()

    config = TrainingConfig(
        base_model=args.base_model,
        lora_rank=args.lora_rank,
        tasks_path=args.tasks_path,
        agent=args.agent,
        model=args.model,
        max_steps_per_episode=args.max_steps,
        vllm_base_url=args.vllm_url,
        epochs=args.epochs,
        checkpoint_every=args.checkpoint_every,
        resume_from=args.resume_from,
        ppo=PPOConfig(
            gamma=args.gamma,
            learning_rate=args.lr,
            clip_low=args.clip_low,
            clip_high=args.clip_high,
        ),
    )

    run(config)
