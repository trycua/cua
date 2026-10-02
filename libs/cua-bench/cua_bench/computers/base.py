from __future__ import annotations

from typing import TYPE_CHECKING, Any, List, Literal, Optional, Protocol, TypedDict

from ..types import Snapshot

if TYPE_CHECKING:
    from ..apps.registry import AppsProxy

_DEFAULT_SESSION_NAME = "native"


def get_session(name: Optional[str] = None) -> type[DesktopSession]:
    """Return session class by name.

    ``native`` (alias ``computer``): a real OS in a sandbox (container or VM),
    driven through :class:`~cua_bench.computers.remote.RemoteDesktopSession`.
    The Playwright ``simulated``/``webtop`` provider was removed in cua-bench
    0.3; those names return the same session with a deprecation warning.
    """
    sess = (name or _DEFAULT_SESSION_NAME).lower()
    if sess in ("simulated", "webtop"):
        import warnings

        warnings.warn(
            "the simulated provider was removed in cua-bench 0.3; "
            f"get_session({name!r}) returns the native RemoteDesktopSession",
            DeprecationWarning,
            stacklevel=2,
        )
        sess = "native"

    if sess in ("native", "computer"):
        from .remote import RemoteDesktopSession

        return RemoteDesktopSession

    raise ValueError(f"Unknown session provider: {name}. Available: 'native' (alias 'computer')")


class DesktopSetupConfig(TypedDict, total=False):
    """A task's ``computer["setup_config"]``: the sandbox the task runs in.

    Every key is optional. Command-line flags (``--image``, ``--kind``,
    ``--cpu``, ``--memory``) override the task's values; ``--runtime`` (the
    engine) is a command-line choice only.
    """

    #: Guest OS: ``linux`` (the default; ``ubuntu`` is an alias), ``windows``
    #: (``win11``, ``win10`` and older names are aliases), ``macos`` or
    #: ``android``. macOS and Android run only with ``--on local``.
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
        "windows",
    ]
    #: Screen width in pixels.
    width: int
    #: Screen height in pixels.
    height: int
    #: Registry image or OS alias (``linux``, ``windows``, ``macos:tahoe``), or
    #: ``pool:<name>`` for an existing Fleet pool. ``--image`` wins, then this,
    #: then ``CUA_BENCH_IMAGE``, then the canonical image of ``os_type``.
    image: str
    #: Preferred kind: ``container`` (Linux only) or ``vm``.
    kind: str
    #: Kinds the task supports (a requirement, unlike ``kind``): for
    #: example ``["vm"]`` for a task that needs its own kernel.
    kinds: List[str]
    #: What the target must provide: ``kvm``, ``env:<NAME>`` (a set
    #: environment variable), ``openai`` (``env:OPENAI_API_KEY``) or
    #: ``hf-gated`` (``env:HF_TOKEN``).
    requires: List[str]
    #: A port the image serves itself, used as the readiness probe.
    server_port: int
    #: VM memory, for example ``"8GB"``.
    memory: str
    #: VM CPUs, for example ``"4"``.
    cpu: str
    #: Ignored (kept so older tasks still load).
    background: str
    #: Ignored (kept so older tasks still load).
    wallpaper: str
    #: Ignored (kept so older tasks still load).
    installed_apps: List[str]
    #: Deprecated and ignored.
    storage: str
    #: Deprecated: ``"cloud"`` runs on Fleet; use ``--on cloud`` instead.
    provider_type: str


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

    # --- Selector-based Automation API ---

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
