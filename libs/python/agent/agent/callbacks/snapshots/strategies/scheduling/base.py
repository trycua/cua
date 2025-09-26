"""
Base scheduling strategy interface.
"""

from abc import ABC, abstractmethod
from typing import Dict, Any, Optional
import uuid
import logging

logger = logging.getLogger(__name__)


class SchedulingStrategy(ABC):
    """
    Abstract base class for scheduling strategies.
    Eliminates if-else statements by using polymorphism.
    """

    def __init__(self):
        """Initialize the scheduling strategy."""
        self.current_run_id: Optional[str] = None
        self.action_count: int = 0

    @abstractmethod
    def should_create_on_run_start(self) -> bool:
        """Determine if a snapshot should be created at run start."""
        pass

    @abstractmethod
    def should_create_on_run_end(self) -> bool:
        """Determine if a snapshot should be created at run end."""
        pass

    @abstractmethod
    def should_create_on_action(self) -> bool:
        """Determine if a snapshot should be created after an action."""
        pass

    @abstractmethod
    def get_strategy_name(self) -> str:
        """Get the name of this strategy."""
        pass

    def start_new_run(self) -> str:
        """Start tracking a new run."""
        self.current_run_id = str(uuid.uuid4())
        self.action_count = 0
        logger.debug(f"Started new run: {self.current_run_id}")
        return self.current_run_id

    def increment_action_count(self) -> int:
        """Increment the action counter."""
        self.action_count += 1
        return self.action_count

    def get_run_context(self) -> Dict[str, Any]:
        """Get the current run context."""
        return {
            "run_id": self.current_run_id,
            "action_count": self.action_count,
            "strategy": self.get_strategy_name()
        }

    def reset_run_context(self) -> None:
        """Reset the run context."""
        self.current_run_id = None
        self.action_count = 0
        logger.debug("Run context reset")

    def get_statistics(self) -> Dict[str, Any]:
        """Get strategy statistics."""
        return {
            "strategy_name": self.get_strategy_name(),
            "current_run_id": self.current_run_id,
            "actions_in_run": self.action_count
        }