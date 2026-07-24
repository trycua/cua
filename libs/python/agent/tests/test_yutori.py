"""
Tests for the Yutori Navigator (n1.5) integration.

Covers:
1. Model routing — Navigator model names resolve to the Yutori loop
2. YutoriAdapter — model name normalization and request param handling
3. Action conversion — the n1.5 action space
4. Normalizer compatibility — converted actions survive OperatorNormalizerCallback
   and match BrowserTool method signatures
5. predict_step — screenshot injection, extra_body params, inline DOM tools,
   ref resolution, key sequences, terminal messages
"""

import base64
import io
import json
from unittest.mock import AsyncMock, patch

import pytest
from litellm.types.utils import ModelResponse
from PIL import Image

from cua_agent.adapters.yutori_adapter import YutoriAdapter
from cua_agent.agent import assert_callable_with
from cua_agent.callbacks.operator_validator import OperatorNormalizerCallback
from cua_agent.decorators import find_agent_config
from cua_agent.loops.yutori import (
    YutoriNavigatorConfig,
    _convert_key_expression,
    _convert_navigator_action,
)
from cua_agent.responses import convert_completion_messages_to_responses_items
from cua_agent.tools.browser_tool import BrowserTool

SCREEN = (1280, 800)


class TestModelRouting:
    """The Yutori loop must match all published Navigator model names."""

    @pytest.mark.parametrize(
        "model",
        [
            "yutori/n1.5",
            "yutori/n1.5-latest",
            "yutori/n1.5-20260428",
            "n1.5-latest",
        ],
    )
    def test_navigator_models_route_to_yutori_loop(self, model):
        config = find_agent_config(model)
        assert config is not None
        assert config.agent_class is YutoriNavigatorConfig

    # yutori/n1 is deprecated and no longer matched; the API rejects it with
    # a clear model_deprecated error if a request is attempted.
    @pytest.mark.parametrize("model", ["claude-sonnet-4-5", "gpt-5.4", "qwen3vl", "yutori/n1"])
    def test_other_models_do_not_route_to_yutori_loop(self, model):
        config = find_agent_config(model)
        assert config is None or config.agent_class is not YutoriNavigatorConfig


class TestYutoriAdapter:
    def test_normalize_model_pins_bare_names_to_latest(self):
        adapter = YutoriAdapter(api_key="test")
        assert adapter._normalize_model("yutori/n1.5") == "n1.5-latest"
        assert adapter._normalize_model("n1.5") == "n1.5-latest"
        # Versioned names pass through unchanged
        assert adapter._normalize_model("yutori/n1.5-latest") == "n1.5-latest"
        assert adapter._normalize_model("n1.5-20260428") == "n1.5-20260428"

    def test_build_params_forwards_extra_body(self):
        adapter = YutoriAdapter(api_key="test")
        params = adapter._build_params(
            {
                "model": "yutori/n1.5",
                "messages": [],
                "extra_body": {"tool_set": "browser_tools_core-20260403"},
            }
        )
        assert params["model"] == "openai/n1.5-latest"
        assert params["extra_body"] == {"tool_set": "browser_tools_core-20260403"}
        assert params["extra_headers"]["Authorization"] == "Bearer test"

    def test_build_params_translates_max_tokens(self):
        # The Yutori API rejects max_tokens and requires max_completion_tokens
        adapter = YutoriAdapter(api_key="test")
        params = adapter._build_params({"model": "yutori/n1.5", "messages": [], "max_tokens": 512})
        assert "max_tokens" not in params
        assert params["max_completion_tokens"] == 512

    def test_yutori_provider_registered_by_computer_agent(self):
        import litellm

        from cua_agent.agent import ComputerAgent

        ComputerAgent(model="yutori/n1.5-latest", tools=[], telemetry_enabled=False)
        providers = {entry["provider"] for entry in litellm.custom_provider_map}
        assert "yutori" in providers


class TestActionConversion:
    """n1.5 action space conversion to internal computer_call actions."""

    def test_clicks_with_modifier(self):
        actions = _convert_navigator_action(
            "left_click", {"coordinates": [500, 500], "modifier": "ctrl"}, *SCREEN
        )
        assert actions == [
            {"action": "click", "button": "left", "x": 640, "y": 400, "modifier": "ctrl"}
        ]

    def test_middle_click(self):
        actions = _convert_navigator_action("middle_click", {"coordinates": [500, 500]}, *SCREEN)
        assert actions == [{"action": "click", "button": "middle", "x": 640, "y": 400}]

    def test_mouse_move(self):
        actions = _convert_navigator_action("mouse_move", {"coordinates": [100, 100]}, *SCREEN)
        assert actions == [{"action": "move", "x": 128, "y": 80}]

    def test_mouse_down_up(self):
        assert _convert_navigator_action("mouse_down", {"coordinates": [10, 10]}, *SCREEN) == [
            {"action": "mouse_down", "x": 13, "y": 8}
        ]
        assert _convert_navigator_action("mouse_up", {}, *SCREEN) == [{"action": "mouse_up"}]

    def test_drag(self):
        actions = _convert_navigator_action(
            "drag", {"start_coordinates": [100, 100], "coordinates": [200, 200]}, *SCREEN
        )
        assert actions == [
            {
                "action": "left_click_drag",
                "start_coordinate": [128, 80],
                "end_coordinate": [256, 160],
            }
        ]

    def test_key_press_lowercase_key_space(self):
        actions = _convert_navigator_action("key_press", {"key": "ctrl+shift+t"}, *SCREEN)
        assert actions == [{"action": "keypress", "keys": ["ctrl", "shift", "t"]}]

    def test_key_press_sequence_expands_to_multiple_actions(self):
        actions = _convert_navigator_action("key_press", {"key": "down down enter"}, *SCREEN)
        assert actions == [
            {"action": "keypress", "keys": ["down"]},
            {"action": "keypress", "keys": ["down"]},
            {"action": "keypress", "keys": ["enter"]},
        ]

    def test_key_aliases(self):
        assert _convert_key_expression("meta+c escape pageup pagedown") == [
            ["cmd", "c"],
            ["esc"],
            ["page_up"],
            ["page_down"],
        ]

    def test_hold_key(self):
        actions = _convert_navigator_action("hold_key", {"key": "shift", "duration": 2}, *SCREEN)
        assert actions == [{"action": "hold_key", "key": "shift", "duration": 2.0}]

    def test_wait_with_duration(self):
        assert _convert_navigator_action("wait", {"duration": 5}, *SCREEN) == [
            {"action": "wait", "time": 5.0}
        ]
        assert _convert_navigator_action("wait", {}, *SCREEN) == [{"action": "wait"}]

    def test_navigation_actions(self):
        assert _convert_navigator_action("go_back", {}, *SCREEN) == [{"action": "history_back"}]
        assert _convert_navigator_action("go_forward", {}, *SCREEN) == [
            {"action": "history_forward"}
        ]
        assert _convert_navigator_action("refresh", {}, *SCREEN) == [{"action": "refresh"}]
        assert _convert_navigator_action("goto_url", {"url": "https://x.com"}, *SCREEN) == [
            {"action": "visit_url", "url": "https://x.com"}
        ]

    def test_ref_pixels_take_precedence(self):
        actions = _convert_navigator_action(
            "left_click", {"ref": "e7", "coordinates": [500, 500]}, *SCREEN, ref_pixels=(321, 123)
        )
        assert actions == [{"action": "click", "button": "left", "x": 321, "y": 123}]

    def test_custom_tool_returns_none(self):
        assert _convert_navigator_action("my_custom_tool", {"a": 1}, *SCREEN) is None


class TestNormalizerCompatibility:
    """Every converted action must survive the operator normalizer and match a
    BrowserTool method signature — this is exactly what the agent validates at
    execution time."""

    CASES = [
        ("left_click", {"coordinates": [500, 500], "modifier": "ctrl"}),
        ("right_click", {"coordinates": [500, 500]}),
        ("middle_click", {"coordinates": [500, 500]}),
        ("double_click", {"coordinates": [1, 2], "modifier": "shift"}),
        ("triple_click", {"coordinates": [1, 2]}),
        ("mouse_move", {"coordinates": [100, 100]}),
        ("mouse_down", {"coordinates": [10, 10]}),
        ("mouse_up", {}),
        ("drag", {"start_coordinates": [100, 100], "coordinates": [200, 200]}),
        ("scroll", {"coordinates": [500, 500], "direction": "down", "amount": 3}),
        ("type", {"text": "hello"}),
        ("key_press", {"key": "ctrl+a"}),
        ("hold_key", {"key": "shift", "duration": 1.5}),
        ("wait", {"duration": 4}),
        ("goto_url", {"url": "https://x.com"}),
        ("go_back", {}),
        ("go_forward", {}),
        ("refresh", {}),
    ]

    @pytest.mark.asyncio
    @pytest.mark.parametrize("fn_name,args", CASES)
    async def test_action_executable_on_browser_tool(self, fn_name, args):
        normalizer = OperatorNormalizerCallback()
        tool = BrowserTool.__new__(BrowserTool)  # methods only, no interface needed

        actions = _convert_navigator_action(fn_name, args, *SCREEN)
        assert actions, f"{fn_name} produced no actions"
        for action in actions:
            fake_cm = {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {
                        "type": "function",
                        "id": "call_x",
                        "function": {"name": "computer", "arguments": json.dumps(action)},
                    }
                ],
            }
            items = convert_completion_messages_to_responses_items([fake_cm])
            items = await normalizer.on_llm_end(items)
            computer_calls = [i for i in items if i.get("type") == "computer_call"]
            assert computer_calls, f"{fn_name}: no computer_call after normalization"
            normalized = computer_calls[0]["action"]
            method = getattr(tool, normalized["type"], None)
            assert method is not None, f"BrowserTool has no method {normalized['type']}"
            action_args = {k: v for k, v in normalized.items() if k != "type"}
            assert_callable_with(method, **action_args)

    @pytest.mark.asyncio
    async def test_normalizer_preserves_modifier_and_wait_time(self):
        normalizer = OperatorNormalizerCallback()
        items = [
            {
                "type": "computer_call",
                "call_id": "c1",
                "action": {"type": "click", "button": "left", "x": 1, "y": 2, "modifier": "ctrl"},
            },
            {
                "type": "computer_call",
                "call_id": "c2",
                "action": {"type": "wait", "time": 5.0},
            },
        ]
        items = await normalizer.on_llm_end(items)
        assert items[0]["action"]["modifier"] == "ctrl"
        assert items[1]["action"]["time"] == 5.0


def _tiny_png_b64():
    img = Image.new("RGB", (64, 40), "white")
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return base64.b64encode(buf.getvalue()).decode()


def _make_response(tool_calls=None, content="", reasoning=None):
    message = {"role": "assistant", "content": content}
    if tool_calls:
        message["tool_calls"] = tool_calls
    if reasoning:
        message["reasoning_content"] = reasoning
    return ModelResponse(
        id="chatcmpl-test",
        choices=[{"index": 0, "message": message, "finish_reason": "stop"}],
        model="n1.5-latest",
        usage={"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
    )


def _tool_call(call_id, name, arguments):
    return {
        "id": call_id,
        "type": "function",
        "function": {"name": name, "arguments": json.dumps(arguments)},
    }


@pytest.fixture
def navigator_handler():
    handler = AsyncMock()
    handler.screenshot = AsyncMock(return_value=_tiny_png_b64())
    handler.get_dimensions = AsyncMock(side_effect=Exception("BrowserTool has no get_dimensions"))
    handler.viewport_width = 1280
    handler.viewport_height = 800
    handler.get_current_url = AsyncMock(return_value="https://example.com")
    handler.evaluate_js = AsyncMock(
        return_value={"success": True, "result": {"pageContent": "[e1] <button> Submit"}}
    )
    return handler


class TestPredictStep:
    @pytest.mark.asyncio
    async def test_click_step_with_extra_body_and_screenshot_injection(self, navigator_handler):
        loop = YutoriNavigatorConfig()
        captured = {}

        async def on_api_start(kwargs):
            captured.update(kwargs)

        response = _make_response(
            tool_calls=[
                _tool_call("call_1", "left_click", {"coordinates": [500, 500], "modifier": "ctrl"})
            ],
            content="Clicking the button",
            reasoning="I should click submit",
        )
        with patch("litellm.acompletion", new=AsyncMock(return_value=response)):
            result = await loop.predict_step(
                messages=[{"role": "user", "content": "click submit"}],
                model="yutori/n1.5-latest",
                computer_handler=navigator_handler,
                tool_set="browser_tools_expanded-20260403",
                api_key="test-key",
                _on_api_start=on_api_start,
            )

        # Yutori-specific params travel in extra_body, not as top-level params
        assert captured["extra_body"] == {"tool_set": "browser_tools_expanded-20260403"}
        assert "tool_set" not in captured

        # A screenshot is injected when the latest message carries no image
        last_message = captured["messages"][-1]
        assert last_message["role"] == "user"
        assert any(part.get("type") == "image_url" for part in last_message["content"])

        computer_calls = [i for i in result["output"] if i["type"] == "computer_call"]
        assert computer_calls[0]["action"]["type"] == "click"
        assert computer_calls[0]["action"]["modifier"] == "ctrl"
        assert any(i["type"] == "reasoning" for i in result["output"])

    @pytest.mark.asyncio
    async def test_dom_tool_executed_inline(self, navigator_handler):
        loop = YutoriNavigatorConfig()
        response = _make_response(
            tool_calls=[_tool_call("call_2", "extract_elements", {"filter": "interactive"})]
        )
        with patch("litellm.acompletion", new=AsyncMock(return_value=response)):
            result = await loop.predict_step(
                messages=[{"role": "user", "content": "list elements"}],
                model="yutori/n1.5-latest",
                computer_handler=navigator_handler,
                api_key="test-key",
            )

        navigator_handler.evaluate_js.assert_awaited_once()
        function_calls = [i for i in result["output"] if i["type"] == "function_call"]
        outputs = [i for i in result["output"] if i["type"] == "function_call_output"]
        assert len(function_calls) == 1 and len(outputs) == 1
        # Matching call ids make the agent skip re-execution (ignore_call_ids)
        assert function_calls[0]["call_id"] == outputs[0]["call_id"]
        assert "[e1] <button> Submit" in outputs[0]["output"]
        assert "Current URL: https://example.com" in outputs[0]["output"]

    @pytest.mark.asyncio
    async def test_dom_tool_unavailable_reports_error_to_model(self, navigator_handler):
        loop = YutoriNavigatorConfig()
        # Old computer-server without the 'evaluate' browser command
        navigator_handler.evaluate_js = AsyncMock(
            return_value={"success": False, "error": "Unknown command: evaluate"}
        )
        response = _make_response(
            tool_calls=[_tool_call("call_2b", "execute_js", {"text": "document.title"})]
        )
        with patch("litellm.acompletion", new=AsyncMock(return_value=response)):
            result = await loop.predict_step(
                messages=[{"role": "user", "content": "get title"}],
                model="yutori/n1.5-latest",
                computer_handler=navigator_handler,
                api_key="test-key",
            )

        outputs = [i for i in result["output"] if i["type"] == "function_call_output"]
        assert outputs and outputs[0]["output"].startswith("[ERROR]")

    @pytest.mark.asyncio
    async def test_key_sequence_expands_with_unique_call_ids(self, navigator_handler):
        loop = YutoriNavigatorConfig()
        response = _make_response(
            tool_calls=[_tool_call("call_3", "key_press", {"key": "down enter"})]
        )
        with patch("litellm.acompletion", new=AsyncMock(return_value=response)):
            result = await loop.predict_step(
                messages=[{"role": "user", "content": "press keys"}],
                model="yutori/n1.5-latest",
                computer_handler=navigator_handler,
                api_key="test-key",
            )

        computer_calls = [i for i in result["output"] if i["type"] == "computer_call"]
        assert len(computer_calls) == 2
        assert len({c["call_id"] for c in computer_calls}) == 2
        assert [c["action"]["keys"] for c in computer_calls] == [["down"], ["enter"]]

    @pytest.mark.asyncio
    async def test_ref_click_resolved_via_evaluate_js(self, navigator_handler):
        loop = YutoriNavigatorConfig()
        navigator_handler.evaluate_js = AsyncMock(
            return_value={"success": True, "result": {"success": True, "coordinates": [321, 123]}}
        )
        response = _make_response(tool_calls=[_tool_call("call_5", "left_click", {"ref": "e7"})])
        with patch("litellm.acompletion", new=AsyncMock(return_value=response)):
            result = await loop.predict_step(
                messages=[{"role": "user", "content": "click the ref"}],
                model="yutori/n1.5-latest",
                computer_handler=navigator_handler,
                api_key="test-key",
            )

        computer_calls = [i for i in result["output"] if i["type"] == "computer_call"]
        assert computer_calls[0]["action"]["x"] == 321
        assert computer_calls[0]["action"]["y"] == 123

    @pytest.mark.asyncio
    async def test_no_tool_calls_is_terminal(self, navigator_handler):
        loop = YutoriNavigatorConfig()
        response = _make_response(content="The answer is 42.")
        with patch("litellm.acompletion", new=AsyncMock(return_value=response)):
            result = await loop.predict_step(
                messages=[{"role": "user", "content": "question"}],
                model="yutori/n1.5-latest",
                computer_handler=navigator_handler,
                api_key="test-key",
            )

        messages = [i for i in result["output"] if i["type"] == "message"]
        assert messages[0]["content"][0]["text"] == "The answer is 42."
