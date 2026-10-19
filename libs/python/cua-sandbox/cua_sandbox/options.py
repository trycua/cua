"""Sandbox options: command, env, services, readiness probes and sizing.

::

    from cua_sandbox import Image, Sandbox, http

    sb = await Sandbox.create(
        Image.from_registry("python:3.12-slim"),
        command=["python", "-m", "my_mcp", "--port", "8765"],
        env={"FOO": "bar"},
        services={"mcp": 8765},
        wait_for=http("mcp", "/health"),
        on="local",
        kind="container",                # "auto" | "container" | "vm"
        runtime="gvisor",                # the engine; "auto" picks one
    )
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import timedelta
from typing import TYPE_CHECKING, Any, Iterable, Mapping, Optional, Sequence, Union

if TYPE_CHECKING:
    from cua_sandbox.pool import Pool


@dataclass(frozen=True)
class CloudOptions:
    """Options for Cua Cloud sandboxes. Cua Cloud has ended: passing
    ``cloud=`` to ``Sandbox.create`` or ``Sandbox.ephemeral`` raises
    :class:`~cua_sandbox._ended.CloudEnded`, and the class goes away in the
    next minor release.

    * ``warm``: keep one sandbox of this image ready.
    * ``max_pool_size``: most sandboxes of this image at once.
    * ``claim_ttl``: how long the sandbox outlived the process.
    * ``pool``: claim from this existing pool.
    * ``apply``: with ``pool``, update the pool's template.
    """

    warm: Optional[bool] = None
    max_pool_size: Optional[int] = None
    claim_ttl: Union[float, timedelta, None] = None
    pool: "Pool | str | None" = None
    apply: bool = False


@dataclass(frozen=True)
class Probe:
    """A readiness probe on a declared service: TCP, or ``GET path`` returning
    2xx when ``path`` is set. Build it with :func:`tcp` or :func:`http`."""

    service: str
    path: Optional[str] = None

    def native(self) -> Any:
        """The ``cua.ReadinessProbe`` for this probe."""
        from cua_sandbox._sdk import native

        n = native()
        return n.ReadinessProbe(port=0, http_path=self.path, http_status=None, service=self.service)


def tcp(service: str) -> Probe:
    """Ready once a TCP connect to the declared ``service`` succeeds."""
    return Probe(service=service)


def http(service: str, path: str = "/") -> Probe:
    """Ready once ``GET path`` on the declared ``service`` returns 2xx."""
    return Probe(service=service, path=path if path.startswith("/") else f"/{path}")


WaitFor = Union[Probe, Sequence[Probe], None]


def probes(wait_for: WaitFor) -> list[Probe]:
    """``wait_for`` as a list (a single probe, a sequence, or ``None``)."""
    if wait_for is None:
        return []
    if isinstance(wait_for, Probe):
        return [wait_for]
    out = list(wait_for)
    for p in out:
        if not isinstance(p, Probe):
            raise TypeError("wait_for takes tcp(service) / http(service, path) probes")
    return out


def check_services(services: Optional[Mapping[str, int]], wait_for: Iterable[Probe]) -> dict:
    """Validates ``services`` (name -> guest port) and that every probe names
    one of them."""
    out: dict[str, int] = {}
    for name, port in (services or {}).items():
        if not isinstance(name, str) or not name:
            raise ValueError("service names must be non-empty strings")
        if isinstance(port, bool) or not isinstance(port, int) or not 0 < port < 65536:
            raise ValueError(f"service {name!r}: port must be 1-65535, got {port!r}")
        out[name] = port
    for p in wait_for:
        if p.service not in out and p.service != "env":
            raise ValueError(
                f"wait_for names service {p.service!r}, which is not declared in services= "
                f"(declared: {sorted(out)})"
            )
    return out


#: ``network=`` values: ``"default"`` gives the guest outbound network (like a
#: Docker container); ``"none"`` cuts it while published ports still work.
NETWORK_MODES = ("default", "none")


def check_network(network: Optional[str]) -> Optional[str]:
    """Normalizes ``network=`` (``None`` / ``"default"`` / ``"none"``).

    Only local QEMU VMs can run with ``"none"``; containers and Lume reject it
    (``Unsupported``) rather than ignore it.
    """
    if network is None:
        return None
    if not isinstance(network, str):
        raise ValueError(f"network must be one of {NETWORK_MODES}, got {network!r}")
    value = network.strip().lower() or "default"
    if value not in NETWORK_MODES:
        raise ValueError(f"network {network!r}: expected 'default' or 'none'")
    return value


def parse_memory(memory: Union[str, int, None]) -> Optional[int]:
    """``"4GB"`` / ``"512MB"`` / ``4096`` (MiB) -> MiB."""
    if memory is None:
        return None
    if isinstance(memory, bool):
        raise ValueError("memory must be like '4GB', '512MB' or MiB as an int")
    if isinstance(memory, int):
        return memory
    text = str(memory).strip().upper().replace(" ", "")
    for suffix, factor in (
        ("GIB", 1024),
        ("GB", 1024),
        ("G", 1024),
        ("MIB", 1),
        ("MB", 1),
        ("M", 1),
    ):
        if text.endswith(suffix):
            number = text[: -len(suffix)]
            break
    else:
        number, factor = text, 1024
    try:
        value = float(number)
    except ValueError as error:
        raise ValueError(f"bad memory {memory!r} (use '4GB' or '512MB')") from error
    if value <= 0:
        raise ValueError(f"bad memory {memory!r}")
    return int(value * factor)


@dataclass(frozen=True)
class PublicUrl:
    """A shareable URL of a sandbox service that stops working at
    ``expires_at``: a loopback URL with its own token, served by the cua
    daemon."""

    url: str
    expires_at: str
    id: str
    service: str
    provider_details: dict = field(default_factory=dict)
