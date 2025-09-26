"""
Core retention policy enforcement.
"""

from datetime import datetime
from typing import List, Dict, Any, Optional, Tuple
import logging

from .age_cleanup import AgeBasedCleanup
from .limit_cleanup import LimitBasedCleanup

logger = logging.getLogger(__name__)


class RetentionPolicyEnforcer:
    """
    Enforces snapshot retention policies.
    """

    def __init__(self,
                 max_snapshots: int = 10,
                 retention_days: int = 7,
                 auto_cleanup: bool = True):
        """
        Initialize the retention policy enforcer.

        Args:
            max_snapshots: Maximum number of snapshots to retain
            retention_days: Delete snapshots older than this many days
            auto_cleanup: Whether to automatically cleanup old snapshots
        """
        self.max_snapshots = max_snapshots
        self.retention_days = retention_days
        self.auto_cleanup = auto_cleanup

        self.age_cleanup = AgeBasedCleanup(retention_days)
        self.limit_cleanup = LimitBasedCleanup(max_snapshots)

    async def enforce_snapshot_limit(self,
                                    provider_adapter: Any,
                                    container_name: str) -> List[Dict[str, Any]]:
        """Delete oldest snapshots if over the maximum limit."""
        if not self.auto_cleanup:
            return []

        return await self.limit_cleanup.enforce_limit(
            provider_adapter, container_name, self.auto_cleanup
        )

    async def cleanup_old_snapshots(self,
                                   provider_adapter: Any,
                                   container_name: str) -> List[Dict[str, Any]]:
        """Delete snapshots older than retention_days."""
        if not self.auto_cleanup:
            return []

        return await self.age_cleanup.cleanup_expired(
            provider_adapter, container_name, self.auto_cleanup
        )

    def should_cleanup(self, last_cleanup_time: Optional[datetime] = None) -> bool:
        """Determine if cleanup should be performed."""
        if not self.auto_cleanup:
            return False

        if last_cleanup_time is None:
            return True

        time_since_cleanup = datetime.now() - last_cleanup_time
        return time_since_cleanup.days >= 1

    def get_snapshots_to_delete(self,
                               snapshots: List[Dict[str, Any]]) -> Tuple[List[Dict[str, Any]], str]:
        """Calculate which snapshots should be deleted based on policies."""
        snapshots_to_delete = []
        reasons = []

        expired, expired_count = self.age_cleanup.find_expired_snapshots(snapshots)
        if expired:
            snapshots_to_delete.extend(expired)
            reasons.append(f"{expired_count} expired (>{self.retention_days} days)")

        over_limit, over_count = self.limit_cleanup.find_over_limit_snapshots(snapshots)
        for snapshot in over_limit:
            if snapshot not in snapshots_to_delete:
                snapshots_to_delete.append(snapshot)
        if over_limit:
            reasons.append(f"{over_count} over limit (>{self.max_snapshots})")

        reason_str = " and ".join(reasons) if reasons else "none"
        return snapshots_to_delete, reason_str

    def update_policy(self,
                     max_snapshots: Optional[int] = None,
                     retention_days: Optional[int] = None,
                     auto_cleanup: Optional[bool] = None) -> None:
        """Update retention policy settings."""
        if max_snapshots is not None:
            self.max_snapshots = max_snapshots
            self.limit_cleanup.max_snapshots = max_snapshots
            logger.info(f"Updated max_snapshots to {max_snapshots}")

        if retention_days is not None:
            self.retention_days = retention_days
            self.age_cleanup.retention_days = retention_days
            logger.info(f"Updated retention_days to {retention_days}")

        if auto_cleanup is not None:
            self.auto_cleanup = auto_cleanup
            logger.info(f"Updated auto_cleanup to {auto_cleanup}")
