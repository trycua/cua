"""
Factory for creating cleanup policies without if-else statements.
"""

from typing import List
from ..strategies.cleanup import (
    CompositeCleanupPolicy,
    AgeBasedCleanupPolicy,
    CountBasedCleanupPolicy,
    CleanupPolicy
)


class CleanupPolicyFactory:
    """
    Factory for creating cleanup policies without conditional logic.
    """

    @staticmethod
    def create_standard_policy(
        max_snapshots: int = 10,
        retention_days: int = 7,
        auto_cleanup: bool = True
    ) -> CompositeCleanupPolicy:
        """
        Create standard composite cleanup policy.
        No if-else statements - always creates both policies.

        Args:
            max_snapshots: Maximum number of snapshots to retain
            retention_days: Delete snapshots older than this many days
            auto_cleanup: Whether to enable cleanup

        Returns:
            Composite cleanup policy with age and count-based policies
        """
        policies: List[CleanupPolicy] = [
            AgeBasedCleanupPolicy(retention_days, auto_cleanup),
            CountBasedCleanupPolicy(max_snapshots, auto_cleanup)
        ]

        return CompositeCleanupPolicy(policies)
    @staticmethod
    def create_age_only_policy(retention_days: int = 7, enabled: bool = True) -> AgeBasedCleanupPolicy:
        """Create age-based only cleanup policy."""
        return AgeBasedCleanupPolicy(retention_days, enabled)

    @staticmethod
    def create_count_only_policy(max_snapshots: int = 10, enabled: bool = True) -> CountBasedCleanupPolicy:
        """Create count-based only cleanup policy."""
        return CountBasedCleanupPolicy(max_snapshots, enabled)

    @staticmethod
    def create_disabled_policy() -> CompositeCleanupPolicy:
        """Create disabled cleanup policy."""
        policies: List[CleanupPolicy] = [
            AgeBasedCleanupPolicy(0, enabled=False),
            CountBasedCleanupPolicy(0, enabled=False)
        ]
        return CompositeCleanupPolicy(policies)

    @staticmethod
    def create_custom_policy(policies: List[CleanupPolicy]) -> CompositeCleanupPolicy:
        """Create custom composite policy from provided policies."""
        return CompositeCleanupPolicy(policies)
