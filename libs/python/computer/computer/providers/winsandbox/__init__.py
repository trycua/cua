"""Windows Sandbox provider for Cua Computer."""

try:
    import winsandbox

    HAS_WINSANDBOX = True
except ImportError:
    HAS_WINSANDBOX = False

from .provider import WinSandboxProvider

__all__ = ["WinSandboxProvider", "HAS_WINSANDBOX"]
