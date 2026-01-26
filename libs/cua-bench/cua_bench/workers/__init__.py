"""Worker-based gym system for parallel environment management.

This module provides a FastAPI-based worker system for running CUA-Bench
environments in parallel, enabling efficient RL training and evaluation.

Components:
- worker_server: FastAPI server wrapping Environment instances
- worker_client: HTTP client for interacting with worker servers
- worker_manager: Utilities for spawning and managing multiple workers
- dataloader: MultiTurnDataloader and ReplayBuffer for RL training
"""

from .dataloader import MultiTurnDataloader, ReplayBuffer, create_dataloader_from_urls
from .worker_client import CBEnvWorkerClient
from .worker_manager import (
    WorkerHandle,
    WorkerPool,
    cleanup_workers,
    create_workers,
)

__all__ = [
    # Worker server (run as module: python -m cua_bench.workers.worker_server)
    "CBEnvWorkerClient",
    "WorkerHandle",
    "WorkerPool",
    "create_workers",
    "cleanup_workers",
    "MultiTurnDataloader",
    "ReplayBuffer",
    "create_dataloader_from_urls",
]
