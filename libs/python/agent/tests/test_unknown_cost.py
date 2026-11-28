"""Unknown pricing must not crash reporting or bypass spending limits."""

from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock

import pytest


@pytest.mark.asyncio
@pytest.mark.parametrize("usage", [{"response_cost": None}, {}])
async def test_unknown_cost_stops_budgeted_run(usage, capsys):
    from cua_agent.callbacks.budget_manager import BudgetManagerCallback

    callback = BudgetManagerCallback(1.0)
    await callback.on_usage(usage)
    assert await callback.on_run_continue({}, [], []) is False
    assert "unknown" in capsys.readouterr().out.lower()
    assert callback.total_cost == 0.0


@pytest.mark.asyncio
async def test_cli_reports_unknown_total(monkeypatch, capsys):
    import cua_agent.cli as cli

    class Agent:
        agent_config_info = SimpleNamespace(agent_class=type("TestLoop", (), {}))

        async def run(self, history):
            yield {"usage": {"response_cost": 0.25}, "output": []}
            yield {"usage": {"response_cost": None}, "output": []}
            yield {
                "usage": {"response_cost": 0.5},
                "output": [{"type": "computer_call", "action": {"type": "click", "x": 1, "y": 2}}],
            }

    async def quit_input(*args):
        return "quit"

    monkeypatch.setattr(cli, "ainput", quit_input)
    monkeypatch.setattr(cli, "yaspin", MagicMock())
    monkeypatch.setattr(cli.dotenv, "load_dotenv", lambda: None)
    await cli.chat_loop(Agent(), "custom/model", "test", initial_prompt="hello")
    output = capsys.readouterr().out
    assert "unknown" in output.lower()
    assert "Total cost: $" not in output


@pytest.mark.parametrize(
    "value,expected",
    [
        (None, None),
        (0, 0.0),
        (0.25, 0.25),
        (2, 2.0),
        (True, None),
        ("0.2", None),
        (-1, None),
        (float("nan"), None),
        (float("inf"), None),
    ],
)
def test_cost_normalization(value, expected):
    from cua_agent.usage import normalize_response_cost

    assert normalize_response_cost(value) == expected


@pytest.mark.asyncio
@pytest.mark.parametrize("reset", [True, False])
async def test_unknown_budget_state_obeys_run_reset(reset):
    from cua_agent.callbacks.budget_manager import BudgetManagerCallback

    callback = BudgetManagerCallback(1.0, reset_after_each_run=reset)
    await callback.on_usage({"response_cost": None})
    await callback.on_usage({"response_cost": 0.25})
    assert not await callback.on_run_continue({}, [], [])
    await callback.on_run_start({}, [])
    assert await callback.on_run_continue({}, [], []) is reset


@pytest.mark.asyncio
async def test_budget_known_prices_and_error_mode():
    from cua_agent.callbacks.budget_manager import (
        BudgetExceededError,
        BudgetManagerCallback,
    )

    callback = BudgetManagerCallback(1.0, raise_error=True)
    await callback.on_usage({"response_cost": 0})
    await callback.on_usage({"response_cost": 0.25})
    assert await callback.on_run_continue({}, [], [])
    await callback.on_usage({"response_cost": 0.75})
    with pytest.raises(BudgetExceededError, match="Budget exceeded"):
        await callback.on_run_continue({}, [], [])
    await callback.on_run_start({}, [])
    with pytest.raises(BudgetExceededError, match="Cannot enforce budget.*unknown"):
        await callback.on_usage({"response_cost": None})


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["logging", "trajectory"])
@pytest.mark.parametrize("omit_unknown", [False, True])
async def test_usage_loggers_keep_unknown_totals(kind, omit_unknown, tmp_path):
    from cua_agent.callbacks.logging import LoggingCallback
    from cua_agent.callbacks.trajectory_saver import TrajectorySaverCallback

    callback = LoggingCallback() if kind == "logging" else TrajectorySaverCallback(str(tmp_path))
    callback.total_usage = {}
    for cost in [0.25, None, 0.5]:
        usage = {"total_tokens": 3}
        if cost is not None or not omit_unknown:
            usage["response_cost"] = cost
        await callback.on_usage(usage)
    assert callback.total_usage == {"response_cost": None, "total_tokens": 9}


@pytest.mark.asyncio
async def test_telemetry_does_not_report_partial_total_as_complete(monkeypatch):
    import cua_agent.callbacks.telemetry as telemetry

    monkeypatch.setattr(telemetry, "is_telemetry_enabled", lambda: True)
    events = []
    monkeypatch.setattr(telemetry, "record_event", lambda *args: events.append(args))
    callback = telemetry.TelemetryCallback(SimpleNamespace())
    for cost in [0.25, None, 0.5]:
        await callback.on_usage({"response_cost": cost, "total_tokens": 3})
    assert callback.total_usage["response_cost"] is None
    assert callback.total_usage["total_tokens"] == 9
    unknown_event = [args[1] for args in events if args[0] == "agent_usage"][1]
    assert "response_cost" not in unknown_event


@pytest.mark.asyncio
@pytest.mark.parametrize("cost", [None, 0, 0.25])
@pytest.mark.parametrize("model_response", [False, True])
async def test_openai_loop_normalizes_dict_and_model_costs(monkeypatch, cost, model_response):
    from unittest.mock import AsyncMock

    import cua_agent.loops.openai as loop

    payload = {
        "usage": {"total_tokens": 3},
        "output": [],
        "_hidden_params": {"response_cost": cost},
    }
    response = (
        SimpleNamespace(
            usage=payload["usage"],
            _hidden_params=payload["_hidden_params"],
            model_dump=lambda: dict(payload),
        )
        if model_response
        else payload
    )
    monkeypatch.setattr(loop.litellm, "aresponses", AsyncMock(return_value=response))
    on_usage = AsyncMock()
    result = await loop.OpenAIComputerUseConfig().predict_step(
        [], "custom/model", _on_usage=on_usage
    )
    assert result["usage"]["response_cost"] == cost
    on_usage.assert_awaited_once_with(result["usage"])


@pytest.mark.asyncio
async def test_cli_preserves_priced_totals_and_ignores_non_usage_chunks(monkeypatch, capsys):
    import cua_agent.cli as cli

    class Agent:
        agent_config_info = SimpleNamespace(agent_class=type("TestLoop", (), {}))

        async def run(self, history):
            yield {"usage": {"response_cost": 0.25}, "output": []}
            yield {"output": []}
            yield {
                "usage": {"response_cost": 0.5},
                "output": [{"type": "message", "role": "assistant", "content": [{"text": "done"}]}],
            }

    monkeypatch.setattr(cli, "ainput", AsyncMock(return_value="quit"))
    monkeypatch.setattr(cli, "yaspin", MagicMock())
    await cli.chat_loop(Agent(), "priced/model", "test", initial_prompt="hello")
    output = capsys.readouterr().out
    assert "Total cost: $0.75" in output
    assert "unknown" not in output.lower()


@pytest.mark.asyncio
async def test_final_response_reports_unknown_budget_without_another_iteration(capsys):
    from cua_agent import ComputerAgent
    from cua_agent.callbacks.budget_manager import BudgetManagerCallback

    callback = BudgetManagerCallback(1.0)
    agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929", callbacks=[callback])

    async def predict_step(**kwargs):
        await kwargs["_on_usage"]({"response_cost": None})
        return {
            "output": [{"type": "message", "role": "assistant", "content": []}],
            "usage": {"response_cost": None},
        }

    agent.agent_loop = SimpleNamespace(predict_step=predict_step)
    results = [result async for result in agent.run("hello")]
    assert len(results) == 1
    assert "Cannot enforce budget" in capsys.readouterr().out
    assert not await callback.on_run_continue({}, [], [])


@pytest.mark.asyncio
async def test_tool_result_chunks_have_zero_cost(monkeypatch):
    from cua_agent import ComputerAgent

    agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929")
    step = 0

    async def predict_step(**kwargs):
        nonlocal step
        step += 1
        await kwargs["_on_usage"]({"response_cost": 0.25})
        output = (
            [{"type": "computer_call", "call_id": "call-1", "action": {"type": "screenshot"}}]
            if step == 1
            else [{"type": "message", "role": "assistant", "content": []}]
        )
        return {"output": output, "usage": {"response_cost": 0.25}}

    agent.agent_loop = SimpleNamespace(predict_step=predict_step)
    monkeypatch.setattr(
        agent,
        "_handle_item",
        AsyncMock(
            side_effect=[[{"type": "computer_call_output", "call_id": "call-1", "output": {}}], []]
        ),
    )
    results = [result async for result in agent.run("hello")]
    assert [result["usage"]["response_cost"] for result in results] == [0.25, 0.0, 0.25]


@pytest.mark.asyncio
@pytest.mark.parametrize("with_token_counts", [False, True])
async def test_direct_gemini_usage_is_unpriced_and_stops_budget(
    with_token_counts, monkeypatch, capsys
):
    import cua_agent.loops.gemini as loop
    from cua_agent.callbacks.budget_manager import BudgetManagerCallback

    metadata = (
        SimpleNamespace(prompt_token_count=3, candidates_token_count=2, total_token_count=5)
        if with_token_counts
        else None
    )
    response = SimpleNamespace(usage_metadata=metadata, candidates=[])
    generate_content = MagicMock(return_value=response)
    client = SimpleNamespace(models=SimpleNamespace(generate_content=generate_content))
    monkeypatch.setattr(loop, "_lazy_import_genai", lambda: (None, MagicMock()))
    monkeypatch.setattr(loop, "_create_gemini_client", lambda model, genai, kwargs: (client, model))
    monkeypatch.setattr(
        loop, "_convert_messages_to_gemini_contents", lambda messages, types: ([], (1024, 768))
    )
    budget = BudgetManagerCallback(1.0)
    result = await loop.GeminiComputerUseConfig().predict_step(
        [], "gemini-2.5-computer-use-preview-test", _on_usage=budget.on_usage
    )
    assert result["usage"]["response_cost"] is None
    if with_token_counts:
        assert result["usage"]["total_tokens"] == 5
    assert not await budget.on_run_continue({}, [], [])
    assert "Cannot enforce budget" in capsys.readouterr().out
    generate_content.assert_called_once()
