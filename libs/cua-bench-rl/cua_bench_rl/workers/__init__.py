"""Worker-based gym system for parallel environment management.

This module provides a FastAPI-based worker system for running CUA-Bench
environments in parallel, enabling efficient RL training and evaluation.

Components:
- worker_server: FastAPI server wrapping Environment instances
- worker_client: HTTP client for interacting with worker servers
- worker_manager: Utilities for spawning and managing multiple workers
- dataloader: MultiTurnDataloader and ReplayBuffer for RL training
"""

from typing import TYPE_CHECKING, Any

from .worker_client import CBEnvWorkerClient
from .worker_manager import (
    WorkerHandle,
    WorkerPool,
    cleanup_workers,
    create_workers,
)

__all__ = [
    # Worker server (run as module: python -m cua_bench_rl.workers.worker_server)
    "CBEnvWorkerClient",
    "WorkerHandle",
    "WorkerPool",
    "create_workers",
    "cleanup_workers",
    "MultiTurnDataloader",
    "ReplayBuffer",
]

# The dataloader needs torch (an RL-training dependency, not in the default
# install): import it on first use so the client, server and manager work,
# and test collection succeeds, without torch.
_LAZY = {"MultiTurnDataloader", "ReplayBuffer"}

if TYPE_CHECKING:
    from .dataloader import MultiTurnDataloader, ReplayBuffer


def __getattr__(name: str) -> Any:
    if name in _LAZY:
        from . import dataloader

        return getattr(dataloader, name)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
