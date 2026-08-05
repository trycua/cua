"""cua-sandbox — ephemeral and persistent sandboxed computer environments.

Usage::

    import cua_sandbox as cua

    # Configure API access
    cua.configure(api_key="sk-...")

    # Local sandbox
    async with cua.sandbox(local=True) as sb:
        await sb.screenshot()

    # Localhost (no sandbox, direct host control)
    async with cua.localhost() as host:
        await host.mouse.click(100, 200)
"""

__version__ = "0.1.0"

from cua_sandbox._auth import login, whoami
from cua_sandbox._config import configure
from cua_sandbox.image import Image
from cua_sandbox.localhost import Localhost, localhost
from cua_sandbox.pool import Pool, Template
from cua_sandbox.runtime.compat import (
    RuntimeSupport,
    check_local_support,
    skip_if_unsupported,
)
from cua_sandbox.sandbox import Sandbox, SandboxInfo, sandbox
from cua_sandbox.transport.cloud import CloudTransport
from fleet_sdk import (
    ClaimLifecycle,
    ClaimSpec,
    CreatePoolRequest,
    CreateTemplateRequest,
    Firmware,
    OsGymSandboxTemplateSpec,
    OsGymSandboxWarmPoolSpec,
    PreservedJson,
    RuntimeKind,
    SandboxService,
    SandboxTemplateRef,
    ServiceProtocol,
)
from fleet_sdk import Template as TemplateResource
from fleet_sdk import (
    VmTemplate,
)

__all__ = [
    "configure",
    "login",
    "whoami",
    "Image",
    "Pool",
    "Template",
    "TemplateResource",
    "CreatePoolRequest",
    "CreateTemplateRequest",
    "ClaimLifecycle",
    "ClaimSpec",
    "PreservedJson",
    "SandboxTemplateRef",
    "OsGymSandboxWarmPoolSpec",
    "OsGymSandboxTemplateSpec",
    "RuntimeKind",
    "VmTemplate",
    "SandboxService",
    "ServiceProtocol",
    "Firmware",
    "Sandbox",
    "SandboxInfo",
    "sandbox",
    "Localhost",
    "localhost",
    "CloudTransport",
    "RuntimeSupport",
    "check_local_support",
    "skip_if_unsupported",
]
