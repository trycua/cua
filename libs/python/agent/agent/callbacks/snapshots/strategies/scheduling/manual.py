"""
Manual scheduling strategy - no automatic snapshots.
"""

from .base import SchedulingStrategy


class ManualSchedulingStrategy(SchedulingStrategy):
    """
    Manual scheduling strategy that never creates automatic snapshots.
    Eliminates if-else by always returning False.
    """

    def should_create_on_run_start(self) -> bool:
        """Manual strategy never creates snapshots on run start."""
        return False

    def should_create_on_run_end(self) -> bool:
        """Manual strategy never creates snapshots on run end."""
        return False

    def should_create_on_action(self) -> bool:
        """Manual strategy never creates snapshots on actions."""
        return False

    def get_strategy_name(self) -> str:
        """Get the name of this strategy."""
        return "manual"