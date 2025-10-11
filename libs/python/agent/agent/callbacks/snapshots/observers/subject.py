"""
Subject class for managing snapshot event observers.
"""

from typing import List, Dict, Any
import logging

from .base import SnapshotEventObserver

logger = logging.getLogger(__name__)


class SnapshotEventSubject:
    """
    Subject class for managing and notifying snapshot event observers.
    Eliminates if-else statements by delegating to observers.
    """

    def __init__(self):
        """Initialize with empty observer list."""
        self.observers: List[SnapshotEventObserver] = []

    def attach_observer(self, observer: SnapshotEventObserver) -> None:
        """Attach an observer to the subject."""
        self.observers.append(observer)
        logger.debug(f"Attached observer: {observer.get_observer_name()}")

    def detach_observer(self, observer: SnapshotEventObserver) -> None:
        """Detach an observer from the subject."""
        if observer in self.observers:
            self.observers.remove(observer)
            logger.debug(f"Detached observer: {observer.get_observer_name()}")

    async def notify_run_start(self, event_data: Dict[str, Any]) -> None:
        """Notify all interested observers about run start."""
        await self._notify_observers("run_start", event_data, lambda obs: obs.on_run_start(event_data))

    async def notify_run_end(self, event_data: Dict[str, Any]) -> None:
        """Notify all interested observers about run end."""
        await self._notify_observers("run_end", event_data, lambda obs: obs.on_run_end(event_data))

    async def notify_action_end(self, event_data: Dict[str, Any]) -> None:
        """Notify all interested observers about action end."""
        await self._notify_observers("action_end", event_data, lambda obs: obs.on_action_end(event_data))

    async def _notify_observers(self, event_type: str, event_data: Dict[str, Any], notify_func) -> None:
        """
        Notify all observers interested in the event type.
        No if-else statements - uses polymorphic dispatch.
        """
        interested_observers = [
            obs for obs in self.observers
            if obs.is_interested_in_event(event_type)
        ]

        logger.debug(f"Notifying {len(interested_observers)} observers about {event_type}")

        for observer in interested_observers:
            try:
                await notify_func(observer)
            except Exception as e:
                logger.error(f"Observer {observer.get_observer_name()} failed on {event_type}: {e}")

    def get_observer_count(self) -> int:
        """Get the number of attached observers."""
        return len(self.observers)

    def get_observer_names(self) -> List[str]:
        """Get names of all attached observers."""
        return [obs.get_observer_name() for obs in self.observers]