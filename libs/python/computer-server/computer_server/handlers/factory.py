import inspect
import logging
import os
from typing import Optional, Tuple

from computer_server.diorama.base import BaseDioramaHandler

from ..backend_policy import VNCUnavailableHandler, configured_backend
from ..utils.helpers import get_current_os
from .base import (
    BaseAccessibilityHandler,
    BaseAutomationHandler,
    BaseDesktopHandler,
    BaseFileHandler,
    BaseWindowHandler,
)

logger = logging.getLogger(__name__)

OS_TYPE = get_current_os()

from .generic import GenericDesktopHandler, GenericFileHandler, GenericWindowHandler

HandlerTuple = Tuple[
    BaseAccessibilityHandler,
    BaseAutomationHandler,
    BaseDioramaHandler,
    BaseFileHandler,
    BaseDesktopHandler,
    BaseWindowHandler,
]


class HandlerFactory:
    """Factory for creating OS-specific handlers."""

    _shared_handlers: Optional[HandlerTuple] = None

    @staticmethod
    def create_handlers() -> HandlerTuple:
        """Create and return appropriate handlers for the current OS.

        Returns:
            Tuple[BaseAccessibilityHandler, BaseAutomationHandler, BaseDioramaHandler, BaseFileHandler]: A tuple containing
            the appropriate accessibility, automation, diorama, and file handlers for the current OS.

        Raises:
            NotImplementedError: If the current OS is not supported
            RuntimeError: If unable to determine the current OS
        """
        backend = configured_backend()
        vnc_host = os.environ.get("CUA_VNC_HOST")
        if backend == "vnc":
            if not vnc_host:
                raise RuntimeError(
                    "CUA_VNC_HOST must be set when using VNC backend "
                    "(--backend=vnc requires --vnc-host)"
                )
            from .vnc import VNCAccessibilityHandler, VNCAutomationHandler

            vnc_port = int(os.environ.get("CUA_VNC_PORT", "5900"))
            vnc_password = os.environ.get("CUA_VNC_PASSWORD", "")
            unavailable = VNCUnavailableHandler()
            logger.info(f"Using VNC backend → {vnc_host}:{vnc_port}")
            return (
                VNCAccessibilityHandler(),
                VNCAutomationHandler(host=vnc_host, port=vnc_port, password=vnc_password),
                BaseDioramaHandler(),
                unavailable,
                unavailable,
                unavailable,
            )
        if backend not in {"native", "cua-driver"}:
            raise RuntimeError("CUA_BACKEND must be native, vnc, or cua-driver")
        if backend == "cua-driver" and OS_TYPE == "android":
            raise RuntimeError("CUA_BACKEND=cua-driver is not supported on Android")
        if backend == "cua-driver":
            from .cua_driver import (
                CuaDriverAccessibilityHandler,
                CuaDriverAutomationHandler,
            )

            automation = CuaDriverAutomationHandler()
            logger.info("Using Cua Driver automation backend (%s mode)", automation.mode)
            return (
                CuaDriverAccessibilityHandler(),
                automation,
                BaseDioramaHandler(),
                GenericFileHandler(),
                GenericDesktopHandler(),
                GenericWindowHandler(),
            )

        if OS_TYPE == "android":
            from .android import (
                AndroidAccessibilityHandler,
                AndroidAutomationHandler,
                AndroidDesktopHandler,
                AndroidFileHandler,
                AndroidWindowHandler,
            )

            handlers: HandlerTuple = (
                AndroidAccessibilityHandler(),
                AndroidAutomationHandler(),
                BaseDioramaHandler(),
                AndroidFileHandler(),
                AndroidDesktopHandler(),
                AndroidWindowHandler(),
            )
        elif OS_TYPE == "darwin":
            from computer_server.diorama.macos import MacOSDioramaHandler

            from .macos import MacOSAccessibilityHandler, MacOSAutomationHandler

            handlers = (
                MacOSAccessibilityHandler(),
                MacOSAutomationHandler(),
                MacOSDioramaHandler(),
                GenericFileHandler(),
                GenericDesktopHandler(),
                GenericWindowHandler(),
            )
        elif OS_TYPE == "linux":
            from .linux import LinuxAccessibilityHandler, LinuxAutomationHandler

            handlers = (
                LinuxAccessibilityHandler(),
                LinuxAutomationHandler(),
                BaseDioramaHandler(),
                GenericFileHandler(),
                GenericDesktopHandler(),
                GenericWindowHandler(),
            )
        elif OS_TYPE == "windows":
            from .windows import WindowsAccessibilityHandler, WindowsAutomationHandler

            handlers = (
                WindowsAccessibilityHandler(),
                WindowsAutomationHandler(),
                BaseDioramaHandler(),
                GenericFileHandler(),
                GenericDesktopHandler(),
                GenericWindowHandler(),
            )
        else:
            raise NotImplementedError(f"OS '{OS_TYPE}' is not supported")

        return handlers

    @classmethod
    def get_handlers(cls) -> HandlerTuple:
        """Return the process-wide handler set shared by HTTP and MCP surfaces."""

        if cls._shared_handlers is None:
            cls._shared_handlers = cls.create_handlers()
        return cls._shared_handlers

    @classmethod
    async def close_handlers(cls) -> None:
        """Close the shared automation runtime exactly once."""

        handlers = cls._shared_handlers
        cls._shared_handlers = None
        if handlers is None:
            return
        close = getattr(handlers[1], "close", None)
        if close is None:
            return
        result = close()
        if inspect.isawaitable(result):
            await result
