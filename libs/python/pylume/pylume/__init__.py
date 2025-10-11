"""
PyLume Python SDK - A client library for managing macOS VMs with PyLume.

Example:
    >>> from pylume import PyLume, VMConfig
    >>> client = PyLume()
    >>> config = VMConfig(name="my-vm", cpu=4, memory="8GB", disk_size="64GB")
    >>> client.create_vm(config)
    >>> client.run_vm("my-vm")
"""

# Import exceptions then all models
from .exceptions import (
    LumeConfigError,
    LumeConnectionError,
    LumeError,
    LumeImageError,
    LumeNotFoundError,
    LumeServerError,
    LumeTimeoutError,
    LumeVMError,
)
from .models import (
    CloneSpec,
    ImageInfo,
    ImageList,
    ImageRef,
    SharedDirectory,
    VMConfig,
    VMRunOpts,
    VMStatus,
    VMUpdateOpts,
)

# Import main class last to avoid circular imports
from .pylume import PyLume

__version__ = "0.2.1"

__all__ = [
    "PyLume",
    "VMConfig",
    "VMStatus",
    "VMRunOpts",
    "VMUpdateOpts",
    "ImageRef",
    "CloneSpec",
    "SharedDirectory",
    "ImageList",
    "ImageInfo",
    "LumeError",
    "LumeServerError",
    "LumeConnectionError",
    "LumeTimeoutError",
    "LumeNotFoundError",
    "LumeConfigError",
    "LumeVMError",
    "LumeImageError",
]
