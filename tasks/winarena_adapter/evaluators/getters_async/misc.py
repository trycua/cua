"""Async misc getters - rules, accessibility tree."""

import json
import logging
from typing import Any, Dict, Optional

logger = logging.getLogger("winarena.getters_async.misc")


async def get_rule(session, config: Dict[str, Any]) -> Dict[str, Any]:
    """Return inline rules from config.

    This is a pass-through getter for inline expected values.

    Config:
        rules (dict): The rules to return

    Returns:
        The rules dict
    """
    return config.get("rules", {})


async def get_accessibility_tree(session, config: Dict[str, Any]) -> Optional[str]:
    """Get the accessibility tree from the VM.

    Returns:
        Accessibility tree as JSON string, or None
    """
    try:
        tree = await session.get_accessibility_tree()
        if isinstance(tree, dict):
            return json.dumps(tree)
        return str(tree)
    except Exception as e:
        logger.error(f"Failed to get accessibility tree: {e}")
        return None


async def get_rule_relativeTime(session, config: Dict[str, Any]) -> Dict[str, Any]:
    """Return rules with relative time calculation.

    Note: This is a simplified implementation.
    """
    return config.get("rules", {})


async def get_time_diff_range(session, config: Dict[str, Any]) -> Dict[str, Any]:
    """Return time difference range rules.

    Note: This is a simplified implementation.
    """
    return config.get("rules", {})
