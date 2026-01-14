from __future__ import annotations

from typing import TYPE_CHECKING, Any, List, Literal, Optional, Protocol, TypedDict

from ..types import Snapshot

if TYPE_CHECKING:
    from ..apps.registry import AppsProxy

_DEFAULT_SESSION_NAME = "simulated"


def get_session(name: Optional[str] = None) -> type[DesktopSession]:
    """Return session class by name.

    Provider names:
        - "simulated" (alias: "webtop"): Playwright-based browser simulation
          Fast, no Docker required. UI is HTML/CSS rendering of desktop.
          Good for web-app testing, UI benchmarks.

        - "native" (alias: "computer"): Real OS in Docker/QEMU container
          Actual desktop environment with real applications.
          Requires Docker. Good for real app testing, OS-level tasks.
    """
    sess = (name or _DEFAULT_SESSION_NAME).lower()

    # Simulated desktop (Playwright-based)
    if sess in ("simulated", "webtop"):
        from .webtop import WebDesktopSession

        return WebDesktopSession

    # Native desktop (Docker/QEMU-based) - uses RemoteDesktopSession with cua-computer SDK
    if sess in ("native", "computer"):
        from .remote import RemoteDesktopSession

        return RemoteDesktopSession

    raise ValueError(
        f"Unknown session provider: {name}. "
        f"Available: 'simulated' (Playwright), 'native' (Docker/QEMU)"
    )


class DesktopSetupConfig(TypedDict, total=False):
    """Configuration for desktop setup provided to providers.

    Fields mirror high-level desktop appearance and workspace options.
    """

    os_type: Literal[
        "win11",
        "win10",
        "win7",
        "winxp",
        "win98",
        "macos",
        "linux",
        "android",
        "ios",
        "windows",  # Generic Windows (maps to win11)
    ]
    width: int
    height: int
    background: str
    wallpaper: str
    installed_apps: List[str]
    # Docker/VM configuration
    image: str  # Docker image to use (e.g., "trycua/winarena:latest", "trycua/cua-xfce:latest")
    storage: str  # Path to image storage for QEMU-based images (e.g., "~/.local/share/cua-bench/images/windows-qemu")
    memory: str  # VM memory allocation (e.g., "8GB")
    cpu: str  # VM CPU allocation (e.g., "4")
    provider_type: str  # Provider type ("docker", "lume", "cloud")


class DesktopSession(Protocol):
    """Desktop session interface for environment backends.

    Usage:
        # Preferred: async context manager
        async with get_session("native")(os_type="linux") as session:
            await session.screenshot()

        # Alternative: manual lifecycle
        session = get_session("native")(os_type="linux")
        await session.start()
        try:
            await session.screenshot()
        finally:
            await session.close()
    """

    def __init__(self, env: Any): ...

    # =========================================================================
    # Async Context Manager & Lifecycle
    # =========================================================================

    async def __aenter__(self) -> "DesktopSession":
        """Async context manager entry - initialize and start the session."""
        ...

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> None:
        """Async context manager exit - cleanup resources."""
        ...

    async def start(
        self,
        config: Optional[DesktopSetupConfig] = None,
        headless: Optional[bool] = None,
    ) -> None:
        """Start the session and connect to the environment.

        Args:
            config: Optional configuration to apply before starting.
            headless: If False, shows browser/VNC preview. Defaults to True.
        """
        ...

    async def serve_static(self, url_path: str, local_path: str) -> None: ...

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
        """Launch a window and return its process ID."""
        ...

    async def get_element_rect(
        self,
        pid: int | str,
        selector: str,
        *,
        space: Literal["window", "screen"] = "window",
        timeout: float = 0.5,
    ) -> dict[str, Any] | None: ...

    async def execute_javascript(self, pid: int | str, javascript: str) -> Any: ...

    async def execute_action(self, action: Any) -> None: ...

    async def screenshot(self) -> bytes: ...

    async def get_snapshot(self) -> Snapshot:
        """Return a lightweight snapshot of the desktop state (windows, etc.).

        Implementations should populate the list of open windows with geometry
        and metadata. If not supported, raise NotImplementedError.
        """
        ...

    async def close(self) -> None: ...

    async def close_all_windows(self) -> None:
        """Close or clear all open windows in the desktop environment."""
        ...

    @property
    def page(self) -> Any: ...

    @property
    def vnc_url(self) -> str:
        """Return the VNC URL for accessing the desktop environment."""
        ...

    @property
    def apps(self) -> "AppsProxy":
        """Access registered apps via session.apps.{app_name}.

        Provides a clean API for working with native applications:
            await session.apps.chrome.install()
            await session.apps.chrome.launch(url="https://example.com")
            url = await session.apps.chrome.get_current_url()

        Returns:
            AppsProxy that provides access to bound app instances
        """
        ...

    # --- Playwright-like Automation API ---

    async def click_element(self, pid: int | str, selector: str) -> None:
        """Find element by CSS selector and click its center.

        Uses the session's get_element_rect to fetch element rect in screen space
        and then dispatches a ClickAction.

        Args:
            pid: Process ID of the window
            selector: CSS selector for the element
        """
        ...

    async def right_click_element(self, pid: int | str, selector: str) -> None:
        """Find element by CSS selector and right-click its center.

        Args:
            pid: Process ID of the window
            selector: CSS selector for the element
        """
        ...

    # --- Native Provider Commands ---

    async def run_command(
        self,
        command: str,
        *,
        timeout: Optional[float] = None,
        check: bool = True,
    ) -> "CommandResult":
        """Execute a shell command on the native desktop environment.

        This method is only available with the native provider (Docker/QEMU).
        It will raise NotImplementedError on simulated sessions.

        Args:
            command: Shell command to execute
            timeout: Optional timeout in seconds
            check: If True (default), raise an exception if the command fails
                   (non-zero return code). If False, return the result regardless.

        Returns:
            CommandResult with stdout, stderr, and return_code

        Raises:
            NotImplementedError: If called on simulated provider
            RuntimeError: If check=True and command returns non-zero exit code

        Example:
            result = await session.run_command("ls -la /home/user")
            print(result.stdout)
        """
        ...

    # --- App Management ---

    async def install_app(
        self,
        app_name: str,
        *,
        with_shortcut: bool = True,
        **kwargs,
    ) -> None:
        """Install a registered app on the native desktop environment.

        Uses the app registry to find platform-specific install functions.
        This method is only available with the native provider (Docker/QEMU).

        Args:
            app_name: Name of the app to install (e.g., "godot", "firefox")
            with_shortcut: Create desktop shortcut (default True)
            **kwargs: App-specific arguments (e.g., version="4.2.1")

        Raises:
            ValueError: If app is not registered
            NotImplementedError: If app doesn't support the current platform

        Example:
            await session.install_app("godot", version="4.2.1")
            await session.install_app("firefox", with_shortcut=True)
        """
        ...

    async def launch_app(
        self,
        app_name: str,
        **kwargs,
    ) -> None:
        """Launch a registered app on the native desktop environment.

        Uses the app registry to find platform-specific launch functions.
        This method is only available with the native provider (Docker/QEMU).

        Args:
            app_name: Name of the app to launch
            **kwargs: App-specific arguments (e.g., project_path="/path")

        Raises:
            ValueError: If app is not registered
            NotImplementedError: If app doesn't support the current platform

        Example:
            await session.launch_app("godot", project_path="~/project", editor=True)
        """
        ...


class CommandResult(TypedDict):
    """Result from run_command execution."""

    stdout: str
    stderr: str
    return_code: int
