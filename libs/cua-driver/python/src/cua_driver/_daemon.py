"""macOS daemon preference for ``CuaDriver.create()``.

An embedded (in-process) runtime on macOS has no AppKit host, so the agent
cursor overlay cannot be drawn and the permission identity is the host
process (python, node, an IDE) instead of CuaDriver.app. The daemon
(``CuaDriver.app`` running ``serve``) draws the cursor and owns the TCC grants,
so on macOS ``create()`` connects to it, starting it through LaunchServices
when the app is installed. This module has no native dependency so it can be
tested without the staged library.
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import warnings
from typing import Any, Callable, Optional

APP_NAME = "CuaDriver"
APP_PATH = f"/Applications/{APP_NAME}.app"
EMBEDDED_ENV = "CUA_DRIVER_EMBEDDED"
START_TIMEOUT_SECONDS = 15.0

_warned = False


def _warn_once(message: str) -> None:
    global _warned
    if _warned:
        return
    _warned = True
    warnings.warn(message, RuntimeWarning, stacklevel=4)


def launch_app() -> bool:
    """Start the daemon through LaunchServices, like ``permissions grant``."""
    try:
        status = subprocess.run(
            ["/usr/bin/open", "-n", "-g", "-a", APP_NAME, "--args", "serve"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except OSError:
        return False
    return status.returncode == 0


def create_driver(
    connect: Callable[[], Any],
    create_embedded: Callable[[], Any],
    *,
    embedded: bool = False,
    claude_code_compatibility: bool = False,
    platform: Optional[str] = None,
    app_installed: Optional[Callable[[], bool]] = None,
    launch: Callable[[], bool] = launch_app,
    timeout: float = START_TIMEOUT_SECONDS,
    sleep: Callable[[float], None] = time.sleep,
) -> Any:
    """Return a daemon-backed driver on macOS when possible, else embedded."""
    platform = sys.platform if platform is None else platform
    if (
        embedded
        or claude_code_compatibility
        or platform != "darwin"
        or os.environ.get(EMBEDDED_ENV) == "1"
    ):
        return create_embedded()

    if app_installed is None:
        app_installed = lambda: os.path.isdir(APP_PATH)

    driver = connect()
    if driver.is_available():
        return driver

    if app_installed() and launch():
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if driver.is_available():
                return driver
            sleep(0.25)

    _warn_once(
        "CuaDriver.create() is using the in-process runtime, which cannot draw "
        "the agent cursor on macOS and attributes permissions to this process. "
        "Install CuaDriver.app and run `cua-driver serve` (or call "
        "CuaDriver.connect()) to get the agent cursor, or pass embedded=True "
        "to silence this warning."
    )
    return create_embedded()
