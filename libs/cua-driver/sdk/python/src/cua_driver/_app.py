"""App — represents a running macOS application managed by cua-driver."""

from __future__ import annotations

import re as _re
import subprocess
import time
from typing import TYPE_CHECKING, Optional

if TYPE_CHECKING:
    from ._client import DriverClient

from ._window import Window


def kill_app(bundle_id: str, force: bool = False) -> None:
    """Kill all running instances of the app with *bundle_id*.

    Sends a graceful quit event via AppleScript.  With ``force=True``,
    also sends SIGKILL to any remaining processes via System Events.

    No :class:`DriverClient` required — safe to call before a test suite
    starts.

    Examples
    --------
    >>> kill_app("com.apple.calculator")          # graceful quit
    >>> kill_app("com.apple.calculator", force=True)  # force kill
    """
    # Graceful quit via AppleScript — ignore timeout (app may show a dialog)
    try:
        subprocess.run(
            ["osascript", "-e", f'tell application id "{bundle_id}" to quit'],
            check=False,
            timeout=5,
        )
    except subprocess.TimeoutExpired:
        pass
    if force:
        time.sleep(0.5)
        # Collect PIDs via System Events and send SIGKILL
        try:
            result = subprocess.run(
                [
                    "osascript", "-e",
                    f'tell application "System Events" to get unix id of '
                    f'(every process whose bundle identifier is "{bundle_id}")',
                ],
                capture_output=True, text=True, check=False, timeout=8,
            )
            for pid_str in _re.split(r"[,\s]+", result.stdout.strip()):
                if pid_str.strip().isdigit():
                    subprocess.run(["kill", "-9", pid_str.strip()], check=False)
        except subprocess.TimeoutExpired:
            pass


def _resolve_window_id(client: "DriverClient", pid: int) -> int:
    """Pick the best ``window_id`` for *pid*.

    Prefers on-screen windows on the current Space (sorted by z_index),
    then falls back to the largest window by area.  Raises ``RuntimeError``
    when the pid has no windows at all.
    """
    result = client.call_tool("list_windows", {"pid": pid})
    windows = result.get("structuredContent", {}).get("windows", [])
    if not windows:
        raise RuntimeError(f"pid {pid} has no windows")

    preferred = [
        w for w in windows
        if w.get("is_on_screen") and w.get("on_current_space") is not False
    ]
    if preferred:
        preferred.sort(key=lambda w: w.get("z_index", 0), reverse=True)
        return preferred[0]["window_id"]

    def _area(w: dict) -> float:
        b = w.get("bounds", {})
        return float(b.get("width", 0)) * float(b.get("height", 0))

    windows.sort(key=_area, reverse=True)
    return windows[0]["window_id"]


class App:
    """A handle to a running macOS application.

    Use :meth:`launch` to start an app, or construct directly if you
    already have a pid.

    Context-manager support ensures the process is quit on exit::

        with App.launch("com.apple.calculator", driver=client) as app:
            window = app.main_window()
            ...
    """

    def __init__(
        self,
        client: "DriverClient",
        pid: int,
        bundle_id: str,
    ) -> None:
        self._client = client
        self._pid = pid
        self._bundle_id = bundle_id

    # ------------------------------------------------------------------
    # Factory
    # ------------------------------------------------------------------

    @classmethod
    def launch(
        cls,
        bundle_id: str,
        client: "DriverClient",
        settle: float = 1.0,
    ) -> "App":
        """Launch *bundle_id* and return an :class:`App` handle.

        Parameters
        ----------
        bundle_id:
            macOS bundle identifier, e.g. ``"com.apple.calculator"``.
        client:
            An open :class:`~cua_driver.DriverClient`.
        settle:
            Seconds to wait after launch before returning.  Increase
            for slow-starting apps.
        """
        result = client.call_tool("launch_app", {"bundle_id": bundle_id})
        pid = result["structuredContent"]["pid"]
        if settle > 0:
            time.sleep(settle)
        return cls(client, pid, bundle_id)

    # ------------------------------------------------------------------
    # Context manager
    # ------------------------------------------------------------------

    def __enter__(self) -> "App":
        return self

    def __exit__(self, *exc: object) -> None:
        self.quit()

    # ------------------------------------------------------------------
    # Window access
    # ------------------------------------------------------------------

    def main_window(self, timeout: float = 5.0) -> Window:
        """Return the best available window for this app.

        Retries until *timeout* to handle apps that take a moment to
        open their first window.
        """
        deadline = time.monotonic() + timeout
        last_exc: Exception = RuntimeError("no windows")
        while True:
            try:
                window_id = _resolve_window_id(self._client, self._pid)
                return Window(self._client, self._pid, window_id)
            except RuntimeError as exc:
                last_exc = exc
                if time.monotonic() >= deadline:
                    raise last_exc
                time.sleep(0.3)

    def get_window(
        self,
        title: Optional[str] = None,
        timeout: float = 5.0,
    ) -> Window:
        """Return a window whose title contains *title*.

        If *title* is ``None``, returns the first available window.
        """
        deadline = time.monotonic() + timeout
        while True:
            result = self._client.call_tool("list_windows", {"pid": self._pid})
            windows = result.get("structuredContent", {}).get("windows", [])
            for w in windows:
                if title is None or title in w.get("title", ""):
                    return Window(self._client, self._pid, w["window_id"])
            if time.monotonic() >= deadline:
                raise TimeoutError(
                    f"Window with title={title!r} not found for pid={self._pid} "
                    f"within {timeout}s"
                )
            time.sleep(0.3)

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    def quit(self) -> None:
        """Terminate the app process (SIGTERM to its pid)."""
        subprocess.run(["kill", str(self._pid)], check=False)

    def force_quit(self) -> None:
        """Force-kill the app process (SIGKILL to its pid)."""
        subprocess.run(["kill", "-9", str(self._pid)], check=False)

    @classmethod
    def kill_by_bundle_id(cls, bundle_id: str, force: bool = False) -> None:
        """Kill all running instances of *bundle_id*.

        Convenience wrapper around the module-level :func:`kill_app`.

        Example::

            App.kill_by_bundle_id("com.apple.calculator")
        """
        kill_app(bundle_id, force=force)

    # ------------------------------------------------------------------
    # Properties
    # ------------------------------------------------------------------

    @property
    def pid(self) -> int:
        return self._pid

    @property
    def bundle_id(self) -> str:
        return self._bundle_id
