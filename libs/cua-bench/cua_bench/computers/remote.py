"""Remote desktop session for cua-bench.

This module provides a DesktopSession implementation on top of the cua-sandbox
SDK (``cua_sandbox.Sandbox``), driving any golden environment (linux-docker,
windows-qemu, linux-qemu, android-qemu) through its public interfaces
(``screen``, ``mouse``, ``keyboard``, ``shell``, ``files``).

This is the unified desktop session implementation that supports:
- Native environments (Docker containers, QEMU VMs) provisioned by cua-sandbox
- Remote connections to pre-existing sandboxes by URL
- Full bench_ui integration (pywebview windows, JS execution, element queries)
  by running small Python snippets in the guest through ``sb.shell``
"""

from __future__ import annotations

import asyncio
import base64
import inspect
import json
import logging
import textwrap
import time
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict, Literal, Optional
from urllib.parse import urlparse

if TYPE_CHECKING:
    from ..apps.registry import AppsProxy

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

logger = logging.getLogger(__name__)

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


_RESULT_MARKER = "__CUA_BENCH_RESULT__:"

#: Makes ``window.screenX``/``screenY`` (and ``screenLeft``/``screenTop``) the
#: screen position of the page's client area. Real window managers report the
#: frame's origin there (title bar included), while cua-bench tasks, written
#: for the retired simulated provider, map page to screen coordinates as
#: ``rect.left + window.screenX``. The offset (dx, dy) is the frame's
#: decoration; the getters follow the window if it moves.
_CLIENT_ORIGIN_JS = """(function(dx, dy){
  function native(name){
    var d = Object.getOwnPropertyDescriptor(window, name)
      || Object.getOwnPropertyDescriptor(Window.prototype, name);
    if (d && d.get) { return d.get.bind(window); }
    var v = window[name];
    return function(){ return v; };
  }
  var sx = native('screenX'), sy = native('screenY');
  function gx(){ return sx() + dx; }
  function gy(){ return sy() + dy; }
  ['screenX', 'screenLeft'].forEach(function(n){
    Object.defineProperty(window, n, {get: gx, configurable: true});
  });
  ['screenY', 'screenTop'].forEach(function(n){
    Object.defineProperty(window, n, {get: gy, configurable: true});
  });
  return [window.screenX, window.screenY];
})(%d, %d)"""

#: Before ``click_element``: bring the element into view the way a user
#: scrolls to it (a click lands on the screen, so an element below the fold
#: would miss). An ``<option>`` lives in a native popup outside the page, so
#: it is picked the way a user's click ends up (select it, fire input and
#: change). Returns "option", "visible" or "missing".
_PREPARE_CLICK_JS = """(function(sel){
  var el = document.querySelector(sel);
  if (!el) { return 'missing'; }
  var select = el.tagName === 'OPTION' ? el.closest('select') : null;
  if (select) {
    select.value = el.value;
    el.selected = true;
    select.dispatchEvent(new Event('input', {bubbles: true}));
    select.dispatchEvent(new Event('change', {bubbles: true}));
    return 'option';
  }
  if (el.scrollIntoView) { el.scrollIntoView({block: 'nearest', inline: 'nearest'}); }
  return 'visible';
})(%s)"""


def _parse_int(value: Any) -> Optional[int]:
    try:
        return int(str(value).strip()) if value not in (None, "") else None
    except ValueError:
        return None


def _parse_memory_mb(value: Any) -> Optional[int]:
    """Parse '8GB' / '512MB' / '8192' into MiB."""
    if value in (None, ""):
        return None
    text = str(value).strip().upper()
    try:
        if text.endswith("GB") or text.endswith("G"):
            return int(float(text.rstrip("GB")) * 1024)
        if text.endswith("MB") or text.endswith("M"):
            return int(float(text.rstrip("MB")))
        return int(text)
    except ValueError:
        return None


def _warn_deprecated(message: str) -> None:
    import warnings

    warnings.warn(message, DeprecationWarning, stacklevel=4)


_TRUE = ("1", "true", "yes", "on")

#: Where a session with ``api_url`` connects: cua-spacesd (the canonical
#: images, port 3211) or a legacy computer-server (``POST /cmd``, the images
#: cua-bench 0.2 and harnesses like Agents' Last Exam run).
TRANSPORTS = ("auto", "spacesd", "computer-server")
SPACESD_PORT = 3211


def _env_flag(name: str) -> Optional[bool]:
    import os

    value = os.environ.get(name, "").strip().lower()
    if not value:
        return None
    return value in _TRUE


def _env_float(name: str) -> Optional[float]:
    import os

    value = os.environ.get(name, "").strip()
    if not value:
        return None
    try:
        return float(value)
    except ValueError:
        return None


#: Sandbox refs (``local:<name>``, ``cloud:<name>``, ``direct:<host:port>``,
#: ``relay:<id>``) and the legacy ``fleet:`` spelling: always cua-spacesd.
_REF_PREFIXES = ("local:", "cloud:", "direct:", "relay:", "fleet:", "space://")


def is_sandbox_ref(value: str) -> bool:
    return (
        bool(value)
        and "://" not in value.replace("space://", "", 1)
        and (value.startswith(_REF_PREFIXES))
    )


def resolve_transport(api_url: str, transport: Optional[str] = None) -> str:
    """``spacesd`` or ``computer-server`` for a client-only session.

    ``auto`` (the default, or ``CUA_BENCH_TRANSPORT``) picks cua-spacesd for a
    sandbox ref (``local:box``, ``cloud:box``, ``direct:host:3211``), its
    port 3211 and a ``cua+``/``grpc`` scheme, and computer-server otherwise:
    0.2.x sessions by URL (ports 5000/8000) keep working unchanged.
    """
    import os

    if is_sandbox_ref(api_url):
        return "spacesd"

    choice = (transport or os.environ.get("CUA_BENCH_TRANSPORT") or "auto").strip().lower()
    choice = {"legacy": "computer-server", "computer_server": "computer-server"}.get(choice, choice)
    if choice not in TRANSPORTS:
        raise ValueError(f"transport must be one of {', '.join(TRANSPORTS)} (got {transport!r})")
    if choice != "auto":
        return choice
    parsed = urlparse(api_url)
    if parsed.port == SPACESD_PORT or parsed.scheme.startswith(("cua", "grpc")):
        return "spacesd"
    return "computer-server"


class CommandOutput(dict):
    """``run_command``'s result: the 0.2.x dict, also readable as attributes.

    Keys: ``success``, ``stdout``, ``stderr``, ``return_code`` (0.2.x
    semantics unless strict) and ``exit_code`` (always the real code).
    ``.returncode`` is the real exit code, like a cua-sandbox CommandResult.
    """

    def __getattr__(self, name: str) -> Any:
        if name == "returncode":
            return self["exit_code"]
        try:
            return self[name]
        except KeyError:
            raise AttributeError(name) from None


class _SandboxComputer:
    """``session.computer`` for a sandbox session: ``.interface`` is the
    cua-computer surface, everything else is the ``cua_sandbox.Sandbox``."""

    def __init__(self, sandbox: Any, interface: Any) -> None:
        self._sb = sandbox
        self.interface = interface
        self._interface = interface
        self._original_interface = interface
        self._initialized = True

    def __getattr__(self, name: str) -> Any:
        if name.startswith("__") or name == "_sb":
            raise AttributeError(name)
        return getattr(self._sb, name)


class RemoteDesktopSession:
    """Unified desktop session using the cua-sandbox SDK.

    Supports three modes:
    1. **Full lifecycle mode** (default): cua-sandbox manages the container/VM
       - Pass config via constructor kwargs or start(config={...})
       - cua-sandbox starts the container, waits for boot, connects

    2. **Client-only mode**: Connect to a pre-existing machine by ``api_url``:
       cua-spacesd (port 3211) or a legacy computer-server (``transport=``).

    3. **Attached** (:meth:`attach`): wrap a sandbox the caller owns (``cb run``).

    Compatibility with cua-bench 0.2.x (Agents' Last Exam relies on it):
    ``run_command``/``shell_command`` report ``return_code`` 0 and never raise
    for ``check=True`` unless ``strict_exit_codes=True`` (or
    ``CUA_BENCH_STRICT_EXIT_CODES=1``); the real code is always in
    ``exit_code``. Shell commands have no timeout unless ``timeout=`` (or
    ``CUA_BENCH_SHELL_TIMEOUT``) sets one. ``session.computer`` /
    ``session.interface`` expose the cua-computer 0.5 surface, and the private
    ``_os_type``, ``_api_host``, ``_api_port``, ``_vnc_port``, ``_computer``
    and ``_initialized`` attributes keep their 0.2.x meaning.

    Full lifecycle mode opens the sandbox exactly like ``cb run``: the
    canonical ``ghcr.io/trycua/<os>`` image unless ``image=`` names a registry
    ref, locally unless ``provider_type="cloud"`` (Fleet).

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
        *,
        transport: Optional[str] = None,
        strict_exit_codes: Optional[bool] = None,
        timeout: Optional[float] = None,
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
            image: Registry image (default: the canonical image for os_type)
            provider_type: "cloud" runs on Fleet; anything else runs locally
            memory: VM memory allocation (e.g., "8GB")
            cpu: VM CPU allocation (e.g., "4")
            name: Container/VM name (auto-generated if empty)
            storage: Deprecated and ignored
            ephemeral: Whether to use ephemeral storage (default True)
            headless: If False, opens VNC preview in browser on start
            transport: "auto" (default), "spacesd" or "computer-server" for api_url
            strict_exit_codes: report real exit codes and raise for check=True
                (default False, the 0.2.x behaviour; env CUA_BENCH_STRICT_EXIT_CODES)
            timeout: shell command timeout in seconds (default none, the 0.2.x
                behaviour; env CUA_BENCH_SHELL_TIMEOUT)
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
        self._transport = transport
        if strict_exit_codes is None:
            strict_exit_codes = bool(_env_flag("CUA_BENCH_STRICT_EXIT_CODES"))
        self._strict_exit_codes = bool(strict_exit_codes)
        self._shell_timeout = (
            timeout if timeout is not None else _env_float("CUA_BENCH_SHELL_TIMEOUT")
        )

        # Parse API URL to extract host and port (0.2.x attributes; harnesses
        # read them to build their own cua-computer client).
        if api_url and is_sandbox_ref(api_url):
            host, _, port = api_url.split(":", 1)[1].rpartition(":")
            self._api_host = (host or api_url).strip("[]") or "localhost"
            self._api_port = int(port) if port.isdigit() else SPACESD_PORT
        elif api_url:
            parsed = urlparse(self._api_url)
            self._api_host = parsed.hostname or "localhost"
            self._api_port = parsed.port or 5000
        else:
            self._api_host = "localhost"
            self._api_port = 8000

        # Parse VNC URL for port
        vnc_parsed = urlparse(vnc_url) if vnc_url else None
        self._vnc_port = vnc_parsed.port if vnc_parsed else 8006

        # cua_sandbox.Sandbox instance (lazy initialized)
        self._sandbox: Any = None
        # A cua-computer style Computer (``.interface``): the computer-server
        # transport, or one a harness assigns (0.2.x ``session._computer``).
        self._computer: Any = None
        self._sandbox_interface: Any = None
        self._initialized = False

        # Track PIDs of windows launched via bench_ui (pywebview)
        self._webview_pids: set[int] = set()

        # True when wrapping a sandbox whose lifecycle belongs to the caller.
        self._attached = False
        # The open_sandbox() context of a sandbox this session created itself.
        self._lifecycle: Any = None

    @classmethod
    def attach(
        cls,
        sandbox: Any,
        *,
        os_type: str = "linux",
        width: Optional[int] = None,
        height: Optional[int] = None,
        strict_exit_codes: Optional[bool] = None,
        timeout: Optional[float] = None,
    ) -> "RemoteDesktopSession":
        """Wrap an already-connected ``cua_sandbox.Sandbox``.

        The caller owns the sandbox: :meth:`close` only drops the reference
        (the runner releases the sandbox or its Fleet claim itself).
        """
        session = cls(
            os_type=os_type,
            width=width or 1920,
            height=height or 1080,
            strict_exit_codes=strict_exit_codes,
            timeout=timeout,
        )
        session._sandbox = sandbox
        session._initialized = True
        session._client_only_mode = True
        session._ephemeral = False
        session._attached = True
        return session

    @property
    def computer(self):
        """The cua-computer style ``Computer`` of this session (0.2.x).

        Its ``.interface`` is the cua-computer interface surface; on a sandbox
        session every other attribute is the ``cua_sandbox.Sandbox``'s.
        """
        if self._computer is not None:
            return self._computer
        if self._sandbox is None:
            raise RuntimeError("Session not initialized. Call start() first.")
        return _SandboxComputer(self._sandbox, self.interface)

    @property
    def sandbox(self) -> Any:
        """The connected ``cua_sandbox.Sandbox`` (None for computer-server sessions)."""
        return self._sandbox

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
        """Ensure the cua_sandbox.Sandbox is provisioned (if needed) and connected."""
        if self._initialized and (self._sandbox is not None or self._computer is not None):
            return

        if self._client_only_mode:
            if resolve_transport(self._api_url, self._transport) == "computer-server":
                # A legacy computer-server (0.2.x semantics, cua-computer client).
                from ..compat.legacy_interface import Computer

                self._computer = Computer(
                    os_type=self._os_type,
                    use_host_computer_server=True,
                    api_host=self._api_host,
                    api_port=self._api_port,
                    noVNC_port=self._vnc_port,
                    api_base_url=self._api_url,
                )
                await self._computer.run()
            else:
                from cua_sandbox import Sandbox

                # cua-spacesd: a sandbox ref (local:/cloud:/direct:) or a URL.
                if is_sandbox_ref(self._api_url):
                    self._sandbox = await Sandbox.connect(self._api_url)
                else:
                    self._sandbox = await Sandbox.connect(url=self._api_url)
        else:
            # Full lifecycle mode: the same path as `cb run` (targets + sandboxes):
            # the canonical image for the OS unless one is given, a local
            # sandbox unless provider_type="cloud".
            self._sandbox = await self._open_owned_sandbox()
            try:
                self._vnc_url = await self._sandbox.get_display_url()
            except Exception:
                pass

        self._initialized = True

    def _lifecycle_target(self):
        """The Target and EnvSpec this session's constructor arguments ask for."""
        from ..targets import resolve_env_spec, resolve_target

        if self._provider_type not in ("docker", "cloud", "local", "native", ""):
            _warn_deprecated(
                f"RemoteDesktopSession(provider_type={self._provider_type!r}) is deprecated: "
                "the sandbox runtime follows the image (use provider_type='cloud' for Fleet)"
            )
        if self._storage:
            _warn_deprecated(
                "RemoteDesktopSession(storage=...) is deprecated and ignored: images come "
                "from registries (pass image=<registry ref>)"
            )
        target = resolve_target(
            "cloud" if self._provider_type == "cloud" else None,
            cpu=_parse_int(self._cpu),
            memory=_parse_memory_mb(self._memory),
        )
        setup = {"os_type": self._os_type, "width": self._width, "height": self._height}
        if self._image:
            setup["image"] = self._image
        spec = resolve_env_spec({"provider": "native", "setup_config": setup}, target)
        return target, spec

    async def _open_owned_sandbox(self) -> Any:
        from contextlib import AsyncExitStack

        from ..sandboxes import open_sandbox

        target, spec = self._lifecycle_target()
        stack = AsyncExitStack()
        try:
            sandbox = await stack.enter_async_context(open_sandbox(spec, target))
        except BaseException:
            await stack.aclose()
            raise
        self._lifecycle = stack
        return sandbox

    @property
    def interface(self):
        """The cua-computer 0.5 interface surface of this session.

        ``create_dir``, ``run_command -> CommandResult``, ``write_text(append=)``
        and friends, over the computer-server or the sandbox. On a sandbox
        session other attributes are the ``cua_sandbox.Sandbox``'s
        (``interface.files``, ``interface.shell`` ...).
        """
        if self._computer is not None:
            return self._computer.interface
        if self._sandbox is None:
            raise RuntimeError("Session not initialized. Call _ensure_computer() first.")
        if self._sandbox_interface is None or self._sandbox_interface.sandbox is not self._sandbox:
            from ..compat.legacy_interface import SandboxInterface

            self._sandbox_interface = SandboxInterface(self._sandbox, os_type=self._os_type)
        return self._sandbox_interface

    @property
    def _legacy(self) -> bool:
        """True when a cua-computer style Computer backs this session."""
        return self._computer is not None

    async def _is_dir(self, path: str) -> bool:
        try:
            return bool(await self.interface.directory_exists(path))
        except Exception:
            return False

    async def _run_raw(self, command: str, timeout: Optional[float]) -> Any:
        """``interface.run_command`` honouring ``timeout`` without breaking
        wrappers a harness installed on the interface (they take one arg)."""
        iface = self.interface
        if timeout is None:
            return await iface.run_command(command)
        run = iface.run_command
        own = getattr(type(iface), "run_command", None)
        if getattr(iface, "_cua_bench_interface", False) and getattr(run, "__func__", None) is own:
            return await run(command, timeout=timeout)
        return await asyncio.wait_for(run(command), timeout)

    def _python_command(self):
        """Decorator: run a self-contained function in the guest's system Python.

        The function source is shipped through ``sb.shell`` and its JSON-encodable
        return value is read back from stdout.
        """

        def decorator(func):
            source = textwrap.dedent(inspect.getsource(func))
            source = "\n".join(
                line for line in source.splitlines() if not line.lstrip().startswith("@")
            )

            async def runner(*args, **kwargs):
                payload = json.dumps({"a": list(args), "k": kwargs})
                script = (
                    "import json\n"
                    f"{source}\n"
                    f"_p = json.loads({payload!r})\n"
                    f"_r = {func.__name__}(*_p['a'], **_p['k'])\n"
                    f"print({_RESULT_MARKER!r} + json.dumps(_r))\n"
                )
                encoded = base64.b64encode(script.encode("utf-8")).decode("ascii")
                python = "python" if self._os_type in ("windows", "win11", "win10") else "python3"
                command = (
                    f'{python} -c "import base64;'
                    f"exec(base64.b64decode('{encoded}').decode('utf-8'))\""
                )
                result = await self._run_raw(command, self.DEFAULT_TIMEOUT)
                stdout = getattr(result, "stdout", "") or ""
                for line in reversed(stdout.splitlines()):
                    if line.startswith(_RESULT_MARKER):
                        return json.loads(line[len(_RESULT_MARKER) :])
                code = getattr(result, "returncode", getattr(result, "return_code", "?"))
                raise RuntimeError(
                    f"Remote python '{func.__name__}' failed (exit {code}): "
                    f"{(getattr(result, 'stderr', '') or stdout).strip()[:500]}"
                )

            return runner

        return decorator

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
            @self._python_command()
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
                    if not await self._is_dir(remote_parent):
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

            @self._python_command()
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
        @self._python_command()
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
        await self._pin_client_origin(pid)
        return pid

    async def _pin_client_origin(self, pid: int | str) -> None:
        """Point the window's ``screenX``/``screenY`` at its client area.

        Best effort: a window that does not report its client origin keeps
        the toolkit's values.
        """
        try:
            origin = await self.get_element_rect(pid, "html", space="screen", timeout=5.0)
            frame = await self.execute_javascript(pid, "[window.screenX, window.screenY]")
            if not origin or not isinstance(frame, (list, tuple)) or len(frame) != 2:
                return
            dx = int(round(float(origin["x"]) - float(frame[0])))
            dy = int(round(float(origin["y"]) - float(frame[1])))
            if (dx, dy) == (0, 0) or not (0 <= dx <= 200 and 0 <= dy <= 200):
                return
            await self.execute_javascript(pid, _CLIENT_ORIGIN_JS % (dx, dy))
        except Exception as error:  # noqa: BLE001 - never fails a task setup
            logger.debug("could not pin the client origin of window %s: %r", pid, error)

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

        @self._python_command()
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

        @self._python_command()
        def _exec_js(pid, javascript):
            from bench_ui import execute_javascript

            return execute_javascript(pid, javascript)

        return await _exec_js(pid, javascript)

    async def execute_action(self, action: Action) -> None:
        """Execute an action on the remote desktop using the SDK."""
        await self._ensure_computer()
        if self._legacy:
            await self._execute_legacy_action(action)
            return
        sb = self._sandbox

        if isinstance(action, ClickAction):
            await sb.mouse.click(action.x, action.y)

        elif isinstance(action, RightClickAction):
            await sb.mouse.right_click(action.x, action.y)

        elif isinstance(action, DoubleClickAction):
            await sb.mouse.double_click(action.x, action.y)

        elif isinstance(action, MiddleClickAction):
            await sb.mouse.click(action.x, action.y, button="middle")

        elif isinstance(action, DragAction):
            await sb.mouse.drag(action.from_x, action.from_y, action.to_x, action.to_y)

        elif isinstance(action, MoveToAction):
            await sb.mouse.move(action.x, action.y)

        elif isinstance(action, ScrollAction):
            clicks = max(1, abs(action.amount) // 100)
            if action.direction == "up":
                clicks = -clicks
            await sb.mouse.scroll(self._width // 2, self._height // 2, scroll_x=0, scroll_y=clicks)

        elif isinstance(action, TypeAction):
            await sb.keyboard.type(action.text)

        elif isinstance(action, KeyAction):
            await sb.keyboard.keypress(action.key)

        elif isinstance(action, HotkeyAction):
            await sb.keyboard.keypress(list(action.keys))

        elif isinstance(action, WaitAction):
            await asyncio.sleep(action.seconds)

        elif isinstance(action, DoneAction):
            return

        else:
            raise NotImplementedError(f"Action type not supported: {type(action).__name__}")

    async def _execute_legacy_action(self, action: Action) -> None:
        """Actions over the cua-computer interface (0.2.x mapping)."""
        iface = self.interface
        if isinstance(action, ClickAction):
            await iface.left_click(action.x, action.y)
        elif isinstance(action, RightClickAction):
            await iface.right_click(action.x, action.y)
        elif isinstance(action, DoubleClickAction):
            await iface.double_click(action.x, action.y)
        elif isinstance(action, MiddleClickAction):
            await iface.move_cursor(action.x, action.y)
            middle = getattr(iface, "middle_click", None)
            if middle is not None:
                await middle(action.x, action.y)
        elif isinstance(action, DragAction):
            await iface.move_cursor(action.from_x, action.from_y)
            await iface.drag_to(action.to_x, action.to_y)
        elif isinstance(action, MoveToAction):
            await iface.move_cursor(action.x, action.y)
        elif isinstance(action, ScrollAction):
            clicks = max(1, abs(action.amount) // 100)
            await iface.move_cursor(self._width // 2, self._height // 2)
            if action.direction == "up":
                await iface.scroll_up(clicks)
            else:
                await iface.scroll_down(clicks)
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
        @self._python_command()
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
        computer, self._computer = self._computer, None
        if computer is not None:
            # 0.2.x: Computer.stop() (a harness-provided Computer included).
            self._initialized = False
            try:
                await computer.stop()
            except Exception:
                pass
            if self._sandbox is None:
                return
        if self._sandbox is not None and self._attached:
            self._sandbox = None
            self._initialized = False
            return
        lifecycle, self._lifecycle = self._lifecycle, None
        if lifecycle is not None:
            # A sandbox this session opened: release it (local teardown or
            # the Fleet claim) through the same context that created it.
            self._sandbox = None
            self._initialized = False
            try:
                await lifecycle.aclose()
            except Exception:
                pass
            return
        if self._sandbox is not None:
            try:
                await self._sandbox.disconnect()
            except Exception:
                pass
            self._sandbox = None
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
        if not hasattr(self, "_apps_proxy"):
            from ..apps.registry import AppsProxy

            self._apps_proxy = AppsProxy(self)
        return self._apps_proxy

    async def click_element(self, pid: int | str, selector: str) -> None:
        """Find element by CSS selector and click its center.

        Uses get_element_rect to fetch element rect in screen space
        and then dispatches a ClickAction.
        """
        if await self._prepare_click(pid, selector) == "option":
            # An open native popup (the select's first click) would swallow
            # the next input: close it.
            await self.execute_action(KeyAction(key="Escape"))
            return
        rect = await self.get_element_rect(pid, selector, space="screen")
        if not rect:
            raise RuntimeError(f"Element not found for selector: {selector}")
        cx = int(rect["x"] + rect["width"] / 2)
        cy = int(rect["y"] + rect["height"] / 2)
        await self.execute_action(ClickAction(x=cx, y=cy))

    async def _prepare_click(self, pid: int | str, selector: str) -> str:
        """Scroll the element into view (or pick an ``<option>``); see _PREPARE_CLICK_JS."""
        try:
            return str(await self.execute_javascript(pid, _PREPARE_CLICK_JS % json.dumps(selector)))
        except Exception as error:  # noqa: BLE001 - the click still tries the rect
            logger.debug("could not prepare the click on %s: %r", selector, error)
            return "unknown"

    async def right_click_element(self, pid: int | str, selector: str) -> None:
        """Find element by CSS selector and right-click its center."""
        await self._prepare_click(pid, selector)
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
        """Get the accessibility tree if supported ({} when not)."""
        await self._ensure_computer()
        try:
            return await self.interface.get_accessibility_tree()
        except (AttributeError, NotImplementedError, RuntimeError):
            return {}

    async def shell_command(
        self,
        command: str,
        *,
        check: bool = True,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """Execute a shell command.

        Args:
            command: Shell command to execute
            check: With ``strict_exit_codes``, raise when the command exits
                non-zero. Without it (the default, cua-bench 0.2.x semantics)
                ``return_code`` is reported as 0 and ``check`` never raises.
            timeout: Seconds to wait (default: the session's ``timeout``, which
                is none unless set).

        Returns:
            ``{"success", "stdout", "stderr", "return_code", "exit_code"}``;
            ``exit_code`` is always the command's real exit code.

        Raises:
            RuntimeError: If strict, check=True and the command exits non-zero
        """
        await self._ensure_computer()
        wait = timeout if timeout is not None else self._shell_timeout
        result = await self._run_raw(command, wait)
        stdout = result.stdout if hasattr(result, "stdout") else str(result)
        stderr = result.stderr if hasattr(result, "stderr") else ""
        try:
            exit_code = int(getattr(result, "returncode", getattr(result, "return_code", 0)))
        except (TypeError, ValueError):
            exit_code = 0
        if self._strict_exit_codes:
            return_code = exit_code
        else:
            # cua-bench 0.2.x read a ``return_code`` attribute the cua-computer
            # result never had, so it reported 0; graders are calibrated on it.
            return_code = getattr(result, "return_code", 0)

        if check and return_code != 0:
            raise RuntimeError(
                f"Command failed with return code {return_code}.\n"
                f"Command: {command}\n"
                f"Stdout: {stdout}\n"
                f"Stderr: {stderr}"
            )

        return CommandOutput(
            success=return_code == 0,
            stdout=stdout,
            stderr=stderr,
            return_code=return_code,
            exit_code=exit_code,
        )

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
        """Whether ``path`` is an existing file (not a directory), as in 0.2.x."""
        await self._ensure_computer()
        return await self.interface.file_exists(path)

    async def directory_exists(self, path: str) -> bool:
        """Check if a directory exists in the environment."""
        await self._ensure_computer()
        return await self.interface.directory_exists(path)

    async def list_dir(self, path: str) -> list[str]:
        """List contents of a directory in the environment."""
        await self._ensure_computer()
        return list(await self.interface.list_dir(path))

    async def run_command(
        self,
        command: str,
        *,
        check: bool = True,
        timeout: Optional[float] = None,
    ) -> Dict[str, Any]:
        """Execute a shell command (alias for :meth:`shell_command`)."""
        return await self.shell_command(command, check=check, timeout=timeout)

    async def launch_application(self, app_name: str) -> None:
        """Launch an application by name."""
        await self._ensure_computer()
        if self._legacy:
            await self.interface.launch(app_name)
        else:
            await self._sandbox.shell.run(app_name, background=True)

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
        await self.execute_action(ScrollAction(direction=direction, amount=amount))

    async def move_to(self, x: int, y: int) -> None:
        """Move cursor to coordinates."""
        await self.execute_action(MoveToAction(x=x, y=y))

    async def drag(self, from_x: int, from_y: int, to_x: int, to_y: int) -> None:
        """Drag from one position to another."""
        await self.execute_action(DragAction(from_x=from_x, from_y=from_y, to_x=to_x, to_y=to_y))

    # =========================================================================
    # App Registry Integration
    # =========================================================================

    @property
    def os_type(self) -> str:
        """Return the OS type for this session."""
        return self._os_type

    async def install_app(
        self,
        app_name: str,
        *,
        with_shortcut: bool = True,
        **kwargs,
    ) -> None:
        """Install a registered app on the native desktop environment.

        Uses the app registry to find platform-specific install functions.

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
        await self._ensure_computer()
        from ..apps import AppRegistry

        await AppRegistry.install_app(self, app_name, with_shortcut=with_shortcut, **kwargs)

    async def launch_app(
        self,
        app_name: str,
        **kwargs,
    ) -> None:
        """Launch a registered app on the native desktop environment.

        Uses the app registry to find platform-specific launch functions.

        Args:
            app_name: Name of the app to launch
            **kwargs: App-specific arguments (e.g., project_path="/path")

        Raises:
            ValueError: If app is not registered
            NotImplementedError: If app doesn't support the current platform

        Example:
            await session.launch_app("godot", project_path="~/project", editor=True)
        """
        await self._ensure_computer()
        from ..apps import AppRegistry

        await AppRegistry.launch_app(self, app_name, **kwargs)


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
