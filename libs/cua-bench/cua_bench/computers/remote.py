"""Remote desktop session for cua-bench.

This module provides a DesktopSession implementation that uses the official
cua-computer SDK to communicate with cua-computer-server running inside any
golden environment (linux-docker, windows-qemu, linux-qemu, android-qemu).

This is the unified desktop session implementation that supports:
- Native environments (Docker containers, QEMU VMs)
- Remote connections to pre-existing cua-computer-server instances
- Full bench_ui integration (pywebview windows, JS execution, element queries)
"""

from __future__ import annotations

import asyncio
import time
from pathlib import Path
from typing import Any, Dict, Literal, Optional
from urllib.parse import urlparse

from ..types import (
    Action,
    ClickAction,
    DoneAction,
    DoubleClickAction,
    DragAction,
    HotkeyAction,
    KeyAction,
    MiddleClickAction,
    MoveToAction,
    RightClickAction,
    ScrollAction,
    Snapshot,
    TypeAction,
    WaitAction,
    WindowSnapshot,
)
from .base import DesktopSetupConfig

# HTML template with Tailwind CSS for auto-wrapping incomplete HTML
_HTML_TEMPLATE = (
    "<!doctype html>\n"
    "<html>\n"
    "  <head>\n"
    '    <meta charset="UTF-8" />\n'
    '    <meta name="viewport" content="width=device-width, initial-scale=1.0" />\n'
    '    <script src="https://cdn.jsdelivr.net/npm/@tailwindcss/browser@4"></script>\n'
    '    <script src="https://cdn.jsdelivr.net/npm/iconify-icon@3.0.2/dist/iconify-icon.min.js"></script>\n'
    "  </head>\n"
    "  <body>\n"
    "{content}\n"
    "  </body>\n"
    "</html>\n"
)


class RemoteDesktopSession:
    """Unified desktop session using cua-computer SDK.

    Supports two modes:
    1. **Full lifecycle mode** (default): Computer SDK manages container/VM
       - Pass config via constructor kwargs or start(config={...})
       - SDK starts container, waits for boot, connects

    2. **Client-only mode**: Connect to pre-existing cua-computer-server
       - Pass api_url to connect to existing server
       - Used by 2-container architecture, batch execution

    Works with any golden environment type:
    - linux-docker: trycua/cua-xfce container
    - windows-qemu: Windows 11 VM
    - linux-qemu: Linux VM
    - android-qemu: Android VM

    Supports full bench_ui integration when bench_ui is installed in the
    remote environment, enabling:
    - launch_window() with HTML content via pywebview
    - execute_javascript() for DOM manipulation
    - get_element_rect() for element location queries
    - click_element() / right_click_element() for element-based interaction
    """

    # Timeout settings
    DEFAULT_TIMEOUT = 30
    SCREENSHOT_TIMEOUT = 10

    def __init__(
        self,
        api_url: str = "",
        vnc_url: str = "",
        width: int = 1920,
        height: int = 1080,
        os_type: str = "linux",
        image: str = "",
        provider_type: str = "docker",
        memory: str = "8GB",
        cpu: str = "4",
        name: str = "",
        storage: str = "",
        ephemeral: bool = True,
        headless: bool = True,
        **kwargs,
    ):
        """Initialize RemoteDesktopSession.

        Usage:
            # Preferred: async context manager
            async with RemoteDesktopSession(os_type="linux") as session:
                await session.screenshot()

            # Alternative: manual lifecycle
            session = RemoteDesktopSession(os_type="linux")
            await session.start()
            try:
                await session.screenshot()
            finally:
                await session.close()

        Args:
            api_url: URL of pre-existing server (e.g., "http://localhost:5000").
                     If provided, uses client-only mode.
                     If empty, uses full lifecycle mode (SDK manages container).
            vnc_url: URL for VNC/noVNC access to the environment
            width: Screen width
            height: Screen height
            os_type: Operating system type ("linux", "windows", "android")
            image: Docker image to use (e.g., "trycua/cua-xfce:latest")
            provider_type: VM provider type ("docker", "lume", "cloud")
            memory: VM memory allocation (e.g., "8GB")
            cpu: VM CPU allocation (e.g., "4")
            name: Container/VM name (auto-generated if empty)
            storage: Path for persistent storage
            ephemeral: Whether to use ephemeral storage (default True)
            headless: If False, opens VNC preview in browser on start
        """
        self._api_url = api_url.rstrip("/") if api_url else ""
        self._vnc_url = vnc_url
        self._width = width
        self._height = height
        self._os_type = os_type

        # Full lifecycle config (used when api_url is empty)
        self._image = image
        self._provider_type = provider_type
        self._memory = memory
        self._cpu = cpu
        self._name = name
        self._storage = storage
        self._ephemeral = ephemeral
        self._headless = headless

        # Determine mode based on api_url
        self._client_only_mode = bool(api_url)

        # Parse API URL to extract host and port (for client-only mode)
        if api_url:
            parsed = urlparse(self._api_url)
            self._api_host = parsed.hostname or "localhost"
            self._api_port = parsed.port or 5000
        else:
            self._api_host = "localhost"
            self._api_port = 8000  # Default SDK port

        # Parse VNC URL for port
        vnc_parsed = urlparse(vnc_url) if vnc_url else None
        self._vnc_port = vnc_parsed.port if vnc_parsed else 8006

        # Computer SDK instance (lazy initialized)
        self._computer = None
        self._initialized = False

        # Track PIDs of windows launched via bench_ui (pywebview)
        self._webview_pids: set[int] = set()

    async def step(self, action: Action) -> None:
        """Execute an action (alias for execute_action, for env.step() compatibility)."""
        await self.execute_action(action)

    # =========================================================================
    # Async Context Manager & Lifecycle
    # =========================================================================

    async def __aenter__(self) -> "RemoteDesktopSession":
        """Async context manager entry - initialize and start the session."""
        await self.start()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        """Async context manager exit - cleanup resources."""
        await self.close()

    async def start(
        self,
        config: Optional[DesktopSetupConfig] = None,
        headless: Optional[bool] = None,
    ) -> None:
        """Start the session and connect to the environment.

        Args:
            config: Optional configuration to apply before starting.
            headless: If False, opens VNC preview in browser. Defaults to
                constructor value if not specified.

        Example:
            # Using constructor params (preferred)
            async with RemoteDesktopSession(os_type="linux") as session:
                await session.screenshot()

            # Or with config dict
            session = RemoteDesktopSession()
            await session.start(config={"os_type": "linux", "width": 1920})
        """
        # Apply config if provided
        if config:
            if "width" in config:
                self._width = config["width"]
            if "height" in config:
                self._height = config["height"]
            if "os_type" in config:
                self._os_type = config["os_type"]

            # Full lifecycle config (only used when not in client-only mode)
            if not self._client_only_mode:
                if "image" in config:
                    self._image = config["image"]
                if "memory" in config:
                    self._memory = config["memory"]
                if "cpu" in config:
                    self._cpu = config["cpu"]
                if "storage" in config:
                    self._storage = config["storage"]
                if "provider_type" in config:
                    self._provider_type = config["provider_type"]

        # Use provided headless or fall back to constructor value
        effective_headless = headless if headless is not None else self._headless

        await self._ensure_computer()

        # Open VNC preview if not headless
        if not effective_headless:
            import webbrowser

            await asyncio.sleep(1)  # Wait for VNC to be ready
            webbrowser.open(self.vnc_url)

    async def _ensure_computer(self):
        """Ensure the Computer SDK instance is initialized and connected."""
        if self._initialized and self._computer is not None:
            return

        from computer import Computer

        if self._client_only_mode:
            # Client-only mode: connect to pre-existing server
            self._computer = Computer(
                os_type=self._os_type,
                use_host_computer_server=True,
                api_host=self._api_host,
                api_port=self._api_port,
                noVNC_port=self._vnc_port,
            )
        else:
            # Full lifecycle mode: SDK manages container/VM
            image = self._image
            if not image:
                # Default images based on os_type
                if self._os_type in ("windows", "win11", "win10"):
                    image = "trycua/cua-qemu-windows:latest"
                else:
                    image = "trycua/cua-xfce:latest"

            self._computer = Computer(
                os_type=self._os_type,
                provider_type=self._provider_type,
                image=image,
                display=f"{self._width}x{self._height}",
                memory=self._memory,
                cpu=self._cpu,
                name=self._name or None,
                storage=self._storage or None,
                ephemeral=self._ephemeral,
                noVNC_port=self._vnc_port if self._vnc_port != 8006 else None,
            )

        # Initialize and wait for connection
        await self._computer.run()
        self._initialized = True

        # Update VNC URL after initialization (SDK may have allocated ports)
        if not self._client_only_mode and hasattr(self._computer, "noVNC_port"):
            self._vnc_port = self._computer.noVNC_port
            self._vnc_url = f"http://localhost:{self._vnc_port}"

    @property
    def interface(self):
        """Get the computer interface for direct SDK access."""
        if self._computer is None:
            raise RuntimeError("Session not initialized. Call _ensure_computer() first.")
        return self._computer.interface

    # =========================================================================
    # DesktopSession Protocol Implementation
    # =========================================================================

    async def serve_static(self, url_path: str, local_path: str) -> None:
        """Serve static files - not applicable for remote environments."""
        raise NotImplementedError("Remote sessions do not support static file serving")

    async def launch_window(
        self,
        url: Optional[str] = None,
        *,
        html: Optional[str] = None,
        folder: Optional[str] = None,
        title: str = "Window",
        x: Optional[int] = None,
        y: Optional[int] = None,
        width: int = 600,
        height: int = 400,
        icon: Optional[str] = None,
        use_inner_size: bool = False,
        title_bar_style: str = "default",
    ) -> int | str:
        """Launch a window in the remote environment using bench_ui (pywebview).

        Supports:
        - url: Open a URL in a pywebview window
        - html: Display HTML content in a pywebview window
        - folder: Copy folder to remote and serve it in a pywebview window

        Returns:
            Process ID of the pywebview window (int)
        """
        await self._ensure_computer()

        remote_folder_path = None
        html_content = None
        target_url = url

        # Handle folder parameter - copy folder to remote and serve it
        if folder is not None:
            import hashlib

            # Calculate folder hash for unique tmp directory
            folder_hash = hashlib.md5(folder.encode()).hexdigest()[:8]

            # Get target directory path on remote system
            @self._computer.python_command(use_system_python=True)
            def _get_tmp_dir(folder_hash):
                import os
                import tempfile

                tmp_base = tempfile.gettempdir()
                return os.path.join(tmp_base, f"cua_folder_{folder_hash}")

            remote_folder_path = await _get_tmp_dir(folder_hash)

            # Copy folder contents to remote system
            local_folder = Path(folder)
            if not local_folder.exists() or not local_folder.is_dir():
                raise ValueError(f"Folder does not exist or is not a directory: {folder}")

            # Create remote directory
            await self.interface.create_dir(remote_folder_path)

            # Recursively copy all files
            for item in local_folder.rglob("*"):
                if item.is_file():
                    relative_path = item.relative_to(local_folder)
                    remote_path = f"{remote_folder_path}/{relative_path.as_posix()}"

                    # Create parent directory if needed
                    remote_parent = "/".join(remote_path.split("/")[:-1])
                    if not await self.interface.directory_exists(remote_parent):
                        await self.interface.create_dir(remote_parent)

                    # Copy file content
                    content = item.read_bytes()
                    await self.interface.write_bytes(remote_path, content)

            # Unset URL and html
            target_url = None
            html = None

        # Handle HTML content - write to temp folder to avoid argument length limits
        if html is not None:
            import hashlib

            # Wrap incomplete HTML with template
            html_content = (
                html
                if "<html" in (html or "").lower()
                else _HTML_TEMPLATE.replace("{content}", html)
            )

            # Create temp folder for HTML file
            html_hash = hashlib.md5(html_content.encode()).hexdigest()[:8]

            @self._computer.python_command(use_system_python=True)
            def _get_html_tmp_dir(html_hash):
                import os
                import tempfile

                tmp_base = tempfile.gettempdir()
                return os.path.join(tmp_base, f"cua_html_{html_hash}")

            remote_folder_path = await _get_html_tmp_dir(html_hash)

            # Create remote directory
            await self.interface.create_dir(remote_folder_path)

            # Write HTML to index.html in remote folder
            html_file_path = f"{remote_folder_path}/index.html"
            await self.interface.write_text(html_file_path, html_content)

            # Unset URL and html (will use folder instead)
            target_url = None
            html_content = None

        # Launch window via bench_ui on remote
        @self._computer.python_command(use_system_python=True)
        def _open(
            url, html, folder, title, x, y, width, height, icon, use_inner_size, title_bar_style
        ):
            from bench_ui import launch_window

            return launch_window(
                url=url,
                html=html,
                folder=folder,
                title=title,
                x=x,
                y=y,
                width=width,
                height=height,
                icon=icon,
                use_inner_size=use_inner_size,
                title_bar_style=title_bar_style,
            )

        pid = await _open(
            target_url,
            html_content,
            remote_folder_path,
            title,
            x,
            y,
            width,
            height,
            icon,
            use_inner_size,
            title_bar_style,
        )
        self._webview_pids.add(pid)
        return pid

    async def get_element_rect(
        self,
        pid: int | str,
        selector: str,
        *,
        space: Literal["window", "screen"] = "window",
        timeout: float = 0.5,
    ) -> dict[str, Any] | None:
        """Get element rect by CSS selector using bench_ui.

        Args:
            pid: Process ID of the pywebview window
            selector: CSS selector for the element
            space: Coordinate space - "window" or "screen"
            timeout: Maximum time to wait for element

        Returns:
            Dict with x, y, width, height or None if not found
        """
        await self._ensure_computer()

        @self._computer.python_command(use_system_python=True)
        def _get_rect(pid, selector, space):
            from bench_ui import get_element_rect

            return get_element_rect(pid, selector, space=space)

        retry_interval = max(0.1, timeout / 2.0)
        start_time = time.time()

        while True:
            result = await _get_rect(pid, selector, space)
            if result is not None:
                return result

            elapsed = time.time() - start_time
            if elapsed >= timeout:
                return None

            await asyncio.sleep(retry_interval)

    async def execute_javascript(self, pid: int | str, javascript: str) -> Any:
        """Execute JavaScript in a pywebview window using bench_ui.

        Args:
            pid: Process ID of the pywebview window
            javascript: JavaScript code to execute

        Returns:
            Result of the JavaScript execution
        """
        await self._ensure_computer()

        @self._computer.python_command(use_system_python=True)
        def _exec_js(pid, javascript):
            from bench_ui import execute_javascript

            return execute_javascript(pid, javascript)

        return await _exec_js(pid, javascript)

    async def execute_action(self, action: Action) -> None:
        """Execute an action on the remote desktop using the SDK."""
        await self._ensure_computer()
        iface = self.interface

        if isinstance(action, ClickAction):
            await iface.left_click(action.x, action.y)

        elif isinstance(action, RightClickAction):
            await iface.right_click(action.x, action.y)

        elif isinstance(action, DoubleClickAction):
            await iface.double_click(action.x, action.y)

        elif isinstance(action, MiddleClickAction):
            await iface.move_cursor(action.x, action.y)
            # SDK may not have middle_click - fall back to move + custom
            try:
                await iface.middle_click(action.x, action.y)
            except (AttributeError, NotImplementedError):
                await iface.move_cursor(action.x, action.y)

        elif isinstance(action, DragAction):
            await iface.move_cursor(action.from_x, action.from_y)
            await iface.drag_to(action.to_x, action.to_y)

        elif isinstance(action, MoveToAction):
            await iface.move_cursor(action.x, action.y)

        elif isinstance(action, ScrollAction):
            clicks = action.amount // 100
            if clicks == 0:
                clicks = 1
            if action.direction == "down":
                clicks = -clicks
            await iface.scroll(self._width // 2, self._height // 2, clicks)

        elif isinstance(action, TypeAction):
            await iface.type_text(action.text)

        elif isinstance(action, KeyAction):
            await iface.press_key(action.key)

        elif isinstance(action, HotkeyAction):
            await iface.hotkey(*action.keys)

        elif isinstance(action, WaitAction):
            await asyncio.sleep(action.seconds)

        elif isinstance(action, DoneAction):
            return

        else:
            raise NotImplementedError(f"Action type not supported: {type(action).__name__}")

    async def screenshot(self) -> bytes:
        """Capture screenshot from remote environment.

        Returns:
            PNG image bytes
        """
        await self._ensure_computer()
        return await self.interface.screenshot()

    async def get_snapshot(self) -> Snapshot:
        """Get snapshot of desktop state with active window info.

        Uses pywinctl on remote to get active window, and if it's a webview
        we launched, extracts HTML via snapshot.js.
        """
        await self._ensure_computer()

        # Get active window info via pywinctl on remote
        @self._computer.python_command(use_system_python=True)
        def _pywinctl_active_window():
            import pywinctl as pwc

            win = pwc.getActiveWindow()
            if not win:
                return None
            x, y = win.position
            w, h = win.size
            title = str(getattr(win, "title", "") or "")
            return {
                "pid": win.getPID(),
                "wid": win.getHandle(),
                "title": title,
                "x": x,
                "y": y,
                "width": w,
                "height": h,
                "active": True,
                "minimized": False,
            }

        info = await _pywinctl_active_window()
        if info is None:
            return Snapshot(windows=[])

        pid_val = info.get("pid")
        title = info.get("title", "")
        x = int(info.get("x") or 0)
        y = int(info.get("y") or 0)
        width = int(info.get("width") or 0)
        height = int(info.get("height") or 0)

        # If it's a webview we launched, extract HTML via snapshot.js
        win_type = "process"
        win_html: Optional[str] = None
        if isinstance(pid_val, int) and pid_val in self._webview_pids:
            win_type = "webview"
            # Load snapshot.js from www/js directory
            snapshot_js_path = Path(__file__).resolve().parents[1] / "www" / "js" / "snapshot.js"
            if snapshot_js_path.exists():
                snapshot_js_code = snapshot_js_path.read_text(encoding="utf-8")
                js = (
                    snapshot_js_code
                    + "\n;(() => { try { return window.__td_build_snapshot(); } catch(e) { return ''; } })();"
                )
                win_html = await self.execute_javascript(pid_val, js)

        win = WindowSnapshot(
            window_type=win_type,
            pid=str(pid_val) if pid_val is not None else None,
            url=None,
            html=win_html,
            title=str(title or ""),
            x=x,
            y=y,
            width=width,
            height=height,
            active=True,
            minimized=False,
        )
        return Snapshot(windows=[win])

    async def close(self) -> None:
        """Close the session and cleanup resources."""
        if self._computer is not None:
            try:
                await self._computer.stop()
            except Exception:
                pass
            self._computer = None
            self._initialized = False

    async def close_all_windows(self) -> None:
        """Close all windows - best effort."""
        pass

    @property
    def page(self) -> Any:
        """Return underlying page object - not applicable for remote."""
        return None

    @property
    def vnc_url(self) -> str:
        """Return the VNC URL for accessing the environment.

        In full lifecycle mode, this may be updated after the container starts.
        """
        if self._vnc_url:
            return self._vnc_url
        # Generate URL from port
        return f"http://localhost:{self._vnc_port}/?autoconnect=true"

    async def click_element(self, pid: int | str, selector: str) -> None:
        """Find element by CSS selector and click its center.

        Uses get_element_rect to fetch element rect in screen space
        and then dispatches a ClickAction.
        """
        rect = await self.get_element_rect(pid, selector, space="screen")
        if not rect:
            raise RuntimeError(f"Element not found for selector: {selector}")
        cx = int(rect["x"] + rect["width"] / 2)
        cy = int(rect["y"] + rect["height"] / 2)
        await self.execute_action(ClickAction(x=cx, y=cy))

    async def right_click_element(self, pid: int | str, selector: str) -> None:
        """Find element by CSS selector and right-click its center."""
        rect = await self.get_element_rect(pid, selector, space="screen")
        if not rect:
            raise RuntimeError(f"Element not found for selector: {selector}")
        cx = int(rect["x"] + rect["width"] / 2)
        cy = int(rect["y"] + rect["height"] / 2)
        await self.execute_action(RightClickAction(x=cx, y=cy))

    # =========================================================================
    # Additional methods using SDK
    # =========================================================================

    async def get_accessibility_tree(self) -> Dict[str, Any]:
        """Get the accessibility tree if supported."""
        await self._ensure_computer()
        try:
            return await self.interface.get_accessibility_tree()
        except (AttributeError, NotImplementedError):
            return {}

    async def shell_command(self, command: str) -> Dict[str, Any]:
        """Execute a shell command.

        Args:
            command: Shell command to execute

        Returns:
            Command result with stdout/stderr
        """
        await self._ensure_computer()
        result = await self.interface.run_command(command)
        # Convert CommandResult to dict
        return {
            "success": result.return_code == 0 if hasattr(result, "return_code") else True,
            "stdout": result.stdout if hasattr(result, "stdout") else str(result),
            "stderr": result.stderr if hasattr(result, "stderr") else "",
            "return_code": result.return_code if hasattr(result, "return_code") else 0,
        }

    async def read_file(self, path: str) -> str:
        """Read a text file from the environment."""
        await self._ensure_computer()
        return await self.interface.read_text(path)

    async def write_file(self, path: str, content: str) -> None:
        """Write a text file to the environment."""
        await self._ensure_computer()
        await self.interface.write_text(path, content)

    async def read_bytes(self, path: str) -> bytes:
        """Read a file as bytes from the environment."""
        await self._ensure_computer()
        return await self.interface.read_bytes(path)

    async def write_bytes(self, path: str, data: bytes) -> None:
        """Write bytes to a file in the environment."""
        await self._ensure_computer()
        await self.interface.write_bytes(path, data)

    async def file_exists(self, path: str) -> bool:
        """Check if a file exists in the environment."""
        await self._ensure_computer()
        return await self.interface.file_exists(path)

    async def directory_exists(self, path: str) -> bool:
        """Check if a directory exists in the environment."""
        await self._ensure_computer()
        return await self.interface.directory_exists(path)

    async def list_dir(self, path: str) -> list[str]:
        """List contents of a directory in the environment."""
        await self._ensure_computer()
        return await self.interface.list_dir(path)

    async def run_command(self, command: str) -> Dict[str, Any]:
        """Execute a shell command (alias for shell_command)."""
        return await self.shell_command(command)

    async def launch_application(self, app_name: str) -> None:
        """Launch an application by name."""
        await self._ensure_computer()
        await self.interface.launch(app_name)

    async def check_status(self) -> bool:
        """Check if the environment is responsive.

        Returns:
            True if environment is ready, False otherwise
        """
        try:
            await self._ensure_computer()
            # Try a simple operation to verify connectivity
            await self.interface.get_screen_size()
            return True
        except Exception:
            return False

    async def wait_until_ready(self, timeout: int = 60, poll_interval: float = 2.0) -> bool:
        """Wait until the environment is ready.

        Args:
            timeout: Maximum time to wait in seconds
            poll_interval: Time between status checks

        Returns:
            True if environment became ready, False if timeout
        """
        import time

        start = time.time()
        while time.time() - start < timeout:
            if await self.check_status():
                return True
            await asyncio.sleep(poll_interval)
        return False

    # =========================================================================
    # Convenience action methods
    # =========================================================================

    async def click(self, x: int, y: int) -> None:
        """Click at coordinates."""
        await self.execute_action(ClickAction(x=x, y=y))

    async def right_click(self, x: int, y: int) -> None:
        """Right-click at coordinates."""
        await self.execute_action(RightClickAction(x=x, y=y))

    async def double_click(self, x: int, y: int) -> None:
        """Double-click at coordinates."""
        await self.execute_action(DoubleClickAction(x=x, y=y))

    async def type(self, text: str) -> None:
        """Type text."""
        await self.execute_action(TypeAction(text=text))

    async def key(self, key: str) -> None:
        """Press a key."""
        await self.execute_action(KeyAction(key=key))

    async def hotkey(self, keys: list[str]) -> None:
        """Press a key combination."""
        await self.execute_action(HotkeyAction(keys=keys))

    async def scroll(self, direction: str = "down", amount: int = 300) -> None:
        """Scroll the screen."""
        await self.execute_action(
            ScrollAction(
                x=self._width // 2, y=self._height // 2, direction=direction, amount=amount
            )
        )

    async def move_to(self, x: int, y: int) -> None:
        """Move cursor to coordinates."""
        await self.execute_action(MoveToAction(x=x, y=y))

    async def drag(self, from_x: int, from_y: int, to_x: int, to_y: int) -> None:
        """Drag from one position to another."""
        await self.execute_action(DragAction(from_x=from_x, from_y=from_y, to_x=to_x, to_y=to_y))


def create_remote_session(
    api_url: str,
    vnc_url: str = "",
    os_type: str = "linux",
    width: int = 1920,
    height: int = 1080,
) -> RemoteDesktopSession:
    """Create a RemoteDesktopSession.

    Args:
        api_url: URL of the environment's API endpoint
        vnc_url: URL for VNC access
        os_type: Operating system type
        width: Screen width
        height: Screen height

    Returns:
        Configured RemoteDesktopSession instance
    """
    return RemoteDesktopSession(
        api_url=api_url,
        vnc_url=vnc_url,
        os_type=os_type,
        width=width,
        height=height,
    )
