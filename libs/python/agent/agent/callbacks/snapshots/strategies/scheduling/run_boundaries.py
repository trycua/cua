"""
Run boundaries scheduling strategies.
"""

from .base import SchedulingStrategy


class RunStartSchedulingStrategy(SchedulingStrategy):
    """
    Strategy that creates snapshots only at run start.
    """

    def should_create_on_run_start(self) -> bool:
        """Create snapshots on run start."""
        return True

    def should_create_on_run_end(self) -> bool:
        """Don't create snapshots on run end."""
        return False

    def should_create_on_action(self) -> bool:
        """Don't create snapshots on actions."""
        return False

    def get_strategy_name(self) -> str:
        """Get the name of this strategy."""
        return "run_start"


class RunEndSchedulingStrategy(SchedulingStrategy):
    """
    Strategy that creates snapshots only at run end.
    """

    def should_create_on_run_start(self) -> bool:
        """Don't create snapshots on run start."""
        return False

    def should_create_on_run_end(self) -> bool:
        """Create snapshots on run end."""
        return True

    def should_create_on_action(self) -> bool:
        """Don't create snapshots on actions."""
        return False

    def get_strategy_name(self) -> str:
        """Get the name of this strategy."""
        return "run_end"


class RunBoundariesSchedulingStrategy(SchedulingStrategy):
    """
    Strategy that creates snapshots at both run start and end.
    """

    def should_create_on_run_start(self) -> bool:
        """Create snapshots on run start."""
        return True

    def should_create_on_run_end(self) -> bool:
        """Create snapshots on run end."""
        return True

    def should_create_on_action(self) -> bool:
        """Don't create snapshots on actions."""
        return False

    def get_strategy_name(self) -> str:
        """Get the name of this strategy."""
        return "run_boundaries"