"""Named service interface for sandboxes that expose auxiliary endpoints."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from cua_sandbox.transport.base import Transport


@dataclass(frozen=True)
class SignedServiceURL:
    """A revocable public URL for one sandbox service."""

    id: str
    namespace: str
    claim: str
    sandbox: str
    service: str
    label: str | None
    url: str
    created_at: str
    expires_at: str
    revoked_at: str | None

    @classmethod
    def from_resource(cls, resource: Any) -> "SignedServiceURL":
        return cls(
            id=resource.id,
            namespace=resource.namespace,
            claim=resource.claim,
            sandbox=resource.sandbox,
            service=resource.service,
            label=resource.label,
            url=resource.url,
            created_at=resource.created_at,
            expires_at=resource.expires_at,
            revoked_at=resource.revoked_at,
        )


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

        Fleet routes the request through the authenticated lease. Other
        transports raise :class:`NotImplementedError` unless they expose named
        services as well.
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
        """Create a revocable public URL for a named Fleet service."""
        resource = await self._transport.create_signed_service_url(
            name,
            label=label,
            expires_in_seconds=expires_in_seconds,
        )
        return SignedServiceURL.from_resource(resource)

    async def list_signed_urls(self) -> list[SignedServiceURL]:
        """List signed service URLs created for this Fleet sandbox claim."""
        resources = await self._transport.list_signed_service_urls()
        return [SignedServiceURL.from_resource(resource) for resource in resources]

    async def revoke_signed_url(self, signed_url: SignedServiceURL) -> None:
        """Revoke a previously created signed service URL."""
        await self._transport.revoke_signed_service_url(signed_url)
