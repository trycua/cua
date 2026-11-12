"""Telemetry events emitted by cua-agent.

``record_event`` is always mocked, so nothing is sent anywhere.
"""

import json
import logging
from unittest.mock import MagicMock, patch

import pytest


@pytest.fixture(autouse=True)
def no_network(monkeypatch):
    """Fail if anything in these tests opens a network connection."""
    import socket

    def _blocked(*_args, **_kwargs):
        raise AssertionError("network access attempted during telemetry test")

    monkeypatch.setenv("LITELLM_LOCAL_MODEL_COST_MAP", "True")
    monkeypatch.setattr(socket.socket, "connect", _blocked)
    monkeypatch.setattr(socket.socket, "connect_ex", _blocked)
    monkeypatch.setattr(socket, "create_connection", _blocked)


FORBIDDEN_KEYS = {"uploaded_trajectory", "api_key_hash", "vm_name", "os_version"}


def _all_payloads(mock_record_event):
    return [(c[0][0], c[0][1] if len(c[0]) > 1 else {}) for c in mock_record_event.call_args_list]


def _assert_no_forbidden(payloads, *secrets):
    for name, data in payloads:
        assert not (FORBIDDEN_KEYS & set(data)), f"{name} leaked {FORBIDDEN_KEYS & set(data)}"
        blob = json.dumps(data, default=str)
        for secret in secrets:
            assert secret not in blob, f"{name} leaked {secret!r}"


class TestAgentTelemetryEvents:
    @patch("cua_agent.agent.record_event")
    @patch("cua_agent.agent.is_telemetry_enabled", return_value=True)
    def test_agent_init_event(self, mock_telemetry_enabled, mock_record_event):
        from cua_agent.agent import ComputerAgent

        ComputerAgent(
            model="anthropic/claude-sonnet-4-5-20250929",
            instructions="Test instructions",
            max_retries=5,
            trajectory_dir="/tmp/test",
        )
        calls = [c for c in mock_record_event.call_args_list if c[0][0] == "agent_init"]
        assert len(calls) == 1
        _, event_data = calls[0][0]
        assert event_data["model"] == "anthropic/claude-sonnet-4-5-20250929"
        assert "instructions" in event_data["args_provided"]
        assert "max_retries" in event_data["args_provided"]
        assert "trajectory_dir" in event_data["args_provided"]

    @patch("cua_agent.agent.record_event")
    @patch("cua_agent.agent.is_telemetry_enabled", return_value=True)
    def test_agent_init_minimal_args(self, mock_telemetry_enabled, mock_record_event):
        from cua_agent.agent import ComputerAgent

        ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")
        calls = [c for c in mock_record_event.call_args_list if c[0][0] == "agent_init"]
        assert len(calls) == 1
        _, event_data = calls[0][0]
        assert "instructions" not in event_data["args_provided"]
        assert "trajectory_dir" not in event_data["args_provided"]
        assert "max_retries" not in event_data["args_provided"]

    @patch("cua_agent.agent.record_event")
    @patch("cua_agent.agent.is_telemetry_enabled", return_value=True)
    def test_agent_init_never_sends_api_key_or_hash(self, mock_enabled, mock_record_event):
        from cua_agent.agent import ComputerAgent

        secret = "sk-test-SECRET-value-1234567890"
        ComputerAgent(
            model="/Users/alice/models/private.gguf",
            api_key=secret,
            additional_generation_kwargs={"alice_private_kwarg": 1},
        )
        payloads = _all_payloads(mock_record_event)
        _assert_no_forbidden(payloads, secret, "alice")
        init = [d for n, d in payloads if n == "agent_init"][0]
        assert "api_key" in init["args_provided"]
        assert init["model"] == "custom"
        assert "alice_private_kwarg" not in init["args_provided"]

    @patch("cua_agent.agent.record_event")
    @patch("cua_agent.agent.is_telemetry_enabled", return_value=False)
    def test_no_events_when_telemetry_disabled(self, mock_telemetry_enabled, mock_record_event):
        from cua_agent.agent import ComputerAgent

        ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929", telemetry_enabled=False)
        assert not [c for c in mock_record_event.call_args_list if c[0][0] == "agent_init"]


def _agent_with_sandbox_name(name="alice-secret-vm"):
    agent = MagicMock()
    agent.model = "anthropic/claude-sonnet-4-5-20250929"
    agent.agent_loop = None
    handler = MagicMock()
    handler._sandbox = MagicMock()
    handler._sandbox.name = name
    agent.computer_handler = handler
    return agent


class TestTelemetryCallback:
    @pytest.mark.asyncio
    @patch("cua_agent.callbacks.telemetry.record_event")
    @patch("cua_agent.callbacks.telemetry.is_telemetry_enabled", return_value=True)
    async def test_no_trajectory_vm_name_or_hash_ever_sent(
        self, mock_enabled, mock_record_event, caplog
    ):
        from cua_agent.callbacks import telemetry as tmod

        tmod._trajectory_warning_emitted = False
        prompt = "please open my bank account alice@example.com"
        items = [
            {"role": "user", "content": prompt},
            {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "SECRET-OUTPUT"}],
            },
            {
                "type": "computer_call_output",
                "output": {"type": "input_image", "image_url": "data:image/png;base64,SCREEN"},
            },
        ]
        with caplog.at_level(logging.WARNING):
            cb = tmod.TelemetryCallback(_agent_with_sandbox_name(), log_trajectory=True)
            cb2 = tmod.TelemetryCallback(_agent_with_sandbox_name(), log_trajectory=True)
        assert (
            sum("trajectory sharing is not available" in r.getMessage() for r in caplog.records)
            == 1
        )

        for c in (cb, cb2):
            await c.on_run_start({}, items)
            await c.on_responses({}, {})
            await c.on_usage(
                {
                    "prompt_tokens": 3,
                    "completion_tokens": 4,
                    "total_tokens": 7,
                    "response_cost": 0.01,
                    "prompt_tokens_details": {"cached": "x"},
                    "provider_raw": prompt,
                }
            )
            await c.on_run_end({}, items, items)

        payloads = _all_payloads(mock_record_event)
        names = {n for n, _ in payloads}
        assert {"agent_session_start", "agent_run_start", "agent_run_end", "agent_usage"} <= names
        _assert_no_forbidden(
            payloads, prompt, "SECRET-OUTPUT", "SCREEN", "alice-secret-vm", "alice@example.com"
        )

        usage = [d for n, d in payloads if n == "agent_usage"][0]
        assert set(usage) == {
            "session_id",
            "run_id",
            "step",
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "response_cost",
        }

    @pytest.mark.asyncio
    @patch("cua_agent.callbacks.telemetry.record_event")
    @patch("cua_agent.callbacks.telemetry.is_telemetry_enabled", return_value=True)
    async def test_session_start_is_coarse(self, mock_enabled, mock_record_event):
        import sys

        from cua_agent.callbacks.telemetry import TelemetryCallback

        class MyPrivateLoop:
            pass

        agent = _agent_with_sandbox_name()
        agent.agent_loop = MyPrivateLoop()
        agent.model = "huggingface-local/alice/private-model"
        TelemetryCallback(agent)
        name, data = _all_payloads(mock_record_event)[0]
        assert name == "agent_session_start"
        assert data["agent_type"] == "custom"
        assert data["model"] == "huggingface-local/custom"
        assert data["python_version"] == f"{sys.version_info.major}.{sys.version_info.minor}"
        assert "os_version" not in data


class TestToolExecutedEvents:
    @pytest.mark.asyncio
    @patch("cua_agent.agent.record_event")
    @patch("cua_agent.agent.is_telemetry_enabled", return_value=True)
    async def test_function_tool_name_not_sent(self, mock_enabled, mock_record_event):
        from cua_agent.agent import ComputerAgent

        def alice_private_tool(x: int) -> int:
            """Private tool."""
            return x + 1

        agent = ComputerAgent(
            model="anthropic/claude-sonnet-4-5-20250929", tools=[alice_private_tool]
        )
        agent.telemetry_enabled = True
        await agent._handle_item(
            {
                "type": "function_call",
                "call_id": "c1",
                "name": "alice_private_tool",
                "arguments": json.dumps({"x": 1}),
            }
        )
        tool_events = [d for n, d in _all_payloads(mock_record_event) if n == "agent_tool_executed"]
        assert tool_events == [{"tool_type": "function"}]


class TestPackageInit:
    def test_module_init_python_version_is_major_minor(self):
        import sys

        src = open(__import__("cua_agent").__file__).read()
        assert "sys.version," not in src
        assert "sys.version_info.major" in src
        assert f"{sys.version_info.major}.{sys.version_info.minor}"


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
