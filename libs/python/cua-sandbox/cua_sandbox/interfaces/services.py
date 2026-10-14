"""Named services of a sandbox: requests, URLs and shareable URLs.

``sb.service(name)`` is the portable handle (the same calls local and in the
cloud); ``sb.services`` keeps the older request/signed-URL helpers.
"""

from __future__ import annotations

import json as _json
import warnings
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import TYPE_CHECKING, Any, Mapping, Optional, Sequence, Union

from cua_sandbox.transport.base import Transport

if TYPE_CHECKING:
    from cua_sandbox.options import PublicUrl
    from cua_sandbox.sandbox import Sandbox


@dataclass(frozen=True)
class SignedServiceURL:
    """A revocable public URL for one sandbox service.

    Provider internals (the cloud namespace and claim) live in ``details``.
    """

    id: str
    sandbox: str
    service: str
    label: str | None
    url: str
    created_at: str
    expires_at: str
    revoked_at: str | None
    details: dict = field(default_factory=dict)

    @classmethod
    def from_resource(cls, resource: Any) -> "SignedServiceURL":
        return cls(
            id=resource.id,
            sandbox=resource.sandbox,
            service=resource.service,
            label=resource.label,
            url=resource.url,
            created_at=resource.created_at,
            expires_at=resource.expires_at,
            revoked_at=resource.revoked_at,
            details={"namespace": resource.namespace, "claim": resource.claim},
        )

    @property
    def namespace(self) -> str | None:
        """Deprecated: ``details["namespace"]``."""
        warnings.warn(
            "SignedServiceURL.namespace is deprecated; use details['namespace']",
            DeprecationWarning,
            stacklevel=2,
        )
        return self.details.get("namespace")

    @property
    def claim(self) -> str | None:
        """Deprecated: ``details["claim"]``."""
        warnings.warn(
            "SignedServiceURL.claim is deprecated; use details['claim']",
            DeprecationWarning,
            stacklevel=2,
        )
        return self.details.get("claim")


class Services:
    """Request a named service exposed by the active sandbox connection."""

    def __init__(self, transport: Transport):
        self._transport = transport

    async def request(
        self,
        name: str,
        *,
        method: str,
        path: str,
        json: Any = None,
        headers: dict[str, str] | None = None,
    ) -> Any:
        """Send a request to a named service on this sandbox.

        Prefer ``sb.service(name).request(...)``, which works the same local
        and in the cloud.
        """
        return await self._transport.request_service(
            name, method=method, path=path, json_body=json, headers=headers
        )

    async def create_signed_url(
        self,
        name: str,
        *,
        expires_in_seconds: int,
        label: str | None = None,
    ) -> SignedServiceURL:
        """Create a revocable public URL for a named cloud service.

        Prefer ``sb.public_url(name, ttl=...)``, which also works locally.
        """
        resource = await self._transport.create_signed_service_url(
            name,
            label=label,
            expires_in_seconds=expires_in_seconds,
        )
        return SignedServiceURL.from_resource(resource)

    async def list_signed_urls(self) -> list[SignedServiceURL]:
        """List signed service URLs created for this cloud sandbox."""
        resources = await self._transport.list_signed_service_urls()
        return [SignedServiceURL.from_resource(resource) for resource in resources]

    async def revoke_signed_url(self, signed_url: SignedServiceURL) -> None:
        """Revoke a previously created signed service URL."""
        await self._transport.revoke_signed_service_url(signed_url)


Headers = Union[Mapping[str, str], Sequence[tuple[str, str]], None]


def _unix_to_iso(seconds: int) -> str:
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat().replace("+00:00", "Z")


class ServiceHandle:
    """One named service of a sandbox (``sb.service("mcp")``).

    The same calls work local and in the cloud: requests carry your headers
    (the cloud gateway's credentials are added for you), ``url()`` is usable
    from this machine and ``public_url()`` can be shared.
    """

    def __init__(self, sandbox: "Sandbox", name: str) -> None:
        self._sandbox = sandbox
        self.name = name

    def __repr__(self) -> str:
        return f"ServiceHandle({self.name!r})"

    async def _native(self) -> Any:
        return (await self._sandbox._native_handle()).service(self.name)

    async def request(
        self,
        method: str,
        path: str = "/",
        *,
        headers: Headers = None,
        body: bytes | str | None = None,
        json: Any = None,
        timeout: float = 30.0,
    ) -> Any:
        """One HTTP request to ``path`` on the service; returns an
        ``httpx.Response``. ``authorization`` is refused in the cloud, where
        the gateway credentials own it."""
        import httpx
        from cua_sandbox._sdk import millis, native

        if body is not None and json is not None:
            raise ValueError("pass body= or json=, not both")
        pairs = list(headers.items()) if isinstance(headers, Mapping) else list(headers or [])
        if json is not None:
            body = _json.dumps(json).encode()
            if not any(k.lower() == "content-type" for k, _ in pairs):
                pairs.append(("content-type", "application/json"))
        if isinstance(body, str):
            body = body.encode()
        n = native()
        response = await (await self._native()).request(
            method.upper(),
            path if path.startswith("/") else f"/{path}",
            body,
            millis(timeout),
            [n.HttpHeader(name=k, value=v) for k, v in pairs],
        )
        return httpx.Response(
            response.status,
            headers=[(h.name, h.value) for h in response.headers],
            content=bytes(response.body),
            request=httpx.Request(method.upper(), f"http://{self.name}.service{path}"),
        )

    async def url(self) -> str:
        """A URL for the service usable from this machine with no credentials
        (no trailing slash): the published loopback port locally, a signed
        URL (1 h, renewed on later calls) in the cloud."""
        return str(await (await self._native()).url())

    async def public_url(self, ttl: float = 3600, *, label: Optional[str] = None) -> "PublicUrl":
        """A shareable URL that stops working after ``ttl`` seconds (60 s to
        24 h). See :meth:`cua_sandbox.Sandbox.public_url`."""
        return await self._sandbox.public_url(self.name, ttl=ttl, label=label)


def public_url_from_native(u: Any) -> "PublicUrl":
    from cua_sandbox.options import PublicUrl

    return PublicUrl(
        url=u.url,
        expires_at=_unix_to_iso(int(u.expires_at_unix)),
        id=u.id,
        service=u.service,
        provider_details=dict(u.provider_details),
    )
