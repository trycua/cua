"""Off-policy RL training for OpenCUA via the Tinker fine-tuning API.

Modules
-------
traces      Load cua-bench HF Dataset traces → Episode / Step objects.
rollout     Spawn ``cb run dataset`` and block until all sessions finish.
ppo         Build PPO Datums from episodes and execute a Tinker training step.
checkpoints Save Tinker state checkpoints and sync LoRA weights to vLLM.
loop        ``TrainingConfig`` dataclass and the main ``run()`` training loop.

Quickstart
----------
::

    from cua_bench.rl.off_policy.tinker.loop import TrainingConfig, run

    run(TrainingConfig(
        base_model="Qwen/Qwen3-8B",
        tasks_path="tasks/slack_env",
        model="openai/opencua",
        vllm_base_url="http://localhost:30000/v1",
        epochs=20,
    ))

Or from the command line::

    python -m cua_bench.rl.off_policy.tinker.loop \\
        --base-model Qwen/Qwen3-8B \\
        --tasks-path tasks/slack_env \\
        --model openai/opencua \\
        --epochs 20 \\
        --lr 1e-5
"""

from .loop import TrainingConfig, run

__all__ = ["TrainingConfig", "run"]
