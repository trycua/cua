"""
Data models for snapshot management system.
"""

from datetime import datetime
from enum import Enum
from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field


class SnapshotTrigger(str, Enum):
    """Enumeration of snapshot trigger types."""

    MANUAL = "manual"
    RUN_START = "run_start"
    RUN_END = "run_end"
    AFTER_ACTION = "after_action"
    BEFORE_ACTION = "before_action"
    ON_ERROR = "on_error"
    PERIODIC = "periodic"


class SnapshotStatus(str, Enum):
    """Enumeration of snapshot status types."""

    CREATING = "creating"
    COMPLETED = "completed"
    FAILED = "failed"
    RESTORING = "restoring"
    DELETED = "deleted"


class SnapshotMetadata(BaseModel):
    """
    Metadata associated with a container snapshot.

    This contains all the information needed to identify, manage,
    and restore snapshots effectively.
    """

    # Core identification
    snapshot_id: str = Field(..., description="Unique identifier for the snapshot")
    container_id: str = Field(..., description="ID of the container that was snapshotted")
    container_name: str = Field(..., description="Name of the container")

    # Temporal information
    timestamp: datetime = Field(
        default_factory=datetime.now, description="When the snapshot was created"
    )

    # Context information
    trigger: SnapshotTrigger = Field(..., description="What triggered this snapshot")
    action_context: Optional[str] = Field(
        None, description="Context of the action that triggered the snapshot"
    )
    run_id: Optional[str] = Field(None, description="ID of the agent run this snapshot belongs to")

    # Status and lifecycle
    status: SnapshotStatus = Field(
        default=SnapshotStatus.CREATING, description="Current status of the snapshot"
    )

    # Storage information
    image_id: Optional[str] = Field(None, description="Docker image ID created from the snapshot")
    image_tag: Optional[str] = Field(None, description="Docker image tag")
    storage_path: Optional[str] = Field(None, description="Path where snapshot data is stored")
    size_bytes: Optional[int] = Field(None, description="Size of the snapshot in bytes")

    # Additional metadata
    description: Optional[str] = Field(None, description="Human-readable description")
    labels: Dict[str, str] = Field(default_factory=dict, description="Additional labels/tags")
    agent_metadata: Dict[str, Any] = Field(
        default_factory=dict, description="Agent-specific metadata"
    )

    # Restoration information
    parent_snapshot_id: Optional[str] = Field(
        None, description="ID of parent snapshot if this is a restored state"
    )
    restoration_count: int = Field(
        default=0, description="Number of times this snapshot has been restored"
    )

    class Config:
        """Pydantic configuration."""

        json_encoders = {datetime: lambda v: v.isoformat()}


class SnapshotConfig(BaseModel):
    """
    Configuration for snapshot management behavior.

    This controls when snapshots are taken, how they're stored,
    and cleanup policies.
    """

    # Trigger configuration
    triggers: List[SnapshotTrigger] = Field(
        default=[SnapshotTrigger.RUN_START, SnapshotTrigger.RUN_END],
        description="List of triggers that should create snapshots",
    )

    # Storage configuration
    storage_path: str = Field(default=".snapshots", description="Base path for snapshot storage")
    max_snapshots_per_container: int = Field(
        default=10, description="Maximum snapshots to keep per container"
    )
    max_total_snapshots: int = Field(
        default=100, description="Maximum total snapshots across all containers"
    )
    max_storage_size_gb: float = Field(default=10.0, description="Maximum storage size in GB")

    # Cleanup configuration
    cleanup_on_exit: bool = Field(
        default=True, description="Whether to clean up old snapshots on exit"
    )
    auto_cleanup_days: int = Field(
        default=7, description="Auto-delete snapshots older than this many days"
    )

    # Performance configuration
    compression_enabled: bool = Field(default=True, description="Whether to compress snapshots")
    parallel_operations: bool = Field(
        default=True, description="Whether to allow parallel snapshot operations"
    )

    # Integration configuration
    include_volumes: bool = Field(
        default=True, description="Whether to include mounted volumes in snapshots"
    )
    exclude_paths: List[str] = Field(
        default_factory=lambda: ["/tmp", "/var/tmp", "/proc", "/sys"],
        description="Paths to exclude from snapshots",
    )

    # Naming configuration
    naming_pattern: str = Field(
        default="{container_name}_{trigger}_{timestamp}", description="Pattern for snapshot naming"
    )


class RestoreOptions(BaseModel):
    """Options for restoring from a snapshot."""

    new_container_name: Optional[str] = Field(None, description="Name for the restored container")
    preserve_networks: bool = Field(
        default=True, description="Whether to preserve network connections"
    )
    preserve_volumes: bool = Field(default=True, description="Whether to preserve volume mounts")
    environment_overrides: Dict[str, str] = Field(
        default_factory=dict, description="Environment variables to override"
    )
    port_mappings: Dict[str, str] = Field(
        default_factory=dict, description="Port mappings to override (container_port: host_port)"
    )
