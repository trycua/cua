"""OIDC device authorization and refresh support for run.cua.ai."""

import asyncio
import time
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from typing import Any

import aiohttp
from cua_cli.auth.store import OAuthCredentials, load_credentials, save_credentials

DEFAULT_RUN_API_BASE = "https://run.cua.ai"
DEFAULT_OIDC_ISSUER = "https://auth.cua.ai/realms/cyclops-cs"
DEFAULT_CLIENT_ID = "cua-cli"
DEFAULT_SCOPE = "openid profile offline_access"
DEVICE_GRANT_TYPE = "urn:ietf:params:oauth:grant-type:device_code"


class OidcError(RuntimeError):
    """Raised for an OIDC protocol or authorization failure."""


HttpRequest = Callable[[str, str, Mapping[str, str] | None], Awaitable[tuple[int, dict[str, Any]]]]
Sleep = Callable[[float], Awaitable[None]]


@dataclass(frozen=True)
class OidcDiscovery:
    """Relevant endpoints published by OIDC discovery."""

    token_endpoint: str
    device_authorization_endpoint: str
    revocation_endpoint: str | None

    @classmethod
    def from_response(cls, value: Mapping[str, Any]) -> "OidcDiscovery":
        try:
            return cls(
                token_endpoint=str(value["token_endpoint"]),
                device_authorization_endpoint=str(value["device_authorization_endpoint"]),
                revocation_endpoint=(
                    str(value["revocation_endpoint"]) if value.get("revocation_endpoint") else None
                ),
            )
        except (KeyError, TypeError) as error:
            raise OidcError("OIDC issuer returned an incomplete discovery document.") from error


@dataclass(frozen=True)
class DeviceCode:
    """A device authorization response."""

    device_code: str
    user_code: str
    verification_uri: str
    verification_uri_complete: str | None
    expires_in: int
    interval: int

    @classmethod
    def from_response(cls, value: Mapping[str, Any]) -> "DeviceCode":
        try:
            return cls(
                device_code=str(value["device_code"]),
                user_code=str(value["user_code"]),
                verification_uri=str(value["verification_uri"]),
                verification_uri_complete=(
                    str(value["verification_uri_complete"])
                    if value.get("verification_uri_complete")
                    else None
                ),
                expires_in=int(value["expires_in"]),
                interval=max(1, int(value.get("interval", 5))),
            )
        except (KeyError, TypeError, ValueError) as error:
            raise OidcError(
                "OIDC issuer returned an invalid device authorization response."
            ) from error


async def _aiohttp_form_request(
    method: str, url: str, form: Mapping[str, str] | None
) -> tuple[int, dict[str, Any]]:
    timeout = aiohttp.ClientTimeout(total=15)
    headers = {"Accept": "application/json"}
    async with aiohttp.ClientSession() as session:
        async with session.request(
            method, url, data=form, headers=headers, timeout=timeout
        ) as response:
            try:
                payload = await response.json(content_type=None)
            except (aiohttp.ClientError, ValueError):
                payload = {"error_description": await response.text()}
    if not isinstance(payload, dict):
        payload = {"error_description": "OIDC server returned a non-object response."}
    return response.status, payload


class OidcClient:
    """Minimal public-client OIDC implementation using standard OAuth grants."""

    def __init__(
        self,
        oidc_issuer: str = DEFAULT_OIDC_ISSUER,
        client_id: str = DEFAULT_CLIENT_ID,
        scope: str = DEFAULT_SCOPE,
        request: HttpRequest = _aiohttp_form_request,
        sleep: Sleep = asyncio.sleep,
    ) -> None:
        self.oidc_issuer = oidc_issuer.rstrip("/")
        self.client_id = client_id
        self.scope = scope
        self._request = request
        self._sleep = sleep

    async def discover(self) -> OidcDiscovery:
        status, payload = await self._request(
            "GET", f"{self.oidc_issuer}/.well-known/openid-configuration", None
        )
        if status != 200:
            raise OidcError(f"OIDC discovery failed (HTTP {status}).")
        return OidcDiscovery.from_response(payload)

    async def request_device_code(self, discovery: OidcDiscovery) -> DeviceCode:
        status, payload = await self._request(
            "POST",
            discovery.device_authorization_endpoint,
            {"client_id": self.client_id, "scope": self.scope},
        )
        if status != 200:
            raise OidcError(_error_description(payload, "Could not start device authorization."))
        return DeviceCode.from_response(payload)

    async def poll_for_tokens(
        self, discovery: OidcDiscovery, device_code: DeviceCode
    ) -> OAuthCredentials:
        deadline = time.monotonic() + device_code.expires_in
        interval = device_code.interval
        while time.monotonic() < deadline:
            status, payload = await self._request(
                "POST",
                discovery.token_endpoint,
                {
                    "grant_type": DEVICE_GRANT_TYPE,
                    "device_code": device_code.device_code,
                    "client_id": self.client_id,
                },
            )
            if status == 200:
                return _credentials_from_token_response(payload)

            error = payload.get("error")
            if error == "authorization_pending":
                await self._sleep(interval)
                continue
            if error == "slow_down":
                interval += 5
                await self._sleep(interval)
                continue
            if error in {"access_denied", "expired_token"}:
                raise OidcError(
                    _error_description(payload, "Device authorization was not completed.")
                )
            raise OidcError(_error_description(payload, f"Token request failed (HTTP {status})."))
        raise OidcError("Device authorization expired before it was completed.")

    async def refresh(
        self, discovery: OidcDiscovery, credentials: OAuthCredentials
    ) -> OAuthCredentials:
        if not credentials.refresh_token:
            raise OidcError("Your session cannot be refreshed. Run 'cua auth login' again.")
        status, payload = await self._request(
            "POST",
            discovery.token_endpoint,
            {
                "grant_type": "refresh_token",
                "refresh_token": credentials.refresh_token,
                "client_id": self.client_id,
            },
        )
        if status != 200:
            raise OidcError(
                _error_description(payload, "Your session expired. Run 'cua auth login' again.")
            )
        refreshed = _credentials_from_token_response(payload)
        if not refreshed.refresh_token:
            refreshed = OAuthCredentials(
                access_token=refreshed.access_token,
                refresh_token=credentials.refresh_token,
                expires_at=refreshed.expires_at,
                token_type=refreshed.token_type,
                scope=refreshed.scope,
            )
        return refreshed

    async def revoke(self, discovery: OidcDiscovery, credentials: OAuthCredentials) -> bool:
        if not discovery.revocation_endpoint:
            return False
        token = credentials.refresh_token or credentials.access_token
        status, _payload = await self._request(
            "POST",
            discovery.revocation_endpoint,
            {"token": token, "client_id": self.client_id},
        )
        return 200 <= status < 300


def _credentials_from_token_response(payload: Mapping[str, Any]) -> OAuthCredentials:
    try:
        expires_in = int(payload["expires_in"])
        access_token = str(payload["access_token"])
    except (KeyError, TypeError, ValueError) as error:
        raise OidcError("OIDC issuer returned an invalid token response.") from error
    return OAuthCredentials(
        access_token=access_token,
        refresh_token=str(payload["refresh_token"]) if payload.get("refresh_token") else None,
        expires_at=datetime.now(UTC) + timedelta(seconds=max(0, expires_in)),
        token_type=str(payload.get("token_type", "Bearer")),
        scope=str(payload["scope"]) if payload.get("scope") else None,
    )


def _error_description(payload: Mapping[str, Any], fallback: str) -> str:
    description = payload.get("error_description") or payload.get("error")
    return str(description) if description else fallback


async def get_access_token(*, force_refresh: bool = False) -> str:
    """Return a valid access token, refreshing and persisting it when needed."""
    credentials = load_credentials()
    if credentials is None:
        raise OidcError("Not logged in. Run 'cua auth login' first.")
    if not force_refresh and credentials.expires_at > datetime.now(UTC) + timedelta(seconds=60):
        return credentials.access_token

    client = OidcClient()
    refreshed = await client.refresh(await client.discover(), credentials)
    save_credentials(refreshed)
    return refreshed.access_token


def _fleet_access_token_provider_type() -> type[Any]:
    try:
        from cyclops_sdk import AccessTokenProvider
    except (ImportError, OSError) as error:
        raise OidcError(
            "Fleet integration requires cua-cli[fleet] with its generated SDK bindings."
        ) from error

    class FleetAccessTokenProvider(AccessTokenProvider):
        async def get_access_token(self, force_refresh: bool) -> str:
            return await get_access_token(force_refresh=force_refresh)

    return FleetAccessTokenProvider


def FleetAccessTokenProvider() -> Any:
    """Create a generated Fleet SDK token provider backed by CLI device credentials."""
    return _fleet_access_token_provider_type()()
