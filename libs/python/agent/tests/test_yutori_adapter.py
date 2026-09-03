from unittest.mock import Mock

import pytest


def test_yutori_adapter_routes_prefixed_model_to_openai_and_uses_api_base(monkeypatch):
    from cua_agent.adapters import yutori_adapter as module
    from cua_agent.adapters.yutori_adapter import YutoriAdapter

    captured = {}

    def fake_completion(**params):
        captured.update(params)
        return Mock()

    monkeypatch.setattr(module, "completion", fake_completion)

    adapter = YutoriAdapter(base_url="https://default.example/v1", api_key="default-key")
    adapter.completion(
        model="yutori/yutori-admin/n2os-joint-test",
        messages=[],
        api_base="https://baseten.example/v1",
        api_key="call-key",
    )

    assert captured["model"] == "openai/yutori-admin/n2os-joint-test"
    assert captured["api_base"] == "https://baseten.example/v1"
    assert captured["api_key"] == "call-key"
    assert captured["extra_headers"]["Authorization"] == "Bearer call-key"


@pytest.mark.asyncio
async def test_yutori_adapter_async_route_accepts_stripped_model(monkeypatch):
    from cua_agent.adapters import yutori_adapter as module
    from cua_agent.adapters.yutori_adapter import YutoriAdapter

    captured = {}

    async def fake_acompletion(**params):
        captured.update(params)
        return Mock()

    monkeypatch.setattr(module, "acompletion", fake_acompletion)

    adapter = YutoriAdapter(base_url="https://default.example/v1", api_key="default-key")
    await adapter.acompletion(model="yutori-admin/n2os-joint-test", messages=[])

    assert captured["model"] == "openai/yutori-admin/n2os-joint-test"
    assert captured["api_base"] == "https://default.example/v1"
    assert captured["api_key"] == "default-key"
    assert captured["stream"] is False


def test_computer_agent_registers_yutori_provider(disable_telemetry):
    import litellm
    from cua_agent import ComputerAgent

    agent = ComputerAgent(model="yutori/yutori-admin/n2os-joint-test")

    assert type(agent.agent_loop).__name__ == "GenericVlmConfig"
    assert "yutori" in [item["provider"] for item in litellm.custom_provider_map]


def test_yutori_n1_model_still_uses_browser_loop(disable_telemetry):
    from cua_agent import ComputerAgent

    agent = ComputerAgent(model="yutori/n1")

    assert type(agent.agent_loop).__name__ == "YutoriN1Config"
