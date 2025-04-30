import os
import subprocess
from typing import Tuple, Type
from .base import BaseAccessibilityHandler, BaseAutomationHandler

HANDLER_TYPES = {}

try:
    from .macos import MacOSAccessibilityHandler, MacOSAutomationHandler
    HANDLER_TYPES["macos"] = (MacOSAccessibilityHandler, MacOSAutomationHandler)
except Exception:
    pass
try:
    from .windows import WindowsAccessibilityHandler, WindowsAutomationHandler
    HANDLER_TYPES["windows"] = (WindowsAccessibilityHandler, WindowsAutomationHandler)
except Exception as e:
    pass

if not HANDLER_TYPES:
    raise RuntimeError("No handlers found")

class HandlerFactory:
    """Factory for creating OS-specific handlers."""
    
    @staticmethod
    def _get_current_os() -> str:
        """Determine the current OS.
        
        Returns:
            str: The OS type ('darwin', 'windows', or 'linux')
            
        Raises:
            RuntimeError: If unable to determine the current OS
        """
        match os.name:
            case 'nt':
                return 'windows'
            case 'posix':
                try:
                    # Use uname -s to determine OS since this runs on the target machine
                    result = subprocess.run(['uname', '-s'], capture_output=True, text=True)
                    if result.returncode != 0:
                        raise RuntimeError(f"uname command failed: {result.stderr}")
                    return result.stdout.strip().lower()
                except Exception as e:
                    raise RuntimeError(f"Failed to determine current OS: {str(e)}")
            case _:
                raise RuntimeError(f"Unexpected OS name: {os.name}")
    
    @staticmethod
    def create_handlers() -> Tuple[BaseAccessibilityHandler, BaseAutomationHandler]:
        """Create and return appropriate handlers for the current OS.
        
        Returns:
            Tuple[BaseAccessibilityHandler, BaseAutomationHandler]: A tuple containing
            the appropriate accessibility and automation handlers for the current OS.
            
        Raises:
            NotImplementedError: If the current OS is not supported
            RuntimeError: If unable to determine the current OS
        """
        os_type = HandlerFactory._get_current_os()
        if not (handler_types := HANDLER_TYPES.get(os_type)):
            raise NotImplementedError(f"No handlers for OS '{os_type}'")
        return tuple(handler() for handler in handler_types)
