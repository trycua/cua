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

from cua_sandbox._auth import login, whoami
from cua_sandbox._config import configure
from cua_sandbox._refs import AmbiguousSandbox
from cua_sandbox._sdk import (
    InvalidArgument,
    InvalidPlacement,
    SpacesdNotAvailable,
    Unsupported,
)
from cua_sandbox.containers import Container, RegistrySecret
from cua_sandbox.image import Image, ImageInfo
from cua_sandbox.interfaces.services import ServiceHandle
from cua_sandbox.options import Probe, PublicUrl, http, tcp
from cua_sandbox.runtime.compat import (
    RuntimeSupport,
    check_local_support,
    skip_if_unsupported,
)
from cua_sandbox.sandbox import Sandbox, SandboxInfo, sandbox
from cua_sandbox.transport.env import EnvTransport

__all__ = [
    "configure",
    "login",
    "whoami",
    "Image",
    "ImageInfo",
    "ServiceHandle",
    "Container",
    "RegistrySecret",
    "Probe",
    "PublicUrl",
    "http",
    "tcp",
    "Sandbox",
    "SandboxInfo",
    "sandbox",
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
