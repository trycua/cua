"""Async registry getters for Windows."""

import logging
from typing import Any, Dict, Optional

from .base import get_registry_value, get_registry_binary

logger = logging.getLogger("winarena.getters_async.registry")


async def get_registry_key(session, config: Dict[str, Any]) -> Dict[str, Any]:
    """Get a registry key value.

    Config:
        key_path (str): Registry key path (e.g., "HKCU:\\Software\\...")
        value_name (str): Name of the value to retrieve

    Returns:
        Dict with 'status' and 'output'
    """
    key_path = config.get("key_path") or config.get("path")
    value_name = config.get("value_name") or config.get("setting_path") or config.get("name")

    if not key_path or not value_name:
        return {"status": "error", "output": ""}

    value = await get_registry_value(session, key_path, value_name)

    if value is not None:
        return {"status": "success", "output": value + "\n"}
    return {"status": "error", "output": ""}


async def get_registry_key_binary(session, config: Dict[str, Any]) -> Optional[bytes]:
    """Get a registry binary value.

    Config:
        key_path (str): Registry key path
        value_name (str): Name of the value to retrieve

    Returns:
        Binary value as bytes, or None
    """
    key_path = config.get("key_path") or config.get("path")
    value_name = config.get("value_name") or config.get("setting_path") or config.get("name")

    if not key_path or not value_name:
        return None

    return await get_registry_binary(session, key_path, value_name)
