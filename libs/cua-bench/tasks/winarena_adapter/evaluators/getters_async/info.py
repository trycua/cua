"""Async info getters - screen size, window info, wallpaper."""

import logging
from typing import Any, Dict, List, Optional

logger = logging.getLogger("winarena.getters_async.info")


async def get_vm_screen_size(session, config: Dict[str, Any]) -> Optional[Dict[str, int]]:
    """Get the VM screen size.

    Returns:
        Dict with 'width' and 'height', or None
    """
    try:
        result = await session.get_screen_size()
        return result
    except Exception as e:
        logger.error(f"Failed to get screen size: {e}")
        return None


async def get_vm_window_size(session, config: Dict[str, Any]) -> Optional[Dict[str, int]]:
    """Get the size of a window.

    Config:
        app_class_name (str): Application class name or window ID

    Returns:
        Dict with 'width' and 'height', or None
    """
    app_class_name = config.get("app_class_name")
    window_id = config.get("window_id")

    try:
        if window_id:
            size = await session.get_window_size(window_id)
            return size
        elif app_class_name:
            windows = await session.get_application_windows(app_class_name)
            if windows:
                size = await session.get_window_size(windows[0])
                return size
    except Exception as e:
        logger.error(f"Failed to get window size: {e}")

    return None


async def get_vm_wallpaper(session, config: Dict[str, Any]) -> Optional[bytes]:
    """Get the current wallpaper image.

    Returns:
        Wallpaper image as bytes, or None
    """
    from .base import run_powershell

    result = await run_powershell(
        session, "(Get-ItemProperty 'HKCU:\\Control Panel\\Desktop' -Name Wallpaper).Wallpaper"
    )

    if result["exit_code"] == 0 and result["stdout"]:
        wallpaper_path = result["stdout"].strip()
        if wallpaper_path:
            try:
                content = await session.read_bytes(wallpaper_path)
                return content
            except Exception as e:
                logger.error(f"Failed to read wallpaper file: {e}")

    return None


async def get_list_directory(session, config: Dict[str, Any]) -> Optional[List[str]]:
    """List contents of a directory.

    Config:
        path (str): Directory path

    Returns:
        List of filenames, or None
    """
    path = config.get("path")
    if not path:
        return None

    try:
        result = await session.list_dir(path)
        return result
    except Exception as e:
        logger.error(f"Failed to list directory: {e}")
        return None
