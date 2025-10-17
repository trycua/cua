"""
Delete snapshot command.
"""

from typing import Dict, Any
import logging

from .base import SnapshotCommand

logger = logging.getLogger(__name__)


class DeleteSnapshotCommand(SnapshotCommand):
    """
    Command to delete a snapshot.
    Encapsulates deletion logic without if-else statements.
    """

    def __init__(self,
                 snapshot_context,
                 provider_adapter,
                 metadata_manager,
                 container_name: str,
                 snapshot_id: str):
        """Initialize delete command."""
        self.snapshot_context = snapshot_context
        self.provider_adapter = provider_adapter
        self.metadata_manager = metadata_manager
        self.container_name = container_name
        self.snapshot_id = snapshot_id

    async def execute(self) -> Dict[str, Any]:
        """Execute snapshot deletion."""
        try:
            logger.info(f"Deleting snapshot: {self.snapshot_id}")

            result = await self.provider_adapter.delete_snapshot(self.snapshot_id)

            if result.get("status") == "deleted":
                self.metadata_manager.remove_snapshot_metadata(
                    self.container_name,
                    self.snapshot_id
                )
                logger.info(f"Successfully deleted snapshot {self.snapshot_id}")
            else:
                logger.error(f"Failed to delete snapshot: {result.get('error')}")

            return result

        except Exception as e:
            error_msg = str(e)
            logger.error(f"Exception during snapshot deletion: {error_msg}")
            return {"status": "error", "error": error_msg}

    async def can_execute(self) -> bool:
        """Check if deletion can be executed."""
        return self.snapshot_context.can_delete()

    def get_command_name(self) -> str:
        """Get the name of this command."""
        return f"delete_snapshot[{self.snapshot_id}]"