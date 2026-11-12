"""HarnessAgent against a fake cua agents API (hermetic: no SDK, no sandbox)."""

from __future__ import annotations

import json
import sys
import types
from dataclasses import dataclass, field

import pytest
from cua_bench.agents import FailureMode, get_agent
from cua_bench.agents.harness_agent import (
    HarnessAgent,
    failure_mode,
    parse_mcp,
    usage_tokens,
)


class _Record:
    def __init__(self, **kw):
        self.__dict__.update(kw)


class Timeout(Exception):
    pass


@dataclass
class Event:
    json: str


@dataclass
class Page:
    events: list
    cursor: int
    caught_up: bool


@dataclass
class Result:
    status: str = "idle"
    turn: int = 1
    text: str = "done"
    stop_reason: str | None = "end_turn"
    usage_json: str | None = '{"inputTokens": 120, "outputTokens": 7, "totalTokens": 127}'
    error: str | None = None
    tool_calls: int = 2


@dataclass
class FakeRun:
    result_: Result = field(default_factory=Result)
    hang: bool = False
    calls: list = field(default_factory=list)

    def run_id(self):
        return "run-0000beef"

    async def wait(self, timeout_ms):
        self.calls.append(("wait", timeout_ms))
        if self.hang:
            raise Timeout("run-0000beef is still working after 1s")
        return self.result_

    async def result(self):
        return self.result_

    async def interrupt(self):
        self.calls.append(("interrupt",))

    async def stop(self):
        self.calls.append(("stop",))

    async def events(self, cursor, max):
        evs = [Event('{"kind":"turn_started"}'), Event('{"kind":"turn_ended"}')]
        return Page(evs if cursor == 0 else [], cursor + 2, True)


class FakeAgents:
    def __init__(self, run: FakeRun, fail: Exception | None = None):
        self.run_ = run
        self.fail = fail
        self.started = []

    async def run(self, harness, prompt, options):
        if self.fail:
            raise self.fail
        self.started.append((harness, prompt, options))
        return self.run_


class FakeGuest:
    def __init__(self, agents):
        self._agents = agents

    async def agents(self):
        return self._agents


class FakeSandbox:
    def __init__(self, agents):
        self._agents = agents

    async def spacesd(self):
        return FakeGuest(self._agents)


class FakeSession:
    def __init__(self, agents):
        self.sandbox = FakeSandbox(agents)


@pytest.fixture(autouse=True)
def fake_cua(monkeypatch):
    """A stand-in `cua` module: the options records and `embedded().spacesd`."""
    mod = types.ModuleType("cua")
    mod.AgentRunOptions = _Record
    mod.AgentRunMcpServer = _Record
    mod.guests = []

    class _Cua:
        async def spacesd(self, url, token):
            mod.guests.append((url, token))
            return FakeGuest(mod.direct_agents)

    mod.embedded = lambda: _Cua()
    monkeypatch.setitem(sys.modules, "cua", mod)
    for k in list(__import__("os").environ):
        if k.startswith("CUA_BENCH_HARNESS") or k.startswith("CUA_BENCH_SPACESD"):
            monkeypatch.delenv(k)
    return mod


def test_registered_as_harness():
    assert get_agent("harness") is HarnessAgent
    assert HarnessAgent.name() == "harness"


async def test_runs_the_task_in_the_sessions_sandbox(tmp_path):
    run = FakeRun()
    agents = FakeAgents(run)
    agent = HarnessAgent(
        harness="openai-codex",
        env_from_host="OPENAI_API_KEY",
        model="gpt-x",
        mcp=["docs=https://mcp.example.com/mcp"],
        timeout_s=90,
    )
    r = await agent.perform_task("Open the settings app", FakeSession(agents), logging_dir=tmp_path)
    assert (r.total_input_tokens, r.total_output_tokens, r.failure_mode) == (120, 7, FailureMode.NONE)
    harness, prompt, opts = agents.started[0]
    assert (harness, prompt) == ("openai-codex", "Open the settings app")
    assert opts.env_from_host == ["OPENAI_API_KEY"] and opts.model == "gpt-x"
    assert [(m.name, m.url) for m in opts.mcp_servers] == [("docs", "https://mcp.example.com/mcp")]
    assert run.calls == [("wait", 90_000), ("stop",)]
    events = (tmp_path / "harness-events.jsonl").read_text().splitlines()
    assert [json.loads(e)["kind"] for e in events] == ["turn_started", "turn_ended"]
    saved = json.loads((tmp_path / "harness-result.json").read_text())
    assert saved["run_id"] == "run-0000beef" and saved["usage"]["inputTokens"] == 120


async def test_settings_from_the_environment(monkeypatch):
    monkeypatch.setenv("CUA_BENCH_HARNESS", "gemini-cli")
    monkeypatch.setenv("CUA_BENCH_HARNESS_KEYS", "GEMINI_API_KEY, GOOGLE_API_KEY")
    monkeypatch.setenv("CUA_BENCH_HARNESS_BASE_URL", "http://mock-llm:8787")
    monkeypatch.setenv("CUA_BENCH_HARNESS_TIMEOUT", "5")
    agents = FakeAgents(FakeRun())
    agent = HarnessAgent()
    await agent.perform_task("t", FakeSession(agents))
    harness, _, opts = agents.started[0]
    assert harness == "gemini-cli"
    assert opts.env_from_host == ["GEMINI_API_KEY", "GOOGLE_API_KEY"]
    assert opts.base_url == "http://mock-llm:8787"
    assert agents.run_.calls[0] == ("wait", 5000)


async def test_a_direct_spacesd(fake_cua):
    fake_cua.direct_agents = FakeAgents(FakeRun())
    agent = HarnessAgent(spacesd_url="http://10.0.0.5:3211", spacesd_token="t")
    r = await agent.perform_task("t", session=object())
    assert r.failure_mode == FailureMode.NONE
    assert fake_cua.guests == [("http://10.0.0.5:3211", "t")]


async def test_a_session_without_spacesd_is_an_api_error():
    r = await HarnessAgent().perform_task("t", session=object())
    assert r.failure_mode == FailureMode.API_ERROR


async def test_a_start_failure_is_an_api_error():
    agents = FakeAgents(FakeRun(), fail=RuntimeError("ANTHROPIC_API_KEY is not set"))
    r = await HarnessAgent().perform_task("t", FakeSession(agents))
    assert r.failure_mode == FailureMode.API_ERROR


async def test_a_timeout_interrupts_and_counts_as_over_budget():
    run = FakeRun(hang=True, result_=Result(stop_reason=None, usage_json=None))
    r = await HarnessAgent(timeout_s=1).perform_task("t", FakeSession(FakeAgents(run)))
    assert r.failure_mode == FailureMode.MAX_STEPS_EXCEEDED
    assert run.calls == [("wait", 1000), ("interrupt",), ("stop",)]


def test_failure_modes():
    assert failure_mode("idle", "end_turn", None) == FailureMode.NONE
    assert failure_mode("failed", None, "401 from provider") == FailureMode.API_ERROR
    assert failure_mode("crashed", None, None) == FailureMode.UNKNOWN
    assert failure_mode("idle", "max_tokens", None) == FailureMode.MAX_STEPS_EXCEEDED
    assert failure_mode("idle", "cancelled", None) == FailureMode.UNKNOWN


def test_helpers():
    assert usage_tokens('{"inputTokens": 3, "outputTokens": 4}') == (3, 4)
    assert usage_tokens('{"input_tokens": 3}') == (3, 0)
    assert usage_tokens(None) == (0, 0)
    assert usage_tokens("nope") == (0, 0)
    assert parse_mcp("a=http://h/mcp") == {"name": "a", "url": "http://h/mcp"}
    with pytest.raises(ValueError):
        parse_mcp("a=ftp://h")
