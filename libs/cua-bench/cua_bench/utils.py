"""Utility functions for synthetic data generation."""

from __future__ import annotations

import asyncio
import re
from typing import Any, Dict, List, Literal, Optional, Tuple

from .computers.base import DesktopSetupConfig
from .environment import Environment
from .types import Snapshot


async def render_snapshot_async(
    setup_config: Dict[str, Any],
    snapshot: Dict[str, Any],
    screenshot_delay: float = 0,
    provider: Literal["webtop", "computer"] = "webtop",
) -> bytes:
    """Render a snapshot and return screenshot bytes (async).

    Args:
        provider: Provider name ("webtop" or "computer")
        setup_config: Configuration dict for create_sandbox setup_config parameter
        snapshot: Snapshot dict containing windows and other state
        screenshot_delay: Delay in seconds before taking screenshot

    Returns:
        Screenshot as bytes
    """
    # Create environment
    env = Environment()
    env.headless = True

    try:
        # Create sandbox with specified configuration
        await env.create_sandbox(
            provider=provider,
            setup_config=DesktopSetupConfig(**setup_config),
        )

        # Close all windows
        if env.session:
            await env.session.close_all_windows()

        # Launch windows from snapshot
        windows = snapshot.get("windows", [])
        for w in windows:
            html = w.get("html") or ""
            html = _strip_scripts(html)
            title = w.get("title") or "Window"
            x = w.get("x")
            y = w.get("y")
            width = int(w.get("width") or 400)
            height = int(w.get("height") or 300)
            icon = w.get("icon")
            use_inner_size = w.get("use_inner_size", False)
            title_bar_style = w.get("title_bar_style", "default")

            if env.session:
                await env.session.launch_window(
                    html=html,
                    title=title,
                    x=x,
                    y=y,
                    width=width,
                    height=height,
                    icon=icon,
                    use_inner_size=use_inner_size,
                    title_bar_style=title_bar_style,
                )

        # Wait for screenshot delay
        if screenshot_delay > 0:
            await asyncio.sleep(screenshot_delay)

        # Take screenshot
        if env.session:
            screenshot_bytes = await env.session.screenshot()
        else:
            raise RuntimeError("Session not initialized")

        return screenshot_bytes

    finally:
        # Clean up
        await env.close()


async def render_windows_async(
    setup_config: Dict[str, Any],
    windows: List[Dict[str, Any]],
    screenshot_delay: float = 0,
    provider: Literal["webtop", "computer"] = "webtop",
    return_snapshot: bool = False,
    scroll_into_view: Optional[str] = None,
) -> bytes | Tuple[bytes, Snapshot]:
    """Render windows and return screenshot bytes (async).

    Args:
        provider: Provider name ("webtop" or "computer")
        setup_config: Configuration dict for create_sandbox setup_config parameter
        windows: List of window dicts to pass directly to launch_window
        screenshot_delay: Delay in seconds before taking screenshot
        return_snapshot: If True, return tuple of (bytes, Snapshot) instead of just bytes
        scroll_into_view: Optional CSS selector for an element to scroll into view

    Returns:
        Screenshot as bytes, or tuple of (bytes, Snapshot) if return_snapshot=True
    """
    # Create environment
    env = Environment()
    env.headless = True

    try:
        # Create sandbox with specified configuration
        await env.create_sandbox(
            provider=provider,
            setup_config=DesktopSetupConfig(**setup_config),
        )

        # Close all windows
        if env.session:
            await env.session.close_all_windows()

        # Launch windows and track PIDs
        pids = []
        for window in windows:
            # Strip scripts from html if present
            if "html" in window:
                window = dict(window)  # Make a copy to avoid mutating input
                window["html"] = _strip_scripts(window.get("html", ""))

            if env.session:
                pid = await env.session.launch_window(**window)
                pids.append(pid)

        # Scroll element into view if requested
        if scroll_into_view and pids and env.session:
            # Apply scroll to the first window (or all windows if needed)
            for pid in pids:
                try:
                    scroll_js = f"""
                    (function() {{
                        const element = document.querySelector({repr(scroll_into_view)});
                        if (element) {{
                            element.scrollIntoView({{ behavior: 'instant', block: 'center' }});
                        }}
                    }})();
                    """
                    await env.session.execute_javascript(pid, scroll_js)
                except Exception as e:
                    # Continue even if scroll fails for a window
                    print(f"Warning: Failed to scroll element in window {pid}: {e}")

        # Wait for screenshot delay
        if screenshot_delay > 0:
            await asyncio.sleep(screenshot_delay)

        # Take screenshot
        if env.session:
            screenshot_bytes = await env.session.screenshot()
        else:
            raise RuntimeError("Session not initialized")

        if return_snapshot:
            if env.session:
                snapshot = await env.session.get_snapshot()
            else:
                raise RuntimeError("Session not initialized")
            return (screenshot_bytes, snapshot)
        else:
            return screenshot_bytes

    finally:
        # Clean up
        await env.close()


def render_snapshot(
    setup_config: Dict[str, Any],
    snapshot: Dict[str, Any],
    screenshot_delay: float = 0,
    provider: Literal["webtop", "computer"] = "webtop",
) -> bytes:
    """Render a snapshot and return screenshot bytes (sync wrapper).

    Args:
        provider: Provider name ("webtop" or "computer")
        setup_config: Configuration dict for create_sandbox setup_config parameter
        snapshot: Snapshot dict containing windows and other state
        screenshot_delay: Delay in seconds before taking screenshot

    Returns:
        Screenshot as bytes
    """
    return asyncio.run(render_snapshot_async(setup_config, snapshot, screenshot_delay, provider))


def render_windows(
    setup_config: Dict[str, Any],
    windows: List[Dict[str, Any]],
    screenshot_delay: float = 0,
    provider: Literal["webtop", "computer"] = "webtop",
    return_snapshot: bool = False,
    scroll_into_view: Optional[str] = None,
) -> bytes | Tuple[bytes, Snapshot]:
    """Render windows and return screenshot bytes (sync wrapper).

    Args:
        provider: Provider name ("webtop" or "computer")
        setup_config: Configuration dict for create_sandbox setup_config parameter
        windows: List of window dicts to pass directly to launch_window
        screenshot_delay: Delay in seconds before taking screenshot
        return_snapshot: If True, return tuple of (bytes, Snapshot) instead of just bytes
        scroll_into_view: Optional CSS selector for an element to scroll into view

    Returns:
        Screenshot as bytes, or tuple of (bytes, Snapshot) if return_snapshot=True
    """
    return asyncio.run(
        render_windows_async(
            setup_config, windows, screenshot_delay, provider, return_snapshot, scroll_into_view
        )
    )


# --- Helper functions ---

_script_re = re.compile(r"<script\b[^>]*>.*?</script>", re.IGNORECASE | re.DOTALL)


def _strip_scripts(s: str) -> str:
    """Strip script tags from HTML string."""
    return _script_re.sub("", s)
