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
        if OS_TYPE == "android":
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
