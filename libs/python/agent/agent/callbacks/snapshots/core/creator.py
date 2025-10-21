"""
Snapshot creation operations.
"""

from datetime import datetime
from typing import Optional, Dict, Any
import logging
import uuid
from .restore import RestoreOperations

logger = logging.getLogger(__name__)

class SnapshotCreator:
    """
    Handles core snapshot creation and restoration operations.
    """

    def __init__(self, provider_adapter: Any, snapshot_prefix: str = "cua-snapshot"):
        """
        Initialize the snapshot creator.

        Args:
            provider_adapter: Adapter for interacting with the VM provider
            snapshot_prefix: Prefix for generated snapshot names
        """
        self.provider_adapter = provider_adapter
        self.snapshot_prefix = snapshot_prefix
        self.restore_ops = None

    async def create_snapshot(self,
                            container_name: str,
                            trigger: str,
                            metadata: Optional[Dict[str, Any]] = None) -> Optional[Dict[str, Any]]:
        """
        Create a snapshot with the given parameters.

        Args:
            container_name: Name of the container to snapshot
            trigger: Description of what triggered this snapshot
            metadata: Additional metadata to include

        Returns:
            Dictionary with snapshot information or None if failed
        """
        if not await self.provider_adapter.validate_provider():
            logger.warning("Provider does not support snapshots")
            return None

        snapshot_name = self._generate_snapshot_name()

        full_metadata = {
            "trigger": trigger,
            "timestamp": datetime.now().isoformat(),
            "snapshot_id": str(uuid.uuid4()),
            **(metadata or {})
        }

        try:
            logger.info(f"Creating snapshot: {snapshot_name} for container: {container_name}")

            snapshot = await self.provider_adapter.create_snapshot(
                container_name,
                snapshot_name,
                full_metadata
            )

            if snapshot and snapshot.get("status") != "error":
                logger.info(f"Successfully created snapshot: {snapshot_name}")
                return snapshot
            else:
                error_msg = snapshot.get("error", "Unknown error") if snapshot else "No response"
                logger.error(f"Failed to create snapshot: {error_msg}")
                return None

        except Exception as e:
            logger.error(f"Error creating snapshot: {e}")
            return None

    async def restore_snapshot(self, container_name: str, snapshot_id: str) -> Dict[str, Any]:
        """Restore a container to a specific snapshot."""
        if self.restore_ops is None:
            self.restore_ops = RestoreOperations(self.provider_adapter)
        return await self.restore_ops.restore(container_name, snapshot_id)

    def _generate_snapshot_name(self) -> str:
        """Generate a unique snapshot name with timestamp."""
        timestamp_str = datetime.now().strftime("%Y%m%d-%H%M%S")
        return f"{self.snapshot_prefix}-{timestamp_str}"