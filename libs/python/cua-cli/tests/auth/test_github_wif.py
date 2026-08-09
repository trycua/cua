import asyncio
from urllib.parse import parse_qs, urlparse

import pytest
from cua_cli.auth.github_wif import GitHubWifError, request_github_wif_token


def run(coroutine):
    return asyncio.run(coroutine)


def github_environment() -> dict[str, str]:
    return {
        "ACTIONS_ID_TOKEN_REQUEST_URL": (
            "https://actions.example.test/oidc?api-version=2.0&audience=old"
        ),
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "request-secret",
    }


def test_requests_fleets_audience_and_returns_value() -> None:
    captured: dict[str, object] = {}

    async def request(url: str, headers: dict[str, str]):
        captured["url"] = url
        captured["headers"] = headers
        return 200, {"value": "signed-jwt"}

    token = run(request_github_wif_token(environ=github_environment(), request=request))

    query = parse_qs(urlparse(str(captured["url"])).query)
    assert query == {"api-version": ["2.0"], "audience": ["fleets"]}
    assert captured["headers"] == {
        "Accept": "application/json",
        "Authorization": "bearer request-secret",
    }
    assert token == "signed-jwt"


@pytest.mark.parametrize(
    ("missing", "message"),
    [
        ("ACTIONS_ID_TOKEN_REQUEST_URL", "ACTIONS_ID_TOKEN_REQUEST_URL"),
        ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "ACTIONS_ID_TOKEN_REQUEST_TOKEN"),
    ],
)
def test_requires_github_actions_oidc_environment(missing: str, message: str) -> None:
    environment = github_environment()
    del environment[missing]

    with pytest.raises(GitHubWifError, match=message):
        run(request_github_wif_token(environ=environment))


def test_rejects_non_success_without_exposing_secrets() -> None:
    async def request(_url: str, _headers: dict[str, str]):
        return 403, {"message": "denied", "value": "response-secret"}

    with pytest.raises(GitHubWifError) as error:
        run(request_github_wif_token(environ=github_environment(), request=request))

    message = str(error.value)
    assert "HTTP 403" in message
    assert "request-secret" not in message
    assert "response-secret" not in message


@pytest.mark.parametrize("payload", [{}, {"value": ""}, {"value": 123}])
def test_requires_non_empty_string_value(payload: dict[str, object]) -> None:
    async def request(_url: str, _headers: dict[str, str]):
        return 200, payload

    with pytest.raises(GitHubWifError, match="non-empty token"):
        run(request_github_wif_token(environ=github_environment(), request=request))
