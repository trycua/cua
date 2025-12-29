"""Shared provider type definitions for VM metadata and responses.

These base types describe the common shape of objects returned by provider
methods like `list_vms()`.
"""

from __future__ import annotations

from typing import Literal, NotRequired, TypedDict

# Core status values per product docs
VMStatus = Literal[
    "pending",  # VM deployment in progress
    "running",  # VM is active and accessible
    "stopped",  # VM is stopped but not terminated
    "terminated",  # VM has been permanently destroyed
    "failed",  # VM deployment or operation failed
]

OSType = Literal["macos", "linux", "windows"]


class MinimalVM(TypedDict):
    """Minimal VM object shape returned by list calls.

    Providers may include additional fields. Optional fields below are
    common extensions some providers expose or that callers may compute.
    """

    name: str
    status: VMStatus
    # Not always included by all providers
    password: NotRequired[str]
    vnc_url: NotRequired[str]
    api_url: NotRequired[str]


# Convenience alias for list_vms() responses
ListVMsResponse = list[MinimalVM]
