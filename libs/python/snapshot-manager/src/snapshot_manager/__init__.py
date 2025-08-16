"""
Snapshot-based state management system for the Cua Agent SDK.

This package provides functionality to capture, store, and restore container
snapshots at defined intervals or events during agent execution.
"""

from .callback import SnapshotCallback
from .manager import SnapshotManager
from .models import SnapshotConfig, SnapshotMetadata, SnapshotTrigger
from .providers import DockerSnapshotProvider
from .storage import FileSystemSnapshotStorage

__version__ = "0.1.0"

__all__ = [
    "SnapshotMetadata",
    "SnapshotTrigger",
    "SnapshotConfig",
    "SnapshotManager",
    "SnapshotCallback",
    "DockerSnapshotProvider",
    "FileSystemSnapshotStorage",
]
