"""
Every action scheduling strategy - creates snapshot after each action.
"""

from .base import SchedulingStrategy


class EveryActionSchedulingStrategy(SchedulingStrategy):
    """
    Strategy that creates a snapshot after every computer action.
    Eliminates if-else by using polymorphism.
    """

    def should_create_on_run_start(self) -> bool:
        """Every action strategy doesn't create snapshots on run start."""
        return False

    def should_create_on_run_end(self) -> bool:
        """Every action strategy doesn't create snapshots on run end."""
        return False

    def should_create_on_action(self) -> bool:
        """Every action strategy always creates snapshots on actions."""
        return True

    def get_strategy_name(self) -> str:
        """Get the name of this strategy."""
        return "every_action"