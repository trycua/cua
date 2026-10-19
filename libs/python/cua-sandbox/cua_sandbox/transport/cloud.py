"""``CloudTransport``: Cua Cloud has ended.

The transport that drove API-key cloud VMs is gone. The name stays importable
so existing imports keep working; constructing it raises
:class:`~cua_sandbox._ended.CloudEnded`.
"""

from __future__ import annotations

from typing import Any

from cua_sandbox._ended import CloudEnded
from cua_sandbox.transport.base import Transport


class CloudTransport(Transport):
    """Retired: every Cua Cloud entry point raises :class:`CloudEnded`."""

    def __new__(cls, *args: Any, **kwargs: Any) -> "CloudTransport":
        raise CloudEnded()
