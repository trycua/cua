import asyncio
from urllib.parse import parse_qs, urlparse

import aiohttp
import pytest
from cua_cli.auth import github_wif
from cua_cli.auth.github_wif import (
    GitHubWifError,
    _aiohttp_json_get,
    request_github_wif_token,
)


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


class FakeAiohttpResponse:
    def __init__(self, *, payload: object = None, json_error: Exception | None = None) -> None:
        self.status = 200
        self.payload = payload
        self.json_error = json_error

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_args) -> None:
        return None

    async def json(self, *, content_type: object = None) -> object:
        if self.json_error is not None:
            raise self.json_error
        return self.payload


class FakeAiohttpSession:
    def __init__(self, response: FakeAiohttpResponse | None, get_error: Exception | None) -> None:
        self.response = response
        self.get_error = get_error

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_args) -> None:
        return None

    def get(self, _url: str, *, headers: dict[str, str]) -> FakeAiohttpResponse:
        if self.get_error is not None:
            raise self.get_error
        assert self.response is not None
        return self.response


def mock_aiohttp_session(
    monkeypatch,
    *,
    payload: object = None,
    json_error: Exception | None = None,
    get_error: Exception | None = None,
) -> None:
    response = (
        None
        if get_error is not None
        else FakeAiohttpResponse(payload=payload, json_error=json_error)
    )
    monkeypatch.setattr(
        github_wif.aiohttp,
        "ClientSession",
        lambda **_kwargs: FakeAiohttpSession(response, get_error),
    )


def assert_sanitized_error(error: GitHubWifError, message: str) -> None:
    rendered = str(error)
    assert rendered == message
    assert "request-secret" not in rendered
    assert "response-secret" not in rendered


def test_aiohttp_json_get_rejects_invalid_json_without_leaking_response_body(monkeypatch) -> None:
    mock_aiohttp_session(monkeypatch, json_error=ValueError("response-secret"))

    with pytest.raises(GitHubWifError) as error:
        run(
            _aiohttp_json_get(
                "https://actions.example.test/oidc?token=request-secret",
                {"Authorization": "bearer request-secret"},
            )
        )

    assert_sanitized_error(error.value, "GitHub OIDC endpoint returned invalid JSON.")


def test_aiohttp_json_get_rejects_non_object_json_without_leaking_response_body(
    monkeypatch,
) -> None:
    mock_aiohttp_session(monkeypatch, payload=["response-secret"])

    with pytest.raises(GitHubWifError) as error:
        run(
            _aiohttp_json_get(
                "https://actions.example.test/oidc?token=request-secret",
                {"Authorization": "bearer request-secret"},
            )
        )

    assert_sanitized_error(error.value, "GitHub OIDC endpoint returned an invalid response.")


@pytest.mark.parametrize("request_error", [TimeoutError(), aiohttp.ClientConnectionError()])
def test_aiohttp_json_get_sanitizes_timeout_and_client_errors(monkeypatch, request_error) -> None:
    mock_aiohttp_session(monkeypatch, get_error=request_error)

    with pytest.raises(GitHubWifError) as error:
        run(
            _aiohttp_json_get(
                "https://actions.example.test/oidc?token=request-secret",
                {"Authorization": "bearer request-secret"},
            )
        )

    assert_sanitized_error(error.value, "GitHub OIDC request failed.")
