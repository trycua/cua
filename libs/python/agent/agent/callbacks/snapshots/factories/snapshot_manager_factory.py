"""
Main factory for creating snapshot manager components without if-else statements.
"""

from typing import Optional, Any

from ..strategies.scheduling import SchedulingStrategyFactory
from ..strategies.scheduling.trigger_utils import TriggerDescriptor
from ..states.context import SnapshotContext
from ..commands.invoker import CommandInvoker
from ..observers.subject import SnapshotEventSubject
from ..core.creator import SnapshotCreator
from ..metadata import MetadataManager
from ..provider import ProviderAdapter
from ..storage import StorageManager
from .cleanup_factory import CleanupPolicyFactory
from .observer_factory import ObserverFactory


class SnapshotManagerFactory:
    """
    Main factory for creating snapshot manager components.
    Eliminates all if-else statements in component creation.
    """

    @staticmethod
    def create_complete_system(
        computer: Optional[Any] = None,
        snapshot_interval: str = "manual",
        max_snapshots: int = 10,
        retention_days: int = 7,
        metadata_dir: str = "/tmp/cua_snapshots",
        auto_cleanup: bool = True,
        snapshot_prefix: str = "cua-snapshot"
    ) -> dict:
        """
        Create complete snapshot management system.
        No if-else statements - uses factories and composition.

        Returns:
            Dictionary containing all initialized components
        """
        # Create core components
        storage_manager = StorageManager(metadata_dir)
        provider_adapter = ProviderAdapter(computer)
        snapshot_creator = SnapshotCreator(provider_adapter, snapshot_prefix)
        metadata_manager = MetadataManager(storage_manager)

        # Create strategy and state components
        scheduling_strategy = SchedulingStrategyFactory.create(snapshot_interval)
        trigger_descriptor = TriggerDescriptor()

        container_name = computer.config.name if computer and hasattr(computer, 'config') else "default"
        snapshot_context = SnapshotContext(container_name)

        # Create command system
        command_invoker = CommandInvoker()

        # Create cleanup system
        cleanup_policies = [
            CleanupPolicyFactory.create_standard_policy(
                max_snapshots, retention_days, auto_cleanup
            )
        ]

        # Create observer system
        observers = ObserverFactory.create_standard_observers(
            scheduling_strategy, command_invoker, trigger_descriptor, cleanup_policies
        )

        event_subject = SnapshotEventSubject()
        for observer in observers:
            event_subject.attach_observer(observer)

        return {
            "storage_manager": storage_manager,
            "provider_adapter": provider_adapter,
            "snapshot_creator": snapshot_creator,
            "metadata_manager": metadata_manager,
            "scheduling_strategy": scheduling_strategy,
            "trigger_descriptor": trigger_descriptor,
            "snapshot_context": snapshot_context,
            "command_invoker": command_invoker,
            "cleanup_policies": cleanup_policies,
            "event_subject": event_subject,
            "container_name": container_name
        }

    @staticmethod
    def create_minimal_system(computer: Optional[Any] = None) -> dict:
        """Create minimal snapshot system with basic components."""
        storage_manager = StorageManager()
        provider_adapter = ProviderAdapter(computer)
        snapshot_creator = SnapshotCreator(provider_adapter)
        metadata_manager = MetadataManager(storage_manager)
        scheduling_strategy = SchedulingStrategyFactory.create("manual")

        container_name = computer.config.name if computer and hasattr(computer, 'config') else "default"
        snapshot_context = SnapshotContext(container_name)

        return {
            "storage_manager": storage_manager,
            "provider_adapter": provider_adapter,
            "snapshot_creator": snapshot_creator,
            "metadata_manager": metadata_manager,
            "scheduling_strategy": scheduling_strategy,
            "snapshot_context": snapshot_context,
            "container_name": container_name
        }