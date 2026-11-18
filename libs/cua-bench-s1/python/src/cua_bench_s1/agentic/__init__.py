"""Raw multi-step agentic interfaces (for RL training/eval), as opposed to the
closed-option-set `CuaTask` framing in `cua_bench_s1.task`."""

from .cua_bench_basic_env import (
    ENV_NAMES,
    MAX_STEPS,
    CuaBenchBasicEnv,
    EpisodeResult,
    StepResult,
    list_task_variants,
    oracle_reward_check,
    rollout,
)

__all__ = [
    "oracle_reward_check",
    "CuaBenchBasicEnv",
    "StepResult",
    "EpisodeResult",
    "rollout",
    "list_task_variants",
    "ENV_NAMES",
    "MAX_STEPS",
]
