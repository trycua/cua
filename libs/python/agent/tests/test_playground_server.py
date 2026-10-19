"""Security tests for PlaygroundServer.

The playground server has no authentication, so it must only be reachable from
the local machine, only answer local browser origins, never write request data
into os.environ, and never let a request redirect the agent's model traffic.
"""

import asyncio
import os
from unittest.mock import patch

import pytest

pytest.importorskip("fastapi")
pytest.importorskip("httpx")

from cua_agent.playground import server as playground_server  # noqa: E402
from cua_agent.playground.server import PlaygroundServer  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

EVIL = "https://evil.example"


class FakeAgent:
    """Stand-in for ComputerAgent that records how it was built and what it saw."""

    instances = []

    def __init__(self, model, **kwargs):
        self.model = model
        self.kwargs = kwargs
        self.environ_during_run = None
        FakeAgent.instances.append(self)

    async def run(self, messages):
        self.environ_during_run = dict(os.environ)
        yield {
            "output": [
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "ok"}],
                }
            ],
            "usage": {"total_tokens": 2},
        }


@pytest.fixture(autouse=True)
def fake_agent():
    FakeAgent.instances = []
    with patch("cua_agent.ComputerAgent", FakeAgent):
        yield FakeAgent


def make_client(**server_kwargs) -> TestClient:
    return TestClient(PlaygroundServer(**server_kwargs).app, base_url="http://localhost")


def post_responses(client, body, **headers):
    return client.post("/responses", json=body, headers=headers)


class TestBinding:
    @pytest.mark.parametrize(
        "kwargs, expected_host",
        [({}, "127.0.0.1"), ({"host": "0.0.0.0"}, "0.0.0.0")],
    )
    def test_start_async_binds_loopback_unless_host_given(self, kwargs, expected_host):
        captured = {}

        class FakeServer:
            def __init__(self, config):
                captured["host"] = config.host

            async def serve(self):
                return None

        with patch.object(playground_server.uvicorn, "Server", FakeServer):
            asyncio.run(PlaygroundServer().start_async(port=18123, **kwargs))

        assert captured["host"] == expected_host

    def test_agent_open_defaults_to_loopback(self):
        from cua_agent import agent as agent_module

        seen = {}

        class FakeServer:
            def __init__(self, agent_instance=None):
                pass

            def start(self, port=None, open_browser=False, host=None):
                seen["host"] = host

        with patch("cua_agent.playground.PlaygroundServer", FakeServer):
            agent_module.ComputerAgent.open(object.__new__(agent_module.ComputerAgent), port=1)

        assert seen["host"] == "127.0.0.1"


class TestCors:
    def preflight(self, client, origin):
        return client.options(
            "/responses",
            headers={
                "Origin": origin,
                "Access-Control-Request-Method": "POST",
                "Access-Control-Request-Headers": "content-type",
            },
        )

    def test_foreign_origin_preflight_is_not_allowed(self):
        resp = self.preflight(make_client(), EVIL)
        assert "access-control-allow-origin" not in resp.headers
        assert "access-control-allow-credentials" not in resp.headers

    @pytest.mark.parametrize(
        "origin",
        [
            "http://localhost:3000",
            "http://localhost",
            "http://127.0.0.1:5173",
            "https://localhost:8443",
            "http://[::1]:3000",
        ],
    )
    def test_loopback_origins_are_allowed_without_credentials(self, origin):
        resp = self.preflight(make_client(), origin)
        assert resp.status_code == 200
        assert resp.headers["access-control-allow-origin"] == origin
        assert "access-control-allow-credentials" not in resp.headers

    @pytest.mark.parametrize(
        "origin",
        [
            "null",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "http://evil.example:3000",
            "http://localhost:3000.evil.example",
            "file://",
        ],
    )
    def test_lookalike_origins_are_rejected(self, origin):
        resp = self.preflight(make_client(), origin)
        assert "access-control-allow-origin" not in resp.headers

    def test_explicit_extra_origin_can_be_allowed(self):
        client = make_client(allowed_origins=["https://dash.example.com/"])
        resp = self.preflight(client, "https://dash.example.com")
        assert resp.headers["access-control-allow-origin"] == "https://dash.example.com"
        assert "access-control-allow-credentials" not in resp.headers
        assert "access-control-allow-origin" not in self.preflight(client, EVIL).headers

    def test_wildcard_origin_is_refused(self):
        with pytest.raises(ValueError):
            PlaygroundServer(allowed_origins=["*"])


class TestOriginGuard:
    BODY = {"model": "m", "input": "hi"}

    def test_cross_origin_post_is_rejected_and_never_runs_the_agent(self, fake_agent):
        # A text/plain body is a CORS "simple request": no preflight happens, so the
        # server itself must refuse it.
        resp = make_client().post(
            "/responses",
            content='{"model": "m", "input": "hi"}',
            headers={"Origin": EVIL, "Content-Type": "text/plain"},
        )
        assert resp.status_code == 403
        assert fake_agent.instances == []

    def test_null_origin_post_is_rejected(self, fake_agent):
        resp = post_responses(make_client(), self.BODY, Origin="null")
        assert resp.status_code == 403
        assert fake_agent.instances == []

    def test_get_from_foreign_origin_is_rejected(self):
        assert make_client().get("/status", headers={"Origin": EVIL}).status_code == 403

    def test_loopback_origin_post_works(self):
        resp = post_responses(make_client(), self.BODY, Origin="http://localhost:3000")
        assert resp.status_code == 200
        assert resp.json()["status"] == "completed"
        assert resp.headers["access-control-allow-origin"] == "http://localhost:3000"

    def test_request_without_origin_works(self):
        # curl, scripts and other non-browser local clients send no Origin.
        resp = post_responses(make_client(), self.BODY)
        assert resp.status_code == 200
        assert make_client().get("/status").status_code == 200

    def test_extra_allowed_origin_post_works(self):
        client = make_client(allowed_origins=["https://dash.example.com"])
        assert (
            post_responses(client, self.BODY, Origin="https://dash.example.com").status_code == 200
        )


class TestEnvOverride:
    def test_request_env_never_reaches_the_process_environment(self, fake_agent, monkeypatch):
        monkeypatch.setenv("OPENAI_API_KEY", "dev-key")
        monkeypatch.delenv("OPENAI_API_BASE", raising=False)
        before = dict(os.environ)

        resp = post_responses(
            make_client(),
            {
                "model": "m",
                "input": "hi",
                "env": {
                    "OPENAI_API_BASE": "http://127.0.0.1:9999/v1",
                    "OPENAI_API_KEY": "from-request",
                    "PATH": "/tmp/evil",
                },
            },
        )

        assert resp.status_code == 200
        agent = fake_agent.instances[0]
        assert "OPENAI_API_BASE" not in agent.environ_during_run
        assert agent.environ_during_run["OPENAI_API_KEY"] == "dev-key"
        assert agent.environ_during_run["PATH"] == before["PATH"]
        assert dict(os.environ) == before
        assert resp.json()["ignored_request_fields"] == ["env"]

    def test_env_is_ignored_even_with_request_overrides_enabled(self, fake_agent, monkeypatch):
        monkeypatch.delenv("OPENAI_API_BASE", raising=False)
        client = make_client(allow_request_overrides=True)
        post_responses(
            client,
            {"model": "m", "input": "hi", "env": {"OPENAI_API_BASE": "http://127.0.0.1:9999/v1"}},
        )
        assert "OPENAI_API_BASE" not in fake_agent.instances[0].environ_during_run
        assert "OPENAI_API_BASE" not in os.environ

    def test_empty_env_from_older_clients_is_fine(self):
        resp = post_responses(make_client(), {"model": "m", "input": "hi", "env": {}})
        assert resp.status_code == 200
        assert "ignored_request_fields" not in resp.json()


class TestAgentKwargs:
    def run(self, agent_kwargs, **server_kwargs):
        resp = post_responses(
            make_client(**server_kwargs),
            {"model": "m", "input": "hi", "agent_kwargs": agent_kwargs},
        )
        assert resp.status_code == 200
        return FakeAgent.instances[-1].kwargs, resp.json()

    @pytest.mark.parametrize(
        "key",
        ["api_base", "base_url", "api_key", "api_version", "extra_headers", "custom_llm_provider"],
    )
    def test_endpoint_and_credential_kwargs_are_dropped(self, key):
        kwargs, payload = self.run({key: "http://127.0.0.1:9999/v1"})
        assert key not in kwargs
        assert payload["ignored_request_fields"] == [f"agent_kwargs.{key}"]

    def test_safe_tuning_kwargs_still_pass_through(self):
        # These are the options the playground UI sends.
        kwargs, payload = self.run({"use_prompt_caching": False, "only_n_most_recent_images": 3})
        assert kwargs == {"use_prompt_caching": False, "only_n_most_recent_images": 3}
        assert "ignored_request_fields" not in payload

    def test_mixed_kwargs_keep_only_the_safe_ones(self):
        kwargs, _ = self.run(
            {"only_n_most_recent_images": 3, "api_base": "http://x", "api_key": "k"}
        )
        assert kwargs == {"only_n_most_recent_images": 3}

    def test_opt_in_allows_endpoint_overrides(self):
        kwargs, payload = self.run(
            {"api_base": "http://127.0.0.1:9999/v1", "api_key": "k"}, allow_request_overrides=True
        )
        assert kwargs == {"api_base": "http://127.0.0.1:9999/v1", "api_key": "k"}
        assert "ignored_request_fields" not in payload

    @pytest.mark.parametrize(
        "key", ["trust_remote_code", "trajectory_dir", "tools", "callbacks", "custom_loop"]
    )
    def test_code_and_disk_kwargs_are_dropped_even_with_opt_in(self, key):
        kwargs, _ = self.run({key: True}, allow_request_overrides=True)
        assert key not in kwargs

    @pytest.mark.parametrize("bad", [["api_base"], "api_base", 5])
    def test_non_object_agent_kwargs_is_a_400(self, bad):
        resp = post_responses(make_client(), {"model": "m", "input": "hi", "agent_kwargs": bad})
        assert resp.status_code == 400

    def test_non_object_body_is_a_400(self):
        assert make_client().post("/responses", json=["x"]).status_code == 400
