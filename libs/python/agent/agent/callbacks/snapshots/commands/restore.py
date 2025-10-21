"""
Restore snapshot command.
"""

from typing import Dict, Any
import logging

from .base import SnapshotCommand

logger = logging.getLogger(__name__)


class RestoreSnapshotCommand(SnapshotCommand):
    """
    Command to restore a snapshot.
    Encapsulates restoration logic without if-else statements.
    """

    def __init__(self,
                 snapshot_context,
                 snapshot_creator,
                 container_name: str,
                 snapshot_id: str):
        """Initialize restore command."""
        self.snapshot_context = snapshot_context
        self.snapshot_creator = snapshot_creator
        self.container_name = container_name
        self.snapshot_id = snapshot_id

    async def execute(self) -> Dict[str, Any]:
        """Execute snapshot restoration."""
        try:
            self.snapshot_context.start_restoration()

            result = await self.snapshot_creator.restore_snapshot(
                self.container_name,
                self.snapshot_id
            )

            if result.get("status") == "restored":
                self.snapshot_context.complete_restoration()
                logger.info(f"Successfully restored snapshot {self.snapshot_id}")
            else:
                error_msg = result.get("error", "Unknown restoration error")
                self.snapshot_context.fail_restoration(error_msg)
                logger.error(f"Failed to restore snapshot: {error_msg}")

            return result

        except Exception as e:
            error_msg = str(e)
            self.snapshot_context.fail_restoration(error_msg)
            logger.error(f"Exception during snapshot restoration: {error_msg}")
            return {"status": "error", "error": error_msg}

    async def can_execute(self) -> bool:
        """Check if restoration can be executed."""
        return self.snapshot_context.can_restore()

    def get_command_name(self) -> str:
        """Get the name of this command."""
        return f"restore_snapshot[{self.snapshot_id}]"