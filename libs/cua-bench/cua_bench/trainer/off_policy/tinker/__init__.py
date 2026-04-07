from .grpo import GRPOConfig
from .checkpoints import CheckpointInfo, save_checkpoint, get_last_checkpoint

__all__ = [
    "TrainingConfig",
    "GRPOConfig",
    "run",
    "CheckpointInfo",
    "get_last_checkpoint",
    "save_checkpoint",
]


def __getattr__(name: str):
    if name in ("TrainingConfig", "run"):
        from .rl_loop import TrainingConfig, run
        return {"TrainingConfig": TrainingConfig, "run": run}[name]
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
