"""
CUA Agent SDK callback integration for automatic snapshot management.

This callback handler integrates with the CUA Agent SDK's callback system
to automatically create snapshots at appropriate lifecycle events.
"""

import logging
from typing import Dict, List, Any, Optional

try:
    from agent.callbacks.base import AsyncCallbackHandler
    HAVE_CUA_SDK = True
except ImportError:
    # Fallback for when CUA SDK is not available
    class AsyncCallbackHandler:
        pass
    HAVE_CUA_SDK = False

from .manager import SnapshotManager
from .models import SnapshotTrigger, SnapshotConfig

logger = logging.getLogger(__name__)


class SnapshotCallback(AsyncCallbackHandler):
    """
    CUA Agent SDK callback handler for snapshot management.
    
    This callback integrates with the agent's lifecycle to automatically
    create snapshots when configured events occur. It's designed to be
    pluggable and non-intrusive to existing agent workflows.
    """

    def __init__(
        self,
        snapshot_manager: Optional[SnapshotManager] = None,
        config: Optional[SnapshotConfig] = None,
        container_resolver: Optional[callable] = None,
    ):
        """
        Initialize the snapshot callback.

        Args:
            snapshot_manager: SnapshotManager instance to use
            config: Configuration for snapshot behavior
            container_resolver: Function to resolve container ID from agent context
        """
        self.snapshot_manager = snapshot_manager or SnapshotManager()
        self.config = config or SnapshotConfig()

        # Function to resolve container ID from agent context
        # Default implementation assumes container info is in kwargs
        self.container_resolver = container_resolver or self._default_container_resolver

        # Track current run state
        self._current_run_id: Optional[str] = None
        self._current_container_id: Optional[str] = None
        self._action_count = 0

        logger.info("SnapshotCallback initialized")

    def _default_container_resolver(self, kwargs: Dict[str, Any]) -> Optional[str]:
        """
        Default container ID resolver.

        Args:
            kwargs: Agent run arguments

        Returns:
            Container ID if found, None otherwise
        """
        # Look for container information in various common locations
        # This is a best-effort implementation that may need customization

        # Check for direct container_id parameter
        if "container_id" in kwargs:
            return kwargs["container_id"]

        # Check for computer/tool objects that might have container info
        tools = kwargs.get("tools", [])
        for tool in tools:
            if hasattr(tool, "container_id"):
                return tool.container_id
            elif hasattr(tool, "name") and hasattr(tool, "attrs"):
                # Possible Computer object
                attrs = getattr(tool, "attrs", {})
                if "container_id" in attrs:
                    return attrs["container_id"]

        # Check in nested configurations
        config = kwargs.get("config", {})
        if isinstance(config, dict) and "container_id" in config:
            return config["container_id"]

        logger.debug("Could not resolve container ID from agent context")
        return None

    def _generate_run_id(self, kwargs: Dict[str, Any]) -> str:
        """
        Generate or extract a run ID from the agent context.

        Args:
            kwargs: Agent run arguments

        Returns:
            Run ID string
        """
        # Look for existing run ID
        if "run_id" in kwargs:
            return kwargs["run_id"]

        # Generate one based on timestamp and container
        import time

        timestamp = int(time.time())
        container_id = self._current_container_id or "unknown"
        return f"run_{container_id}_{timestamp}"

    async def _create_snapshot_if_enabled(
        self,
        trigger: SnapshotTrigger,
        description: Optional[str] = None,
        action_context: Optional[str] = None,
    ) -> None:
        """
        Create a snapshot if the trigger is enabled and container is available.

        Args:
            trigger: Snapshot trigger type
            description: Optional description
            action_context: Optional action context
        """
        if not self._current_container_id:
            logger.debug(f"No container ID available for {trigger.value} snapshot")
            return

        if not await self.snapshot_manager.should_create_snapshot(trigger):
            logger.debug(f"Snapshot trigger {trigger.value} is not enabled")
            return

        try:
            await self.snapshot_manager.create_snapshot(
                container_id=self._current_container_id,
                trigger=trigger,
                description=description,
                action_context=action_context,
                run_id=self._current_run_id,
            )
            logger.info(
                f"Created {trigger.value} snapshot for container {self._current_container_id}"
            )

        except Exception as e:
            logger.error(f"Failed to create {trigger.value} snapshot: {e}")
            # Don't re-raise - snapshots are not critical to agent operation

    # CUA Agent SDK Callback Methods
    # These mirror the AsyncCallbackHandler interface from the CUA SDK

    async def on_run_start(self, kwargs: Dict[str, Any], old_items: List[Dict[str, Any]]) -> None:
        """Called at the start of an agent run loop."""
        try:
            # Resolve container information
            self._current_container_id = self.container_resolver(kwargs)
            self._current_run_id = self._generate_run_id(kwargs)
            self._action_count = 0

            logger.info(
                f"Agent run started - container: {self._current_container_id}, run: {self._current_run_id}"
            )

            # Create run start snapshot if enabled
            await self._create_snapshot_if_enabled(
                SnapshotTrigger.RUN_START,
                description="Snapshot at agent run start",
                action_context="run_start",
            )

        except Exception as e:
            logger.error(f"Error in on_run_start callback: {e}")

    async def on_run_end(
        self,
        kwargs: Dict[str, Any],
        old_items: List[Dict[str, Any]],
        new_items: List[Dict[str, Any]],
    ) -> None:
        """Called at the end of an agent run loop."""
        try:
            # Create run end snapshot if enabled
            await self._create_snapshot_if_enabled(
                SnapshotTrigger.RUN_END,
                description=f"Snapshot at agent run end (completed {self._action_count} actions)",
                action_context="run_end",
            )

            logger.info(f"Agent run ended - run: {self._current_run_id}")

            # Reset run state
            self._current_run_id = None
            self._current_container_id = None
            self._action_count = 0

        except Exception as e:
            logger.error(f"Error in on_run_end callback: {e}")

    async def on_computer_call_start(self, item: Dict[str, Any]) -> None:
        """Called when a computer call is about to start."""
        try:
            self._action_count += 1
            action_type = item.get("action", {}).get("type", "unknown")

            logger.debug(f"Computer call starting: {action_type} (action #{self._action_count})")

            # Create before-action snapshot if enabled
            await self._create_snapshot_if_enabled(
                SnapshotTrigger.BEFORE_ACTION,
                description=f"Snapshot before action: {action_type}",
                action_context=f"before_{action_type}",
            )

        except Exception as e:
            logger.error(f"Error in on_computer_call_start callback: {e}")

    async def on_computer_call_end(
        self, item: Dict[str, Any], result: List[Dict[str, Any]]
    ) -> None:
        """Called when a computer call has completed."""
        try:
            action_type = item.get("action", {}).get("type", "unknown")

            logger.debug(f"Computer call completed: {action_type}")

            # Create after-action snapshot if enabled
            await self._create_snapshot_if_enabled(
                SnapshotTrigger.AFTER_ACTION,
                description=f"Snapshot after action: {action_type}",
                action_context=f"after_{action_type}",
            )

        except Exception as e:
            logger.error(f"Error in on_computer_call_end callback: {e}")

    async def on_function_call_start(self, item: Dict[str, Any]) -> None:
        """Called when a function call is about to start."""
        try:
            function_name = item.get("function", {}).get("name", "unknown")

            logger.debug(f"Function call starting: {function_name}")

            # Count function calls as actions too
            self._action_count += 1

            # Create before-action snapshot if enabled
            await self._create_snapshot_if_enabled(
                SnapshotTrigger.BEFORE_ACTION,
                description=f"Snapshot before function: {function_name}",
                action_context=f"before_function_{function_name}",
            )

        except Exception as e:
            logger.error(f"Error in on_function_call_start callback: {e}")

    async def on_function_call_end(
        self, item: Dict[str, Any], result: List[Dict[str, Any]]
    ) -> None:
        """Called when a function call has completed."""
        try:
            function_name = item.get("function", {}).get("name", "unknown")

            logger.debug(f"Function call completed: {function_name}")

            # Create after-action snapshot if enabled
            await self._create_snapshot_if_enabled(
                SnapshotTrigger.AFTER_ACTION,
                description=f"Snapshot after function: {function_name}",
                action_context=f"after_function_{function_name}",
            )

        except Exception as e:
            logger.error(f"Error in on_function_call_end callback: {e}")

    # Additional utility methods for manual snapshot control

    async def create_manual_snapshot(self, description: str = "Manual snapshot") -> str:
        """
        Create a manual snapshot with the current agent context.

        Args:
            description: Description for the snapshot

        Returns:
            Snapshot ID

        Raises:
            Exception: If no container context is available or snapshot fails
        """
        if not self._current_container_id:
            raise Exception("No active container context for manual snapshot")

        metadata = await self.snapshot_manager.create_snapshot(
            container_id=self._current_container_id,
            trigger=SnapshotTrigger.MANUAL,
            description=description,
            action_context="manual",
            run_id=self._current_run_id,
        )

        return metadata.snapshot_id

    async def restore_latest_snapshot(self, container_name_suffix: str = "restored") -> str:
        """
        Restore the latest snapshot for the current container.

        Args:
            container_name_suffix: Suffix for the restored container name

        Returns:
            ID of the restored container

        Raises:
            Exception: If no snapshots are available or restore fails
        """
        if not self._current_container_id:
            raise Exception("No active container context for snapshot restore")

        snapshots = await self.snapshot_manager.list_snapshots(
            container_id=self._current_container_id, limit=1
        )

        if not snapshots:
            raise Exception(f"No snapshots available for container {self._current_container_id}")

        latest_snapshot = snapshots[0]

        from .models import RestoreOptions

        options = RestoreOptions(
            new_container_name=f"{latest_snapshot.container_name}_{container_name_suffix}"
        )

        return await self.snapshot_manager.restore_snapshot(latest_snapshot.snapshot_id, options)

    def get_current_context(self) -> Dict[str, Any]:
        """
        Get the current agent execution context.

        Returns:
            Dictionary with current context information
        """
        return {
            "container_id": self._current_container_id,
            "run_id": self._current_run_id,
            "action_count": self._action_count,
            "snapshot_config": self.config.model_dump(),
        }
