"""
Create snapshot command.
"""

from typing import Dict, Any, Optional
import logging

from .base import SnapshotCommand

logger = logging.getLogger(__name__)


class CreateSnapshotCommand(SnapshotCommand):
    """
    Command to create a snapshot.
    Encapsulates creation logic without if-else statements.
    """

    def __init__(self,
                 snapshot_context,
                 snapshot_creator,
                 container_name: str,
                 trigger: str,
                 metadata: Optional[Dict[str, Any]] = None):
        """Initialize create command."""
        self.snapshot_context = snapshot_context
        self.snapshot_creator = snapshot_creator
        self.container_name = container_name
        self.trigger = trigger
        self.metadata = metadata or {}

    async def execute(self) -> Dict[str, Any]:
        """Execute snapshot creation."""
        try:
            self.snapshot_context.start_creation()

            result = await self.snapshot_creator.create_snapshot(
                self.container_name,
                self.trigger,
                self.metadata
            )

            if result and result.get("status") != "error":
                self.snapshot_context.complete_creation(result)
                logger.info(f"Successfully created snapshot: {result.get('tag')}")
                return result
            else:
                error_msg = result.get("error", "Unknown error") if result else "No response"
                self.snapshot_context.fail_creation(error_msg)
                return {"status": "error", "error": error_msg}

        except Exception as e:
            error_msg = str(e)
            self.snapshot_context.fail_creation(error_msg)
            logger.error(f"Exception during snapshot creation: {error_msg}")
            return {"status": "error", "error": error_msg}

    async def can_execute(self) -> bool:
        """Check if creation can be executed."""
        return self.snapshot_context.can_create()

    def get_command_name(self) -> str:
        """Get the name of this command."""
        return f"create_snapshot[{self.trigger}]"