"""cua-sandbox — ephemeral and persistent sandboxed computer environments.

A thin wrapper over the ``cua`` SDK: the local runtimes and cua-spacesd (the
computer interfaces) go through it.

Usage::

    from cua_sandbox import Image, Sandbox, http

    async with Sandbox.ephemeral(
        Image.from_registry("python:3.12-slim"),
        command=["python", "-m", "http.server", "8000"],
        services={"web": 8000},
        wait_for=http("web", "/"),
        on="local",
    ) as sb:
        print((await sb.service("web").request("GET", "/")).status_code)

To control the local machine, use cua-driver (its SDK or MCP server).
"""

__version__ = "0.9.0"  # x-release-please-version

# Retired Cua Cloud pools (every call raises CloudEnded).
from cua_sandbox import pools  # noqa: E402
from cua_sandbox._auth import login, whoami
from cua_sandbox._config import configure
from cua_sandbox._ended import CLOUD_ENDED
from cua_sandbox._refs import AmbiguousSandbox
from cua_sandbox._sdk import (
    InvalidArgument,
    InvalidPlacement,
    SpacesdNotAvailable,
    Unsupported,
)
from cua_sandbox.containers import Container, RegistrySecret
from cua_sandbox.generated.image_models import ImageFileReference
from cua_sandbox.image import Image, ImageInfo
from cua_sandbox.interfaces import SignedServiceURL
from cua_sandbox.interfaces.services import ServiceHandle
from cua_sandbox.options import CloudOptions, Probe, PublicUrl, http, tcp
from cua_sandbox.pool import Pool, Template
from cua_sandbox.runtime.compat import (
    RuntimeSupport,
    check_local_support,
    skip_if_unsupported,
)
from cua_sandbox.sandbox import Sandbox, SandboxInfo, sandbox
from cua_sandbox.spec import (
    ClaimSecretsNotDelivered,
    PoolAccessDeniedError,
    PoolExport,
    PoolOptions,
    PoolSpecMismatch,
    SandboxSpec,
    generate_claim_token,
)
from cua_sandbox.transport.cloud import CloudTransport
from cua_sandbox.transport.env import EnvTransport

# Names that came from Cua Cloud's Fleet SDK package and went with it.
# Importing one explains why instead of a bare "cannot import name".
_RETIRED_FLEET_NAMES = frozenset(
    {
        "ClaimSpec",
        "CreatePoolRequest",
        "CreatePoolRequestBuilder",
        "CreateTemplateRequest",
        "CreateTemplateRequestBuilder",
        "Firmware",
        "OsGymSandboxTemplateSpec",
        "OsGymSandboxTemplateSpecBuilder",
        "OsGymSandboxWarmPoolSpec",
        "OsGymSandboxWarmPoolSpecBuilder",
        "RuntimeKind",
        "SandboxService",
        "SandboxServiceBuilder",
        "SandboxTemplateRef",
        "SandboxTemplateRefBuilder",
        "ServiceProtocol",
        "TemplateResource",
        "VmTemplate",
        "VmTemplateBuilder",
        "WarmPoolAutoscaling",
        "WarmPoolAutoscalingBuilder",
        "fleet_auth_source",
    }
)


def __getattr__(name: str):
    if name in _RETIRED_FLEET_NAMES:
        raise ImportError(f"cannot import name {name!r} from 'cua_sandbox': {CLOUD_ENDED}")
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


__all__ = [
    "configure",
    "pools",
    "login",
    "whoami",
    "Image",
    "ImageInfo",
    "ImageFileReference",
    "Pool",
    "PoolAccessDeniedError",
    "PoolExport",
    "PoolOptions",
    "PoolSpecMismatch",
    "ClaimSecretsNotDelivered",
    "SandboxSpec",
    "generate_claim_token",
    "Template",
    "SignedServiceURL",
    "ServiceHandle",
    "CloudOptions",
    "Container",
    "RegistrySecret",
    "Probe",
    "PublicUrl",
    "http",
    "tcp",
    "Sandbox",
    "SandboxInfo",
    "sandbox",
    "CloudTransport",
    "EnvTransport",
    "SpacesdNotAvailable",
    "InvalidArgument",
    "InvalidPlacement",
    "Unsupported",
    "AmbiguousSandbox",
    "RuntimeSupport",
    "check_local_support",
    "skip_if_unsupported",
]
