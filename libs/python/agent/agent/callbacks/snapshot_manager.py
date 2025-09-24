"""
Snapshot manager callback for managing container snapshots during agent runs.

This callback provides configurable snapshot intervals, retention policies,
and automatic cleanup to prevent unbounded storage growth.
"""

import json
from pathlib import Path
from datetime import datetime, timedelta
from typing import Optional, Dict, Any, List
import logging
import uuid

from .base import AsyncCallbackHandler

logger = logging.getLogger(__name__)


class SnapshotManagerCallback(AsyncCallbackHandler):
    """
    Manages container snapshots with configurable intervals and retention.
    Works as a pluggable component in the Agent SDK callback system.
    """

    def __init__(self,
                 computer: Optional[Any] = None,  # Computer instance
                 snapshot_interval: str = "manual",  # "manual", "every_action", "run_start", "run_end", "run_boundaries"
                 max_snapshots: int = 10,
                 retention_days: int = 7,
                 metadata_dir: str = "/tmp/cua_snapshots",
                 auto_cleanup: bool = True,
                 snapshot_prefix: str = "cua-snapshot"):
        """
        Initialize the snapshot manager callback.

        Args:
            computer: Computer instance with Docker provider
            snapshot_interval: When to create snapshots:
                - "manual": Only create snapshots when explicitly called
                - "every_action": Create snapshot after each computer action
                - "run_start": Create snapshot at the start of each run
                - "run_end": Create snapshot at the end of each run
                - "run_boundaries": Create snapshots at both start and end of runs
            max_snapshots: Maximum number of snapshots to retain
            retention_days: Delete snapshots older than this many days
            metadata_dir: Directory to store snapshot metadata
            auto_cleanup: Whether to automatically cleanup old snapshots
            snapshot_prefix: Prefix for snapshot names
        """
        self.computer = computer
        self.snapshot_interval = snapshot_interval
        self.max_snapshots = max_snapshots
        self.retention_days = retention_days
        self.metadata_dir = Path(metadata_dir)
        self.auto_cleanup = auto_cleanup
        self.snapshot_prefix = snapshot_prefix

        self.current_run_id = None
        self.action_count = 0
        self.metadata_dir.mkdir(parents=True, exist_ok=True)

        logger.info(f"SnapshotManagerCallback initialized with interval: {snapshot_interval}, "
                   f"max_snapshots: {max_snapshots}, retention_days: {retention_days}")

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Create snapshot at run start if configured."""
        self.current_run_id = str(uuid.uuid4())
        self.action_count = 0

        logger.debug(f"Run started with ID: {self.current_run_id}")

        if self.snapshot_interval in ["run_start", "run_boundaries"]:
            logger.info("Creating snapshot at run start")
            await self._create_snapshot_with_context("run_start")

    async def on_run_end(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]],
                         new_items: List[Dict[str, Any]]) -> None:
        """Create snapshot at run end if configured."""
        if self.snapshot_interval in ["run_end", "run_boundaries"]:
            logger.info("Creating snapshot at run end")
            await self._create_snapshot_with_context("run_end")

        if self.auto_cleanup:
            logger.debug("Performing automatic cleanup of old snapshots")
            await self._cleanup_old_snapshots()

    async def on_computer_call_end(self, item: Dict[str, Any], result: List[Dict[str, Any]]) -> None:
        """Create snapshot after each action if configured."""
        self.action_count += 1

        if self.snapshot_interval == "every_action":
            action_type = "unknown"
            if isinstance(item, dict) and "action" in item:
                action = item["action"]
                if isinstance(action, dict) and "type" in action:
                    action_type = action["type"]

            logger.info(f"Creating snapshot after action: {action_type}")
            await self._create_snapshot_with_context(f"after_action_{action_type}")

    async def _create_snapshot_with_context(self, trigger: str) -> Optional[Dict[str, Any]]:
        """Create snapshot with metadata context."""
        if not self.computer:
            logger.warning("No computer instance available for snapshot creation")
            return None

        # Check if the provider supports snapshots
        if not hasattr(self.computer, 'config') or not hasattr(self.computer.config, 'vm_provider'):
            logger.warning("Computer does not have a configured VM provider")
            return None

        provider = self.computer.config.vm_provider
        if not hasattr(provider, 'create_snapshot'):
            logger.warning(f"Provider {type(provider).__name__} does not support snapshots")
            return None

        metadata = {
            "run_id": self.current_run_id,
            "action_count": self.action_count,
            "trigger": trigger,
            "timestamp": datetime.now().isoformat()
        }

        try:
            # Generate snapshot name
            timestamp_str = datetime.now().strftime("%Y%m%d-%H%M%S")
            snapshot_name = f"{self.snapshot_prefix}-{timestamp_str}"

            logger.info(f"Creating snapshot: {snapshot_name}")

            # Use provider's snapshot method
            snapshot = await provider.create_snapshot(
                self.computer.config.name,
                snapshot_name=snapshot_name,
                metadata=metadata
            )

            if snapshot.get("status") == "error":
                logger.error(f"Failed to create snapshot: {snapshot.get('error')}")
                return None

            # Save metadata
            self._save_metadata(snapshot)

            # Enforce retention limits
            if self.auto_cleanup:
                await self._enforce_snapshot_limit()

            logger.info(f"Successfully created snapshot: {snapshot.get('tag')} (ID: {snapshot.get('id', 'unknown')[:8]})")
            return snapshot

        except NotImplementedError:
            logger.warning(f"Provider does not support snapshots")
            return None
        except Exception as e:
            logger.error(f"Error creating snapshot: {e}")
            return None

    def _save_metadata(self, snapshot: Dict[str, Any]) -> None:
        """Save snapshot metadata to JSON file."""
        if not self.computer or not hasattr(self.computer, 'config'):
            logger.warning("Cannot save metadata: computer not properly configured")
            return

        container_name = self.computer.config.name
        metadata_file = self.metadata_dir / f"{container_name}_snapshots.json"

        # Load existing metadata
        if metadata_file.exists():
            try:
                with open(metadata_file) as f:
                    data = json.load(f)
            except (json.JSONDecodeError, IOError) as e:
                logger.warning(f"Could not load existing metadata: {e}")
                data = {"snapshots": [], "retention_policy": {}}
        else:
            data = {"snapshots": [], "retention_policy": {}}

        # Add new snapshot
        data["snapshots"].append(snapshot)

        # Update retention policy
        data["retention_policy"] = {
            "max_snapshots": self.max_snapshots,
            "retention_days": self.retention_days,
            "last_cleanup": datetime.now().isoformat()
        }

        # Save updated metadata
        try:
            with open(metadata_file, 'w') as f:
                json.dump(data, f, indent=2)
            logger.debug(f"Saved snapshot metadata to {metadata_file}")
        except IOError as e:
            logger.error(f"Failed to save metadata: {e}")

    async def _enforce_snapshot_limit(self) -> None:
        """Delete oldest snapshots if over limit."""
        if not self.computer or not hasattr(self.computer.config, 'vm_provider'):
            return

        provider = self.computer.config.vm_provider
        if not hasattr(provider, 'list_snapshots') or not hasattr(provider, 'delete_snapshot'):
            return

        try:
            snapshots = await provider.list_snapshots(self.computer.config.name)

            if len(snapshots) > self.max_snapshots:
                logger.info(f"Enforcing snapshot limit: {len(snapshots)} > {self.max_snapshots}")

                # Sort by timestamp (oldest first)
                snapshots.sort(key=lambda x: x.get("timestamp", ""))

                # Delete oldest snapshots
                snapshots_to_delete = snapshots[:-self.max_snapshots]
                for snapshot in snapshots_to_delete:
                    logger.info(f"Deleting old snapshot: {snapshot.get('tag')} (ID: {snapshot.get('id', 'unknown')[:8]})")
                    await provider.delete_snapshot(snapshot["id"])

        except Exception as e:
            logger.error(f"Error enforcing snapshot limit: {e}")

    async def _cleanup_old_snapshots(self) -> None:
        """Delete snapshots older than retention_days."""
        if not self.computer or not hasattr(self.computer.config, 'vm_provider'):
            return

        provider = self.computer.config.vm_provider
        if not hasattr(provider, 'list_snapshots') or not hasattr(provider, 'delete_snapshot'):
            return

        try:
            snapshots = await provider.list_snapshots(self.computer.config.name)

            cutoff_date = datetime.now() - timedelta(days=self.retention_days)
            logger.debug(f"Cleaning up snapshots older than {cutoff_date.isoformat()}")

            for snapshot in snapshots:
                timestamp_str = snapshot.get("timestamp", "")
                if timestamp_str:
                    try:
                        timestamp = datetime.fromisoformat(timestamp_str)
                        if timestamp < cutoff_date:
                            logger.info(f"Deleting expired snapshot: {snapshot.get('tag')} "
                                      f"(created: {timestamp_str})")
                            await provider.delete_snapshot(snapshot["id"])
                    except (ValueError, TypeError) as e:
                        logger.warning(f"Could not parse timestamp for snapshot: {e}")

        except Exception as e:
            logger.error(f"Error cleaning up old snapshots: {e}")

    async def create_manual_snapshot(self, description: str = "") -> Dict[str, Any]:
        """
        Public method for manual snapshot creation.

        Args:
            description: Optional description for the snapshot

        Returns:
            Dictionary with snapshot information
        """
        logger.info(f"Creating manual snapshot: {description}")
        result = await self._create_snapshot_with_context(f"manual: {description}")
        if not result:
            return {"status": "error", "error": "Failed to create manual snapshot"}
        return result

    async def restore_snapshot(self, snapshot_id: str) -> Dict[str, Any]:
        """
        Restore to a specific snapshot.

        Args:
            snapshot_id: ID of the snapshot to restore

        Returns:
            Dictionary with restore status
        """
        if not self.computer or not hasattr(self.computer.config, 'vm_provider'):
            return {"status": "error", "error": "No computer or provider configured"}

        provider = self.computer.config.vm_provider
        if not hasattr(provider, 'restore_snapshot'):
            return {"status": "error", "error": "Provider does not support snapshot restoration"}

        logger.info(f"Restoring snapshot: {snapshot_id}")
        result = await provider.restore_snapshot(self.computer.config.name, snapshot_id)

        if not result:
            return {"status": "error", "error": "Restore operation returned no result"}

        if result.get("status") == "restored":
            # Reconnect the computer interface after restore
            if hasattr(self.computer, '_interface') and self.computer._interface:
                logger.info("Reconnecting computer interface after restore")
                try:
                    await self.computer._interface.wait_for_ready()
                except Exception as e:
                    logger.warning(f"Could not reconnect interface: {e}")

        return result

    async def list_snapshots(self) -> List[Dict[str, Any]]:
        """
        List all available snapshots.

        Returns:
            List of snapshot dictionaries
        """
        if not self.computer or not hasattr(self.computer.config, 'vm_provider'):
            logger.warning("No computer or provider configured")
            return []

        provider = self.computer.config.vm_provider
        if not hasattr(provider, 'list_snapshots'):
            logger.warning("Provider does not support listing snapshots")
            return []

        return await provider.list_snapshots(self.computer.config.name)

    async def delete_snapshot(self, snapshot_id: str) -> Dict[str, Any]:
        """
        Delete a specific snapshot.

        Args:
            snapshot_id: ID of the snapshot to delete

        Returns:
            Dictionary with deletion status
        """
        if not self.computer or not hasattr(self.computer.config, 'vm_provider'):
            return {"status": "error", "error": "No computer or provider configured"}

        provider = self.computer.config.vm_provider
        if not hasattr(provider, 'delete_snapshot'):
            return {"status": "error", "error": "Provider does not support snapshot deletion"}

        logger.info(f"Deleting snapshot: {snapshot_id}")
        return await provider.delete_snapshot(snapshot_id)