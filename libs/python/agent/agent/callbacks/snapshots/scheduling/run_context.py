"""
Run context management for snapshot scheduling.
"""

from typing import Optional, Dict, Any
import uuid
import logging

logger = logging.getLogger(__name__)


class RunContextManager:
    """
    Manages run context including run IDs and action counting.
    """

    def __init__(self):
        """Initialize the run context manager."""
        self.current_run_id: Optional[str] = None
        self.action_count: int = 0

    def start_new_run(self) -> str:
        """
        Start tracking a new run.

        Returns:
            The new run ID
        """
        self.current_run_id = str(uuid.uuid4())
        self.action_count = 0
        logger.debug(f"Started new run: {self.current_run_id}")
        return self.current_run_id

    def increment_action_count(self) -> int:
        """
        Increment the action counter.

        Returns:
            The new action count
        """
        self.action_count += 1
        return self.action_count

    def get_context(self) -> Dict[str, Any]:
        """
        Get the current run context.

        Returns:
            Dictionary with run context information
        """
        return {
            "run_id": self.current_run_id,
            "action_count": self.action_count
        }

    def reset(self) -> None:
        """Reset the run context."""
        self.current_run_id = None
        self.action_count = 0
        logger.debug("Run context reset")

    def get_statistics(self) -> Dict[str, Any]:
        """
        Get run context statistics.

        Returns:
            Dictionary with run statistics
        """
        return {
            "current_run_id": self.current_run_id,
            "actions_in_run": self.action_count
        }