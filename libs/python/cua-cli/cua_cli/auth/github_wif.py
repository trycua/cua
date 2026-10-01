"""GitHub Actions workload identity token retrieval."""

import os
from collections.abc import Awaitable, Callable, Mapping
from typing import Any
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

import aiohttp

DEFAULT_GITHUB_WIF_AUDIENCE = "fleets"


class GitHubWifError(RuntimeError):
    """Raised when GitHub Actions cannot issue a workload identity token."""


HttpRequest = Callable[[str, dict[str, str]], Awaitable[tuple[int, Mapping[str, Any]]]]


async def _aiohttp_json_get(url: str, headers: dict[str, str]) -> tuple[int, Mapping[str, Any]]:
    timeout = aiohttp.ClientTimeout(total=15)
    try:
        async with aiohttp.ClientSession(timeout=timeout) as session:
            async with session.get(url, headers=headers) as response:
                try:
                    payload = await response.json(content_type=None)
                except (aiohttp.ClientError, ValueError) as error:
                    raise GitHubWifError("GitHub OIDC endpoint returned invalid JSON.") from error
    except GitHubWifError:
        raise
    except (aiohttp.ClientError, TimeoutError) as error:
        raise GitHubWifError("GitHub OIDC request failed.") from error
    if not isinstance(payload, Mapping):
        raise GitHubWifError("GitHub OIDC endpoint returned an invalid response.")
    return response.status, payload


def _with_audience(request_url: str, audience: str) -> str:
    parts = urlsplit(request_url)
    query = [
        (key, value)
        for key, value in parse_qsl(parts.query, keep_blank_values=True)
        if key != "audience"
    ]
    query.append(("audience", audience))
    return urlunsplit((parts.scheme, parts.netloc, parts.path, urlencode(query), parts.fragment))


async def request_github_wif_token(
    *,
    audience: str = DEFAULT_GITHUB_WIF_AUDIENCE,
    environ: Mapping[str, str] | None = None,
    request: HttpRequest = _aiohttp_json_get,
) -> str:
    environment = os.environ if environ is None else environ
    request_url = environment.get("ACTIONS_ID_TOKEN_REQUEST_URL", "")
    request_token = environment.get("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "")
    if not request_url:
        raise GitHubWifError(
            "ACTIONS_ID_TOKEN_REQUEST_URL is missing; run in GitHub Actions with "
            "permissions: id-token: write."
        )
    if not request_token:
        raise GitHubWifError(
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN is missing; run in GitHub Actions with "
            "permissions: id-token: write."
        )

    status, payload = await request(
        _with_audience(request_url, audience),
        {
            "Accept": "application/json",
            "Authorization": f"bearer {request_token}",
        },
    )
    if status != 200:
        raise GitHubWifError(f"GitHub OIDC request failed with HTTP {status}.")
    value = payload.get("value")
    if not isinstance(value, str) or not value:
        raise GitHubWifError("GitHub OIDC response did not contain a non-empty token.")
    return value
