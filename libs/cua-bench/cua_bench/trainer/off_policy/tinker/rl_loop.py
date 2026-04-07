from __future__ import annotations

import argparse
import os
import time
from dataclasses import dataclass, field
from typing import Optional

from .grpo import GRPOConfig
from tinker_cookbook import checkpoint_utils, model_info, renderers
from tinker_cookbook.image_processing_utils import get_image_processor

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


@dataclass
class TrainingConfig:
    # --- Mode ---
    # "online" = collect rollouts via `cb run` each epoch.
    # "offline" = use pre-collected run IDs (skip rollout collection).
    mode: str = "online"

    # --- Model ---
    base_model: str = "Qwen/Qwen3-8B"
    lora_rank: int = 16
    lora_seed: int = 42

    # --- Agent / rollout (online mode) ---
    tasks_path: str = "tasks/"
    agent: str = "opencua"
    model: str = "openai/opencua"
    max_steps_per_episode: int = 50
    vllm_base_url: str = "http://localhost:30000/v1"

    # --- Offline mode ---
    # List of pre-collected run IDs to train on. Reused every epoch.
    offline_run_ids: list[str] = field(default_factory=list)

    # --- Training loop ---
    epochs: int = 20
    save_every: int = 5
    log_path: str = "/tmp/tinker-cua-rl"
    ttl_seconds: Optional[int] = 604800  # 7 days

    # --- GRPO rollout ---
    # Number of rollouts per task per epoch (must be >= 2 for GRPO).
    num_rollouts: int = 2

    # --- GRPO hyperparameters ---
    grpo: GRPOConfig = field(default_factory=GRPOConfig)

    # --- Resume ---
    resume_from: Optional[str] = None

    # --- Tinker ---
    tinker_api_key: Optional[str] = None

    # --- Rollout subprocess environment ---
    rollout_env: dict[str, str] = field(default_factory=dict)


# ---------------------------------------------------------------------------
# Training loop
# ---------------------------------------------------------------------------


def run(config: TrainingConfig) -> None:
    """Execute the full RL training loop.

    All Tinker imports are deferred so the module can be imported in
    environments where ``tinker`` is not installed.
    """
    from tinker import ServiceClient
    import tinker.types as tt
    from transformers import AutoTokenizer, AutoProcessor

    from . import traces, rollout, grpo, checkpoints
    from .rollout import _get_run_output_dir

    if config.mode not in ("online", "offline"):
        raise ValueError(f"Unknown mode {config.mode!r}. Must be 'online' or 'offline'.")

    if config.mode == "offline" and not config.offline_run_ids:
        raise ValueError("offline mode requires at least one run ID in offline_run_ids.")

    api_key = config.tinker_api_key or os.environ.get("TINKER_API_KEY")
    if not api_key:
        raise ValueError(
            "Tinker API key not found. "
            "Set TINKER_API_KEY in the environment or pass it via TrainingConfig."
        )

    service_client = ServiceClient(api_key=api_key)

    # --- Resume or fresh start ---
    resume_info = checkpoints.get_last_checkpoint(config.log_path)
    
    if resume_info:
        print(f"[loop] Resuming from checkpoint: {resume_info.state_path} (epoch {resume_info.epoch})")
        training_client = service_client.create_training_client_from_state_with_optimizer(
            resume_info.state_path
        )
        start_epoch = resume_info.epoch
    else:
        print(f"[loop] Starting fresh LoRA training on {config.base_model}")
        training_client = service_client.create_lora_training_client(
            base_model=config.base_model,
            rank=config.lora_rank,
            seed=config.lora_seed,
        )
        start_epoch = 0

    # --- Tokenizer ---
    tokenizer = AutoTokenizer.from_pretrained(
        config.base_model, trust_remote_code=True
    )
    
    image_processor = get_image_processor(config.base_model)
    
    renderer_name = model_info.get_recommended_renderer_name(config.base_model)
    renderer = renderers.get_renderer(renderer_name, tokenizer=tokenizer, image_processor=image_processor)

    # --- Sampling params for reference logprobs ---
    sampling_params = tt.SamplingParams(max_tokens=1)

    print(f"[loop] Training for {config.epochs} epochs (starting from {start_epoch})")

    # --- Main loop ---
    for epoch in range(start_epoch + 1, config.epochs + 1):
        t_epoch = time.monotonic()
        print(f"\n[loop] === Epoch {epoch}/{config.epochs} ===")

        metrics: dict[str, float] = {
            "progress/epoch": epoch,
            "progress/done_frac": epoch / config.epochs,
            "optim/lr": config.grpo.learning_rate,
        }

        # 1. Periodic checkpoint (before rollout so we can resume on crash)
        if config.save_every > 0 and epoch > 1 and (epoch - 1) % config.save_every == 0:
            checkpoints.save_checkpoint(
                training_client=training_client,
                name=f"epoch-{epoch - 1}",
                log_path=config.log_path,
                kind="state",
                loop_state={"epoch": epoch - 1},
                ttl_seconds=config.ttl_seconds
            )

        # 2. Snapshot current weights for reference logprobs
        sampling_client = training_client.save_weights_and_get_sampling_client()

        # 3. Collect or resolve rollout data
        if config.mode == "online":
            print(f"[loop] Collecting {config.num_rollouts} rollout(s)...")
            rollout_results = rollout.run_rollouts(
                tasks_path=config.tasks_path,
                agent=config.agent,
                model=config.model,
                max_steps=config.max_steps_per_episode,
                num_rollouts=config.num_rollouts,
                extra_env=config.rollout_env or None,
            )
            run_ids = [r[0] for r in rollout_results]
            run_dirs = [r[1] for r in rollout_results]
        else:
            # Offline: reuse pre-collected run IDs every epoch
            run_ids = list(config.offline_run_ids)
            run_dirs = [_get_run_output_dir(rid) for rid in run_ids]
            missing = [d for d in run_dirs if not d.exists()]
            if missing:
                raise FileNotFoundError(
                    f"Offline run directories not found: {missing}. "
                    "Check that the run IDs are correct and traces exist on disk."
                )
            print(f"[loop] Offline mode: using {len(run_ids)} pre-collected run(s)")

        # 4. Load traces from all rollout runs
        episodes = traces.load_runs(run_dirs)
        
        breakpoint()
        
        if not episodes:
            print(f"[loop] No valid episodes across {len(run_dirs)} runs. Skipping epoch.")
            continue

        # Validate GRPO group sizes — warn if any task has only 1 trajectory
        task_groups: dict[str, int] = {}
        for ep in episodes:
            task_groups[ep.task_description] = task_groups.get(ep.task_description, 0) + 1
        singleton_tasks = [t for t, c in task_groups.items() if c < 2]
        if singleton_tasks:
            print(
                f"[loop] Warning: {len(singleton_tasks)} task(s) have < 2 trajectories "
                f"(GRPO needs group_size > 1). These will be skipped."
            )

        rewards_P = [ep.terminal_reward for ep in episodes]
        avg_reward = sum(rewards_P) / len(rewards_P)
        print(
            f"[loop] {len(episodes)} episodes across {len(run_ids)} runs | "
            f"avg_reward={avg_reward:.4f}"
        )
        metrics["reward/total"] = avg_reward

        # 5. Build datums
        print("[loop] Building training batch...")
        
        datums_D = grpo.build_batch(
            renderer=renderer,
            episodes=episodes,
            sampling_client=sampling_client,
            sampling_params=sampling_params,
            tokenizer=tokenizer,
            gamma=config.grpo.gamma,
            max_images=config.grpo.max_images
        )

        # 6. Training step
        if not datums_D:
            print("[loop] All advantages zero — skipping training step.")
        else:
            print(f"[loop] Training on {len(datums_D)} datums...")
            step_stats = grpo.train_step(training_client, datums_D, config.grpo)
            metrics.update(step_stats)

        elapsed = time.monotonic() - t_epoch
        metrics["time/total"] = elapsed
        print(
            f"[loop] loss={metrics.get('loss', 0.0):.4f} | "
            f"n_datums={metrics.get('n_datums', 0)} | "
            f"t={elapsed:.1f}s"
        )

    # Final checkpoint
    checkpoints.save_checkpoint(
        training_client=training_client,
        name="final",
        log_path=config.log_path,
        kind="state",
        loop_state={"epoch": config.epochs},
        ttl_seconds=None,
    )
    print(f"\n[loop] Training complete.")


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Off-policy RL training loop using Tinker (GRPO-style).",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    p.add_argument("--mode", choices=["online", "offline"], default="online",
                   help="'online' collects rollouts via cb run; 'offline' uses pre-collected run IDs")
    p.add_argument("--base-model", default="Qwen/Qwen3-8B")
    p.add_argument("--lora-rank", type=int, default=16)
    p.add_argument("--tasks-path", default="tasks/")
    p.add_argument("--agent", default="opencua")
    p.add_argument("--model", default="openai/opencua",
                   help="litellm model string forwarded to cb run")
    p.add_argument("--max-steps", type=int, default=50)
    p.add_argument("--vllm-url", default="http://localhost:30000/v1")
    p.add_argument("--epochs", type=int, default=20)
    p.add_argument("--save-every", type=int, default=5)
    p.add_argument("--log-path", default="/tmp/tinker-cua-rl")
    p.add_argument("--num-rollouts", type=int, default=2,
                   help="Number of rollouts per task per epoch (>= 2 for GRPO)")
    p.add_argument("--offline-run-ids", nargs="*", default=[],
                   help="Run IDs for offline mode (reused every epoch)")
    p.add_argument("--gamma", type=float, default=0.99)
    p.add_argument("--lr", type=float, default=1e-5)
    p.add_argument("--beta1", type=float, default=0.9,
                   help="AdamW beta1")
    p.add_argument("--beta2", type=float, default=0.95,
                   help="AdamW beta2")
    p.add_argument("--max-images", type=int, default=3,
                   help="Maximum number of recent screenshots to keep per trajectory")
    p.add_argument("--resume-from", default=None,
                   help="tinker:// checkpoint path to resume from")
    return p


if __name__ == "__main__":
    args = _build_parser().parse_args()

    config = TrainingConfig(
        mode=args.mode,
        base_model=args.base_model,
        lora_rank=args.lora_rank,
        tasks_path=args.tasks_path,
        agent=args.agent,
        model=args.model,
        max_steps_per_episode=args.max_steps,
        vllm_base_url=args.vllm_url,
        epochs=args.epochs,
        save_every=args.save_every,
        log_path=args.log_path,
        num_rollouts=args.num_rollouts,
        offline_run_ids=args.offline_run_ids,
        resume_from=args.resume_from,
        grpo=GRPOConfig(
            gamma=args.gamma,
            learning_rate=args.lr,
            beta1=args.beta1,
            beta2=args.beta2,
            max_images=args.max_images,
        ),
    )

    run(config)
