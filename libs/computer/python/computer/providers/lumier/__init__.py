"""Lumier VM provider implementation."""

try:
    # Use the same import approach as in the Lume provider
    from .provider import LumierProvider
    HAS_LUMIER = True
except ImportError:
    HAS_LUMIER = False
