# Sessions package

from .base import DesktopSession, DesktopSetupConfig, get_session
from .remote import RemoteDesktopSession, create_remote_session

__all__ = [
    "DesktopSession",
    "DesktopSetupConfig",
    "get_session",
    "RemoteDesktopSession",
    "create_remote_session",
]
