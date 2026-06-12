import logging
import os
from typing import Tuple

from computer_server.diorama.base import BaseDioramaHandler

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

if OS_TYPE == "android":
    from .android import (
        AndroidAccessibilityHandler,
        AndroidAutomationHandler,
        AndroidDesktopHandler,
        AndroidFileHandler,
        AndroidWindowHandler,
    )
elif OS_TYPE == "darwin":
    from computer_server.diorama.macos import MacOSDioramaHandler

    from .macos import MacOSAccessibilityHandler, MacOSAutomationHandler
elif OS_TYPE == "linux":
    from .linux import LinuxAccessibilityHandler, LinuxAutomationHandler
elif OS_TYPE == "windows":
    from .windows import WindowsAccessibilityHandler, WindowsAutomationHandler

from .generic import GenericDesktopHandler, GenericFileHandler, GenericWindowHandler


class HandlerFactory:
    """Factory for creating OS-specific handlers."""

    @staticmethod
    def create_handlers() -> Tuple[
        BaseAccessibilityHandler,
        BaseAutomationHandler,
        BaseDioramaHandler,
        BaseFileHandler,
        BaseDesktopHandler,
        BaseWindowHandler,
    ]:
        """Create and return appropriate handlers for the current OS.

        Returns:
            Tuple[BaseAccessibilityHandler, BaseAutomationHandler, BaseDioramaHandler, BaseFileHandler]: A tuple containing
            the appropriate accessibility, automation, diorama, and file handlers for the current OS.

        Raises:
            NotImplementedError: If the current OS is not supported
            RuntimeError: If unable to determine the current OS
        """
        backend = os.environ.get("CUA_BACKEND", "native")
        vnc_host = os.environ.get("CUA_VNC_HOST")
        if backend == "vnc" or vnc_host:
            if not vnc_host:
                raise RuntimeError(
                    "CUA_VNC_HOST must be set when using VNC backend "
                    "(--backend=vnc requires --vnc-host)"
                )
            from .vnc import VNCAccessibilityHandler, VNCAutomationHandler

            vnc_port = int(os.environ.get("CUA_VNC_PORT", "5900"))
            vnc_password = os.environ.get("CUA_VNC_PASSWORD", "")
            logger.info(f"Using VNC backend → {vnc_host}:{vnc_port}")
            return (
                VNCAccessibilityHandler(),
                VNCAutomationHandler(host=vnc_host, port=vnc_port, password=vnc_password),
                BaseDioramaHandler(),
                GenericFileHandler(),
                GenericDesktopHandler(),
                GenericWindowHandler(),
            )
        elif OS_TYPE == "android":
            return (
                AndroidAccessibilityHandler(),
                AndroidAutomationHandler(),
                BaseDioramaHandler(),
                AndroidFileHandler(),
                AndroidDesktopHandler(),
                AndroidWindowHandler(),
            )
        elif OS_TYPE == "darwin":
            return (
                MacOSAccessibilityHandler(),
                MacOSAutomationHandler(),
                MacOSDioramaHandler(),
                GenericFileHandler(),
                GenericDesktopHandler(),
                GenericWindowHandler(),
            )
        elif OS_TYPE == "linux":
            return (
                LinuxAccessibilityHandler(),
                LinuxAutomationHandler(),
                BaseDioramaHandler(),
                GenericFileHandler(),
                GenericDesktopHandler(),
                GenericWindowHandler(),
            )
        elif OS_TYPE == "windows":
            return (
                WindowsAccessibilityHandler(),
                WindowsAutomationHandler(),
                BaseDioramaHandler(),
                GenericFileHandler(),
                GenericDesktopHandler(),
                GenericWindowHandler(),
            )
        else:
            raise NotImplementedError(f"OS '{OS_TYPE}' is not supported")
