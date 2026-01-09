"""Async Chrome browser getters."""

import json
import logging
import sqlite3
from typing import Any, Dict, List, Optional

from .base import get_cache_path, run_powershell

logger = logging.getLogger("winarena.getters_async.chrome")


async def _get_chrome_profile_path(session) -> str:
    """Get the Chrome profile path on the VM."""
    result = await run_powershell(
        session, "$env:USERPROFILE + '\\AppData\\Local\\Google\\Chrome\\User Data\\Default'"
    )
    if result["exit_code"] == 0:
        return result["stdout"].strip()
    return r"C:\Users\Docker\AppData\Local\Google\Chrome\User Data\Default"


async def get_enable_do_not_track(session, config: Dict[str, Any]) -> str:
    """Check if Chrome's Do Not Track is enabled.

    Returns:
        'true' if enabled, 'false' otherwise
    """
    profile_path = await _get_chrome_profile_path(session)
    prefs_path = f"{profile_path}\\Preferences"

    try:
        content = await session.read_text(prefs_path)
        if content:
            prefs = json.loads(content)
            # Navigate to enable_do_not_track setting
            dnt = prefs.get("enable_do_not_track", False)
            return "true" if dnt else "false"
    except Exception as e:
        logger.error(f"Failed to read Chrome preferences: {e}")

    return "false"


async def get_bookmarks(session, config: Dict[str, Any]) -> Optional[Dict]:
    """Get Chrome bookmarks.

    Returns:
        Bookmarks dict or None
    """
    profile_path = await _get_chrome_profile_path(session)
    bookmarks_path = f"{profile_path}\\Bookmarks"

    try:
        content = await session.read_text(bookmarks_path)
        if content:
            return json.loads(content)
    except Exception as e:
        logger.error(f"Failed to read Chrome bookmarks: {e}")

    return None


async def get_history(session, config: Dict[str, Any]) -> Optional[List[Dict]]:
    """Get Chrome browsing history.

    Config:
        limit (int): Maximum number of entries (default: 100)

    Returns:
        List of history entries or None
    """
    profile_path = await _get_chrome_profile_path(session)
    history_path = f"{profile_path}\\History"

    # Download the SQLite database
    local_path = get_cache_path("chrome_history.sqlite")

    try:
        content = await session.read_bytes(history_path)
        if content is None:
            return None

        local_path.write_bytes(content)

        # Query the database
        limit = config.get("limit", 100)
        conn = sqlite3.connect(str(local_path))
        cursor = conn.cursor()

        cursor.execute(
            f"""
            SELECT url, title, visit_count, last_visit_time
            FROM urls
            ORDER BY last_visit_time DESC
            LIMIT {limit}
        """
        )

        rows = cursor.fetchall()
        conn.close()

        history = []
        for url, title, visit_count, last_visit_time in rows:
            history.append(
                {
                    "url": url,
                    "title": title,
                    "visit_count": visit_count,
                    "last_visit_time": last_visit_time,
                }
            )

        return history

    except Exception as e:
        logger.error(f"Failed to get Chrome history: {e}")
        return None


async def get_cookie_data(session, config: Dict[str, Any]) -> Optional[List[Dict]]:
    """Get Chrome cookies.

    Config:
        domain (str): Optional domain filter

    Returns:
        List of cookie entries or None
    """
    profile_path = await _get_chrome_profile_path(session)
    cookies_path = f"{profile_path}\\Network\\Cookies"

    # Download the SQLite database
    local_path = get_cache_path("chrome_cookies.sqlite")

    try:
        content = await session.read_bytes(cookies_path)
        if content is None:
            return None

        local_path.write_bytes(content)

        # Query the database
        domain_filter = config.get("domain", "")
        conn = sqlite3.connect(str(local_path))
        cursor = conn.cursor()

        if domain_filter:
            cursor.execute(
                """
                SELECT host_key, name, path, expires_utc, is_secure
                FROM cookies
                WHERE host_key LIKE ?
            """,
                (f"%{domain_filter}%",),
            )
        else:
            cursor.execute(
                """
                SELECT host_key, name, path, expires_utc, is_secure
                FROM cookies
            """
            )

        rows = cursor.fetchall()
        conn.close()

        cookies = []
        for host_key, name, path, expires_utc, is_secure in rows:
            cookies.append(
                {
                    "host_key": host_key,
                    "name": name,
                    "path": path,
                    "expires_utc": expires_utc,
                    "is_secure": is_secure,
                }
            )

        return cookies

    except Exception as e:
        logger.error(f"Failed to get Chrome cookies: {e}")
        return None


async def get_open_tabs_info(session, config: Dict[str, Any]) -> Optional[List[Dict]]:
    """Get information about open Chrome tabs.

    Note: This requires Chrome to be running with --remote-debugging-port.
    For now, returns empty list.
    """
    logger.warning("get_open_tabs_info not fully implemented for async")
    return []


async def get_default_search_engine(session, config: Dict[str, Any]) -> Optional[str]:
    """Get Chrome's default search engine.

    Returns:
        Search engine name or None
    """
    profile_path = await _get_chrome_profile_path(session)
    prefs_path = f"{profile_path}\\Preferences"

    try:
        content = await session.read_text(prefs_path)
        if content:
            prefs = json.loads(content)
            search_provider = prefs.get("default_search_provider_data", {})
            return search_provider.get("short_name")
    except Exception as e:
        logger.error(f"Failed to get Chrome default search engine: {e}")

    return None
