"""Login command using the CUA CLI's OIDC device authorization flow."""

import argparse
import asyncio
import json
import os
import time
import webbrowser
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import aiohttp

RESET = "\033[0m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"

DEFAULT_OIDC_ISSUER = "https://auth.cua.ai/realms/cyclops-cs"
DEFAULT_CLIENT_ID = "cua-cli"
DEFAULT_SCOPE = "openid profile offline_access"
DEVICE_GRANT_TYPE = "urn:ietf:params:oauth:grant-type:device_code"
CONFIG_DIR = Path.home() / ".config" / "cua-bench"
TOKEN_FILE = CONFIG_DIR / "token.json"


class OidcError(RuntimeError):
    """Raised for OIDC protocol and authorization failures."""


HttpRequest = Callable[[str, str, Mapping[str, str] | None], Awaitable[tuple[int, dict[str, Any]]]]
Sleep = Callable[[float], Awaitable[None]]


@dataclass(frozen=True)
class OidcDiscovery:
    """Endpoints published by OIDC discovery."""

    token_endpoint: str
    device_authorization_endpoint: str

    @classmethod
    def from_response(cls, value: Mapping[str, Any]) -> "OidcDiscovery":
        try:
            return cls(
                token_endpoint=str(value["token_endpoint"]),
                device_authorization_endpoint=str(value["device_authorization_endpoint"]),
            )
        except (KeyError, TypeError) as error:
            raise OidcError("OIDC issuer returned an incomplete discovery document.") from error


@dataclass(frozen=True)
class DeviceCode:
    """Response returned by the OIDC device authorization endpoint."""

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


@dataclass(frozen=True)
class OAuthCredentials:
    """The token material needed by CUA Bench commands."""

    access_token: str


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
    """Minimal public-client OIDC device authorization client."""

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
                try:
                    return OAuthCredentials(access_token=str(payload["access_token"]))
                except (KeyError, TypeError) as error:
                    raise OidcError("OIDC issuer returned an invalid token response.") from error

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


def _error_description(payload: Mapping[str, Any], fallback: str) -> str:
    return str(payload.get("error_description") or payload.get("error") or fallback)


def run_async(coroutine):
    """Run the OIDC request sequence from the synchronous CLI command."""
    return asyncio.run(coroutine)


def save_token(token: str, workspace_slug: str | None = None) -> None:
    """Save the authentication token in the established CUA Bench location."""
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)

    data = {"token": token}
    if workspace_slug:
        data["workspace_slug"] = workspace_slug

    with open(TOKEN_FILE, "w") as token_file:
        json.dump(data, token_file, indent=2)
    os.chmod(TOKEN_FILE, 0o600)


def load_token() -> dict | None:
    """Load the authentication token from the config file."""
    if not TOKEN_FILE.exists():
        return None

    try:
        with open(TOKEN_FILE) as token_file:
            return json.load(token_file)
    except (json.JSONDecodeError, OSError):
        return None


def execute(args: argparse.Namespace) -> int:
    """Authenticate with the CUA OIDC device authorization flow."""
    existing = load_token()
    if existing and not getattr(args, "force", False):
        print(f"{YELLOW}Already logged in.{RESET}")
        print(f"{GREY}Use {RESET}{CYAN}cb login --force{RESET}{GREY} to re-authenticate.{RESET}")
        return 0

    client = OidcClient()
    try:
        discovery = run_async(client.discover())
        device_code = run_async(client.request_device_code(discovery))
    except (OidcError, OSError) as error:
        print(f"{RED}Could not start device authorization: {error}{RESET}")
        return 1

    verification_url = device_code.verification_uri_complete or device_code.verification_uri
    print(f"{CYAN}Open this URL in any browser:{RESET} {verification_url}")
    print(f"{GREY}Enter this code if prompted:{RESET} {device_code.user_code}")
    if not getattr(args, "no_browser", False) and os.isatty(0) and os.isatty(1):
        try:
            if webbrowser.open(verification_url):
                print(f"{GREY}Opened the verification URL in your browser.{RESET}")
        except webbrowser.Error:
            print(f"{GREY}Could not open a browser. Use the URL shown above.{RESET}")
    elif not getattr(args, "no_browser", False):
        print(
            f"{GREY}No interactive terminal detected; use the URL shown above in a browser.{RESET}"
        )

    try:
        credentials = run_async(client.poll_for_tokens(discovery, device_code))
        save_token(credentials.access_token)
    except (OidcError, OSError) as error:
        print(f"{RED}Login failed: {error}{RESET}")
        return 1

    print(f"{GREEN}Successfully authenticated!{RESET}")
    print(f"{GREY}Token saved to: {TOKEN_FILE}{RESET}")
    return 0
