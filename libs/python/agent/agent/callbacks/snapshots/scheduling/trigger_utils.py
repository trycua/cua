"""
Trigger description utilities for snapshots.
"""

from typing import Optional, Dict, Any
import logging

logger = logging.getLogger(__name__)


class TriggerDescriptor:
    """
    Generates and manages trigger descriptions for snapshots.
    """

    def get_description(self,
                       event_type: str,
                       action_details: Optional[Dict[str, Any]] = None,
                       action_count: int = 0) -> str:
        """
        Generate a description of what triggered the snapshot.

        Args:
            event_type: Type of event ("run_start", "run_end", "action", "manual")
            action_details: Optional details about the action
            action_count: Current action count

        Returns:
            Description string for the trigger
        """
        if event_type == "run_start":
            return "run_start"
        elif event_type == "run_end":
            return "run_end"
        elif event_type == "action":
            if action_details:
                action_type = self._extract_action_type(action_details)
                return f"after_action_{action_type}"
            return f"after_action_{action_count}"
        elif event_type == "manual":
            return "manual"
        else:
            return event_type

    def _extract_action_type(self, action_details: Dict[str, Any]) -> str:
        """
        Extract the action type from action details.

        Args:
            action_details: Dictionary containing action information

        Returns:
            Action type string
        """
        if isinstance(action_details, dict) and "action" in action_details:
            action = action_details["action"]
            if isinstance(action, dict) and "type" in action:
                return action["type"]
        return "unknown"