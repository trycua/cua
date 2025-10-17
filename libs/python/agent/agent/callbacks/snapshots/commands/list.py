"""
List snapshots command.
"""

from typing import Dict, Any, List
import logging

from .base import SnapshotCommand

logger = logging.getLogger(__name__)


class ListSnapshotsCommand(SnapshotCommand):
    """
    Command to list snapshots.
    Encapsulates listing logic without if-else statements.
    """

    def __init__(self,
                 provider_adapter,
                 container_name: str):
        """Initialize list command."""
        self.provider_adapter = provider_adapter
        self.container_name = container_name

    async def execute(self) -> Dict[str, Any]:
        """Execute snapshot listing."""
        try:
            snapshots = await self.provider_adapter.list_snapshots(self.container_name)

            return {
                "status": "success",
                "snapshots": snapshots,
                "count": len(snapshots)
            }

        except Exception as e:
            error_msg = str(e)
            logger.error(f"Exception during snapshot listing: {error_msg}")
            return {
                "status": "error",
                "error": error_msg,
                "snapshots": [],
                "count": 0
            }

    async def can_execute(self) -> bool:
        """List command can always be executed."""
        return True

    def get_command_name(self) -> str:
        """Get the name of this command."""
        return f"list_snapshots[{self.container_name}]"