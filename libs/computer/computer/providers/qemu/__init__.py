"""QEMU VM provider implementation."""

try:
    from .provider import QEMUProvider
    HAS_QEMU = True
    __all__ = ["QEMUProvider"]
except ImportError:
    HAS_QEMU = False
    __all__ = []
