"""
Snapshot providers for different container technologies.
"""

from .docker_provider import DockerSnapshotProvider

__all__ = ["DockerSnapshotProvider"]
