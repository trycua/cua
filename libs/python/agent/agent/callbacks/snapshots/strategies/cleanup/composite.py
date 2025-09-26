"""
Composite cleanup policy that combines multiple policies.
"""

from typing import List, Dict, Any
import logging

from .base import CleanupPolicy

logger = logging.getLogger(__name__)


class CompositeCleanupPolicy(CleanupPolicy):
    """
    Composite policy that applies multiple cleanup policies.
    Uses composition instead of if-else statements.
    """

    def __init__(self, policies: List[CleanupPolicy]):
        """Initialize with a list of cleanup policies."""
        self.policies = policies

    async def should_cleanup(self, snapshots: List[Dict[str, Any]]) -> bool:
        """Check if any policy requires cleanup."""
        for policy in self.policies:
            if await policy.should_cleanup(snapshots):
                return True
        return False

    async def get_snapshots_to_cleanup(self, snapshots: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
        """Get all snapshots that any policy wants to cleanup."""
        all_snapshots_to_cleanup = set()

        for policy in self.policies:
            policy_snapshots = await policy.get_snapshots_to_cleanup(snapshots)
            for snapshot in policy_snapshots:
                all_snapshots_to_cleanup.add(snapshot.get("id"))

        return [s for s in snapshots if s.get("id") in all_snapshots_to_cleanup]

    async def cleanup(self, provider_adapter: Any, container_name: str) -> List[Dict[str, Any]]:
        """Apply all cleanup policies."""
        all_deleted = []

        for policy in self.policies:
            deleted = await policy.cleanup(provider_adapter, container_name)
            all_deleted.extend(deleted)
            logger.debug(f"Policy {policy.get_policy_name()} deleted {len(deleted)} snapshots")

        return all_deleted

    def get_policy_name(self) -> str:
        """Get the name of this cleanup policy."""
        policy_names = [p.get_policy_name() for p in self.policies]
        return f"composite[{','.join(policy_names)}]"

    def add_policy(self, policy: CleanupPolicy) -> None:
        """Add a new cleanup policy to the composite."""
        self.policies.append(policy)

    def remove_policy(self, policy_name: str) -> bool:
        """Remove a cleanup policy by name."""
        for i, policy in enumerate(self.policies):
            if policy.get_policy_name() == policy_name:
                del self.policies[i]
                return True
        return False