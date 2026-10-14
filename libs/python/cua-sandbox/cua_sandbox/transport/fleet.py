"""Transport for a bound Fleet claim.

Two planes, both daemon-agnostic:

* **Named services** (``sb.services.request(...)``) go through the Fleet
  gateway with ``CyclopsClient.service_request``: any HTTP service the image
  declares, no guest agent assumed.
* **Computer interfaces** (``screen``, ``mouse``, ``shell`` ...) go to
  cua-spacesd on the claim's ``env`` service through the ``cua`` SDK
  (gRPC-Web through the gateway, Fleet bearer + claim header). A claim whose
  image has no spacesd still binds and serves named services; only the
  interfaces raise :class:`~cua_sandbox._sdk.SpacesdNotAvailable`.
"""

from __future__ import annotations

import json
import math
from typing import Any, Dict, List, Mapping, Optional

import httpx
from cua_sandbox._sdk import ENV_SERVICE, fleet_sandbox, millis
from cua_sandbox.transport.env import EnvTransport
from cua_sandbox.transport.osworld import OSWorldOverServiceMixin
from fleet_sdk import HttpHeader, HttpRequest, HttpRequestBuilder

#: How long the first interface call waits for spacesd on a fresh claim.
DEFAULT_ENV_READY_TIMEOUT = 300.0
#: The service an OSWorld pool publishes its Flask server under (legacy adapter).
OSWORLD_SERVICE = "server"


def build_http_request(
    *,
    method: str,
    url: str,
    headers: Optional[List[Any]] = None,
    body: Optional[bytes] = None,
    timeout_secs: Optional[int] = None,
    max_response_bytes: Optional[int] = None,
) -> HttpRequest:
    """Construct ``fleet_sdk.HttpRequest`` through the builder API.

    The builder treats optional record fields as skippable, so request
    construction keeps working when the Fleet SDK adds fields. An absent
    ``timeout_secs`` falls back to the native client's 30-second default.
    """
    builder = HttpRequestBuilder().method(method).url(url).headers(headers or [])
    if body is not None:
        builder = builder.body(body)
    if timeout_secs is not None:
        builder = builder.timeout_secs(timeout_secs)
    if max_response_bytes is not None:
        builder = builder.max_response_bytes(max_response_bytes)
    return builder.build()


def _whole_seconds(timeout: Optional[float]) -> Optional[int]:
    if timeout is None or timeout <= 0:
        return None
    return math.ceil(timeout)


class FleetTransport(EnvTransport):
    """Interfaces over cua-spacesd, named services over the Fleet gateway."""

    def __init__(
        self,
        *,
        sdk: Any,
        bound: Any,
        service_name: str = ENV_SERVICE,
        timeout: float = 30.0,
        owns_sdk: bool = False,
        env_ready_timeout: float = DEFAULT_ENV_READY_TIMEOUT,
        env_token: Optional[str] = None,
        **_: Any,
    ) -> None:
        self._env_token = env_token
        self._sdk = sdk
        self._bound = bound
        self._service_name = service_name
        self._timeout = timeout
        self._owns_sdk = owns_sdk
        self._sdk_closed = False
        self._native_fleet_sandbox: Any = None
        # The claim's image (a named pool's template image), handed to the
        # SDK handle so its ``image_info()`` reports it.
        self._image_info: Any = None
        super().__init__(
            env_factory=self._open_env,
            ready_timeout=env_ready_timeout,
            probe_timeout=min(timeout, 15.0),
        )

    async def _fleet_handle(self) -> Any:
        if self._native_fleet_sandbox is None:
            # The claim's per-claim env token, when it has one.
            token = (self._env_token,) if self._env_token else ()
            # The claim's image, when known (a named pool's template image).
            image = {"image_info": self._image_info} if self._image_info is not None else {}
            self._native_fleet_sandbox = await fleet_sandbox(
                self._bound.namespace, self._bound.claim, *token, **image
            )
        return self._native_fleet_sandbox

    async def _open_env(self) -> Any:
        from cua_sandbox._sdk import SpacesdNotAvailable

        if ENV_SERVICE not in self._bound.services:
            raise SpacesdNotAvailable(
                f"Fleet sandbox {self._bound.name!r} exposes no {ENV_SERVICE!r} service "
                f"(services: {list(self._bound.services)}); its image has no "
                "cua-spacesd, so only named services (sb.services.request) are available"
            )
        handle = await self._fleet_handle()
        return await handle.spacesd(millis(self._probe_timeout))

    async def connect(self) -> None:
        # Daemon-agnostic: the claim is bound; nothing in the guest is probed.
        await super().connect()

    async def disconnect(self) -> None:
        await super().disconnect()
        self._native_fleet_sandbox = None
        # A transport constructed with owns_sdk=True (e.g. by _ClaimHandle.wait)
        # is the sole holder of its Fleet client, so disconnect is where that
        # client's HTTP resources are returned.
        if self._owns_sdk and not self._sdk_closed:
            await self._sdk.close()
            self._sdk_closed = True

    async def request_service(
        self,
        name: str,
        *,
        method: str,
        path: str,
        json_body: Any = None,
        body: bytes | None = None,
        headers: dict[str, str] | list[tuple[str, str]] | None = None,
        timeout: float | None = None,
        max_response_bytes: int | None = None,
    ) -> httpx.Response:
        if name not in self._bound.services:
            raise ValueError(f"Fleet sandbox does not expose service {name!r}")
        return await self._request(
            method,
            path,
            json_body=json_body,
            body=body,
            service_name=name,
            extra_headers=headers,
            timeout=timeout,
            max_response_bytes=max_response_bytes,
        )

    async def native_handle(self) -> Any:
        """The ``cua.Sandbox`` handle of this claim (services, forwards,
        public URLs)."""
        return await self._fleet_handle()

    async def forward_tunnel(self, sandbox_port: int | str) -> "Any":
        """A loopback forward to ``sandbox_port``: over cua-spacesd's
        tunnel when the image has it, else an HTTP/WebSocket proxy through
        the cloud gateway (the port must be a declared service)."""
        if self._delegate is not None:
            return await self._delegate.forward_tunnel(sandbox_port)
        if not isinstance(sandbox_port, int):
            raise ValueError("only numeric TCP ports can be forwarded")
        forward = await (await self._fleet_handle()).forward(sandbox_port)
        return self._track_forward(forward, sandbox_port)

    def _declared_services(self) -> Dict[str, Optional[int]]:
        services = self._bound.services
        if isinstance(services, Mapping):
            return {str(k): (int(v) if v is not None else None) for k, v in services.items()}
        return {str(k): None for k in services or ()}

    async def native_service(self, name: str) -> Any:
        if name not in self._bound.services:
            raise ValueError(
                f"Fleet sandbox does not expose service {name!r} "
                f"(services: {list(self._bound.services)})"
            )
        return (await self._fleet_handle()).service(name)

    async def create_signed_service_url(
        self,
        name: str,
        *,
        label: str | None,
        expires_in_seconds: int,
    ) -> Any:
        return await self._sdk.create_signed_service_url(
            self._bound,
            name,
            label=label,
            expires_in_seconds=expires_in_seconds,
        )

    async def list_signed_service_urls(self) -> list[Any]:
        return await self._sdk.list_signed_service_urls(self._bound)

    async def revoke_signed_service_url(self, signed_service_url: Any) -> None:
        await self._sdk.revoke_signed_service_url(signed_service_url)

    async def _request(
        self,
        method: str,
        path: str,
        *,
        json_body: Any = None,
        body: bytes | None = None,
        service_name: str | None = None,
        extra_headers: dict[str, str] | list[tuple[str, str]] | None = None,
        timeout: float | None = None,
        max_response_bytes: int | None = None,
    ) -> httpx.Response:
        """One HTTP request to a named service through the Fleet gateway."""
        if body is not None and json_body is not None:
            raise ValueError("Specify either body or json_body, not both")
        if json_body is not None:
            body = json.dumps(json_body).encode()
        request_headers = (
            [] if json_body is None else [HttpHeader(name="content-type", value="application/json")]
        )
        header_items = (
            extra_headers.items() if isinstance(extra_headers, dict) else (extra_headers or [])
        )
        for header, value in header_items:
            request_headers.append(HttpHeader(name=header, value=value))
        result = await self._sdk.service_request(
            self._bound,
            service_name or self._service_name,
            path,
            build_http_request(
                method=method,
                url=f"https://service.invalid{path}",
                headers=request_headers,
                body=body,
                timeout_secs=_whole_seconds(self._timeout if timeout is None else timeout),
                max_response_bytes=max_response_bytes,
            ),
        )
        if max_response_bytes is not None and len(result.body) > max_response_bytes:
            raise RuntimeError("Fleet service response exceeds the configured size limit")
        return httpx.Response(
            result.status,
            headers=[(header.name, header.value) for header in result.headers],
            content=result.body,
            request=httpx.Request(method, f"https://service.invalid{path}"),
        )

    async def get_environment(self) -> str:
        try:
            return await super().get_environment()
        except Exception:  # noqa: BLE001 - no driver: the pool OS is unknown
            return "linux"


class OSWorldFleetTransport(OSWorldOverServiceMixin, FleetTransport):
    """Legacy OSWorld adapter: a claim whose ``server`` service is the OSWorld Flask API.

    The OSWorld guest has no cua-spacesd; screenshots, screen size and shell
    commands go to the Flask server through the Fleet gateway instead.
    """

    def __init__(self, *, service_name: str = OSWORLD_SERVICE, **kwargs: Any) -> None:
        if service_name == ENV_SERVICE:
            service_name = OSWORLD_SERVICE
        super().__init__(service_name=service_name, **kwargs)


def fleet_transport_for(agent_type: Optional[str]) -> type[FleetTransport]:
    """Pick the Fleet transport class matching an image's ``agent_type`` hint."""
    return OSWorldFleetTransport if agent_type == "osworld" else FleetTransport


def claim_service_for(agent_type: Optional[str], service: str) -> str:
    """The service a claim waits on: OSWorld pools publish ``server``, not ``env``."""
    if agent_type == "osworld" and service == ENV_SERVICE:
        return OSWORLD_SERVICE
    return service
