"""cua.runtime — sandbox runtime backends.

Usage::

    from cua.runtime import QEMURuntime, TartRuntime

Needs the ``sandbox`` extra: ``pip install 'cua[sandbox]'``.
"""

from cua_sandbox.runtime import (
    AndroidEmulatorRuntime,
    DockerRuntime,
    HyperVRuntime,
    LumeRuntime,
    QEMUBaremetalRuntime,
    QEMUDockerRuntime,
    QEMURuntime,
    QEMUWSL2Runtime,
    Runtime,
    RuntimeInfo,
    TartRuntime,
)

__all__ = [
    "Runtime",
    "RuntimeInfo",
    "DockerRuntime",
    "QEMURuntime",
    "QEMUDockerRuntime",
    "QEMUBaremetalRuntime",
    "QEMUWSL2Runtime",
    "LumeRuntime",
    "HyperVRuntime",
    "AndroidEmulatorRuntime",
    "TartRuntime",
]
