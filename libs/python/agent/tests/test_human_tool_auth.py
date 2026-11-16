"""The human tool listens on loopback and requires a bearer token on every API route."""

import importlib
import os
import stat
from unittest.mock import MagicMock

import pytest

pytest.importorskip("fastapi")
pytest.importorskip("httpx")

from cua_agent import human_auth  # noqa: E402
from cua_agent.human_tool import server  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

TOKEN = "test-token-for-human-tool"
QUEUE_BODY = {"messages": [{"role": "user", "content": "delete it?"}], "model": "human"}


@pytest.fixture(autouse=True)
def isolated_auth(tmp_path, monkeypatch):
    """Keep token files in a temp dir and start every test with no cached token."""
    monkeypatch.setenv("CUA_HOME", str(tmp_path))
    monkeypatch.delenv(human_auth.TOKEN_ENV, raising=False)
    monkeypatch.delenv(human_auth.HOST_ENV, raising=False)
    monkeypatch.setattr(human_auth, "_generated_token", None)


@pytest.fixture
def client(monkeypatch):
    monkeypatch.setenv(human_auth.TOKEN_ENV, TOKEN)
    return TestClient(server.app)


def bearer(token=TOKEN):
    return {"Authorization": f"Bearer {token}"}


# ---- every API route rejects requests without the token ----------------------


@pytest.mark.parametrize(
    "method, path, body",
    [
        ("post", "/queue", QUEUE_BODY),
        ("get", "/pending", None),
        ("get", "/status/some-id", None),
        ("post", "/complete/some-id", {"response": "Yes, delete it"}),
        ("post", "/fail/some-id", {"error": "x"}),
    ],
)
class TestRoutesRequireToken:
    def test_no_token_is_401(self, client, method, path, body):
        resp = getattr(client, method)(path, **({"json": body} if body is not None else {}))
        assert resp.status_code == 401

    def test_wrong_token_is_401(self, client, method, path, body):
        kwargs = {"headers": bearer("not-the-token")}
        if body is not None:
            kwargs["json"] = body
        assert getattr(client, method)(path, **kwargs).status_code == 401

    def test_wrong_scheme_is_401(self, client, method, path, body):
        kwargs = {"headers": {"Authorization": f"Basic {TOKEN}"}}
        if body is not None:
            kwargs["json"] = body
        assert getattr(client, method)(path, **kwargs).status_code == 401


def test_empty_bearer_is_401(client):
    assert client.get("/pending", headers={"Authorization": "Bearer "}).status_code == 401


def test_root_does_not_need_token(client):
    assert client.get("/").status_code == 200


# ---- with the token the normal flow works ------------------------------------


def test_full_flow_with_token(client):
    queued = client.post("/queue", json=QUEUE_BODY, headers=bearer())
    assert queued.status_code == 200
    call_id = queued.json()["id"]

    pending = client.get("/pending", headers=bearer()).json()["pending_calls"]
    assert call_id in [c["id"] for c in pending]

    done = client.post(f"/complete/{call_id}", json={"response": "no"}, headers=bearer())
    assert done.status_code == 200

    status = client.get(f"/status/{call_id}", headers=bearer()).json()
    assert status["status"] == "completed"
    assert status["response"] == "no"


def test_completion_without_token_leaves_call_pending(client):
    call_id = client.post("/queue", json=QUEUE_BODY, headers=bearer()).json()["id"]

    resp = client.post(f"/complete/{call_id}", json={"response": "Yes, delete it"})
    assert resp.status_code == 401

    status = client.get(f"/status/{call_id}", headers=bearer()).json()
    assert status["status"] == "pending"
    assert "response" not in status


# ---- generated token ---------------------------------------------------------


def test_generated_token_is_saved_private_and_accepted(tmp_path):
    token = human_auth.server_token()
    path = human_auth.token_file()

    assert path == tmp_path / "human_tool" / "token"
    assert path.read_text() == token
    assert len(token) >= 32
    if os.name == "posix":
        assert stat.S_IMODE(path.stat().st_mode) == 0o600

    api = TestClient(server.app)
    assert api.get("/pending").status_code == 401
    assert api.get("/pending", headers=bearer(token)).status_code == 200


def test_generated_token_differs_per_launch(monkeypatch):
    first = human_auth.server_token()
    monkeypatch.setattr(human_auth, "_generated_token", None)  # simulate a restart
    second = human_auth.server_token()

    assert first != second
    assert human_auth.token_file().read_text() == second


def test_env_token_wins_and_is_not_written(monkeypatch):
    monkeypatch.setenv(human_auth.TOKEN_ENV, TOKEN)
    assert human_auth.server_token() == TOKEN
    assert not human_auth.token_file().exists()


def test_client_token_order(monkeypatch):
    assert human_auth.client_token() is None  # nothing configured

    token = human_auth.server_token()  # writes the file
    assert human_auth.client_token() == token

    monkeypatch.setenv(human_auth.TOKEN_ENV, "from-env")
    assert human_auth.client_token() == "from-env"
    assert human_auth.client_token("explicit") == "explicit"


def test_token_compare_is_constant_time(monkeypatch):
    calls = []
    real = human_auth.hmac.compare_digest

    def spy(a, b):
        calls.append((a, b))
        return real(a, b)

    monkeypatch.setattr(human_auth.hmac, "compare_digest", spy)

    assert human_auth.token_matches("abc", "abc")
    assert not human_auth.token_matches("abd", "abc")
    assert not human_auth.token_matches(None, "abc")
    assert len(calls) == 2


# ---- default bind address ----------------------------------------------------


def test_default_host_is_loopback(monkeypatch):
    assert human_auth.resolve_host() == "127.0.0.1"
    assert human_auth.is_loopback(human_auth.resolve_host())


def test_host_can_be_exposed_only_explicitly(monkeypatch):
    assert human_auth.resolve_host("0.0.0.0") == "0.0.0.0"
    monkeypatch.setenv(human_auth.HOST_ENV, "0.0.0.0")
    assert human_auth.resolve_host() == "0.0.0.0"
    assert not human_auth.is_loopback("0.0.0.0")


def test_server_main_binds_loopback_by_default(monkeypatch):
    uvicorn = pytest.importorskip("uvicorn")
    run = MagicMock()
    monkeypatch.setattr(uvicorn, "run", run)

    server.main([])
    assert run.call_args.kwargs["host"] == "127.0.0.1"

    server.main(["--host", "0.0.0.0"])
    assert run.call_args.kwargs["host"] == "0.0.0.0"


# ---- clients send the token --------------------------------------------------


def _route_requests_to(client, monkeypatch, module):
    """Make ``module.requests.get/post`` call the in-process app instead of the network."""
    seen = []

    def fake(method):
        def call(url, json=None, headers=None, timeout=None):
            path = "/" + url.split("//", 1)[1].split("/", 1)[1]
            seen.append((method, path, dict(headers or {})))
            return getattr(client, method)(path, json=json, headers=headers)

        return call

    fake_requests = MagicMock()
    fake_requests.get = fake("get")
    fake_requests.post = fake("post")
    fake_requests.RequestException = Exception
    monkeypatch.setattr(module, "requests", fake_requests)
    return seen


def test_adapter_authenticates_with_the_token_file(monkeypatch):
    from cua_agent.adapters import human_adapter

    api = TestClient(server.app)
    human_auth.server_token()  # server side: generated token written to the file
    seen = _route_requests_to(api, monkeypatch, human_adapter)

    adapter = human_adapter.HumanAdapter()  # no token passed, no env var set
    call_id = adapter._queue_completion(QUEUE_BODY["messages"], "human")

    assert call_id
    assert seen[0][2]["Authorization"] == f"Bearer {human_auth.client_token()}"


def test_adapter_token_argument_and_env(client, monkeypatch):
    from cua_agent.adapters import human_adapter

    seen = _route_requests_to(client, monkeypatch, human_adapter)
    assert human_adapter.HumanAdapter()._queue_completion([], "human")  # env var
    assert human_adapter.HumanAdapter(token=TOKEN)._queue_completion([], "human")  # argument
    assert [s[2]["Authorization"] for s in seen] == [f"Bearer {TOKEN}"] * 2


def test_adapter_reports_rejected_token(client, monkeypatch):
    from cua_agent.adapters import human_adapter

    _route_requests_to(client, monkeypatch, human_adapter)
    with pytest.raises(Exception, match="rejected the token"):
        human_adapter.HumanAdapter(token="wrong")._queue_completion([], "human")


def test_adapter_async_poll_sends_token(client, monkeypatch):
    import asyncio

    from cua_agent.adapters import human_adapter

    adapter = human_adapter.HumanAdapter(token=TOKEN)
    assert adapter._headers() == bearer()

    # the async poller builds its aiohttp session from the same headers
    captured = {}

    class FakeSession:
        def __init__(self, headers=None):
            captured["headers"] = headers

        async def __aenter__(self):
            return self

        async def __aexit__(self, *exc):
            return False

        def get(self, url):
            class Resp:
                async def __aenter__(s):
                    return s

                async def __aexit__(s, *exc):
                    return False

                def raise_for_status(s):
                    pass

                async def json(s):
                    return {"status": "completed", "response": "ok"}

            return Resp()

    import aiohttp

    monkeypatch.setattr(aiohttp, "ClientSession", FakeSession)
    result = asyncio.run(adapter._async_wait_for_completion("some-id"))
    assert result == {"response": "ok"}
    assert captured["headers"] == bearer()


def test_ui_authenticates_with_the_token_file(monkeypatch):
    pytest.importorskip("gradio")
    from cua_agent.human_tool import ui

    api = TestClient(server.app)
    human_auth.server_token()
    seen = _route_requests_to(api, monkeypatch, ui)

    handler = ui.HumanCompletionUI()
    assert handler.get_pending_calls() == [] or isinstance(handler.get_pending_calls(), list)
    assert seen[0][1] == "/pending"
    assert seen[0][2]["Authorization"] == f"Bearer {human_auth.client_token()}"

    call_id = api.post("/queue", json=QUEUE_BODY, headers=bearer(human_auth.client_token())).json()[
        "id"
    ]
    assert handler.complete_call_with_response(call_id, "approved") is True
    assert seen[-1][2]["Authorization"] == f"Bearer {human_auth.client_token()}"


def test_ui_main_binds_loopback_and_asks_for_password(monkeypatch):
    pytest.importorskip("gradio")
    from cua_agent.human_tool import ui

    demo = MagicMock()
    monkeypatch.setattr(ui, "create_ui", lambda *a, **k: demo)

    ui.main([])
    kwargs = demo.launch.call_args.kwargs
    assert kwargs["server_name"] == "127.0.0.1"
    # the page's login only lets in whoever knows the token
    token = human_auth.server_token()
    assert kwargs["auth"]("anyone", token) is True
    assert kwargs["auth"]("anyone", "wrong") is False


# ---- combined server + UI ----------------------------------------------------


def test_combined_app_binds_loopback_and_protects_the_page(monkeypatch):
    pytest.importorskip("gradio")
    uvicorn = pytest.importorskip("uvicorn")
    combined = importlib.import_module("cua_agent.human_tool.__main__")

    run = MagicMock()
    monkeypatch.setattr(uvicorn, "run", run)
    combined.main([])
    assert run.call_args.kwargs["host"] == "127.0.0.1"

    token = human_auth.server_token()
    api = TestClient(combined.app)

    # API routes still need the bearer token ...
    assert api.get("/pending").status_code == 401
    assert api.get("/pending", headers=bearer(token)).status_code == 200

    # ... and the web page needs the token as its password
    assert api.post("/gradio/login", data={"username": "u", "password": "wrong"}).status_code in (
        400,
        401,
    )
    assert api.post("/gradio/login", data={"username": "u", "password": token}).status_code == 200
