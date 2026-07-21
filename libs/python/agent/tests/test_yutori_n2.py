"""Contract tests for the Yutori n2-preview computer-use integration."""

import base64
import copy
import io
import json
import time
from unittest.mock import AsyncMock, patch

import pytest
from litellm.types.utils import ModelResponse
from PIL import Image

from cua_agent.adapters.yutori_adapter import YutoriAdapter
from cua_agent.agent import ComputerAgent
from cua_agent.decorators import find_agent_config
from cua_agent.loops.yutori import YutoriNavigatorConfig
from cua_agent.loops.yutori_n2 import (
    N2ActionValidationError,
    N2_MODEL_IMAGE_MAX_HEIGHT,
    N2_MODEL_IMAGE_MAX_WIDTH,
    TOOL_SET_COMPUTER_USE_BATCH,
    YutoriN2Config,
    _function_call_with_execution,
    parse_n2_key_expression,
    prepare_n2_image_data_url,
    retain_n2_request_images,
    translate_n2_action,
    translate_n2_batch,
)
from cua_agent.responses import convert_responses_items_to_completion_messages


def _png_data_url(width=64, height=40, color="white"):
    image = Image.new("RGB", (width, height), color)
    output = io.BytesIO()
    image.save(output, format="PNG")
    return f"data:image/png;base64,{base64.b64encode(output.getvalue()).decode()}"


def _response(tool_calls=None, content="", reasoning=None):
    message = {"role": "assistant", "content": content}
    if tool_calls:
        message["tool_calls"] = tool_calls
    if reasoning:
        message["reasoning_content"] = reasoning
    return ModelResponse(
        id="chatcmpl-test",
        choices=[{"index": 0, "message": message, "finish_reason": "stop"}],
        model="n2-preview",
        usage={"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
    )


def _tool_call(call_id, name, arguments):
    return {
        "id": call_id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": arguments if isinstance(arguments, str) else json.dumps(arguments),
        },
    }


class TestRouting:
    @pytest.mark.parametrize("model", ["yutori/n2-preview", "n2-preview"])
    def test_exact_preview_models_use_n2_computer_loop(self, model):
        config = find_agent_config(model)

        assert config is not None
        assert config.agent_class is YutoriN2Config
        assert config.tool_type == "computer"

    @pytest.mark.parametrize("model", ["yutori/n2", "n2", "yutori/n2-latest", "n2-latest"])
    def test_no_bare_or_stable_n2_alias(self, model):
        config = find_agent_config(model)

        assert config is None or config.agent_class is not YutoriN2Config

    def test_n1_5_routing_is_unchanged(self):
        assert find_agent_config("yutori/n1.5-latest").agent_class is YutoriNavigatorConfig

    def test_adapter_does_not_invent_n2_latest_alias(self):
        adapter = YutoriAdapter(api_key="test")

        assert adapter._normalize_model("yutori/n1.5") == "n1.5-latest"
        assert adapter._normalize_model("yutori/n2") == "n2"
        assert adapter._normalize_model("yutori/n2-preview") == "n2-preview"


class TestKeyGrammar:
    def test_combos_sequences_aliases_and_punctuation(self):
        assert parse_n2_key_expression("ctrl+c down enter") == [
            ["ctrl", "c"],
            ["down"],
            ["enter"],
        ]
        assert parse_n2_key_expression("meta+plus command+bracketleft") == [
            ["cmd", "+"],
            ["cmd", "["],
        ]
        assert parse_n2_key_expression(
            "minus equal comma period slash backslash semicolon quote backquote bracketright"
        ) == [["-"], ["="], [","], ["."], ["/"], ["\\"], [";"], ["'"], ["`"], ["]"]]

    @pytest.mark.parametrize("expression", ["hyper+c", "ctrl++c", "", "unknown"])
    def test_unknown_or_malformed_keys_are_rejected(self, expression):
        with pytest.raises(N2ActionValidationError):
            parse_n2_key_expression(expression)


class TestTranslation:
    def test_retina_coordinates_use_native_screenshot_pixels(self):
        actions = translate_n2_action("left_click", {"coordinates": [500, 500]}, 2880, 1800)

        assert actions == [{"type": "click", "x": 1440, "y": 900, "button": "left"}]

    @pytest.mark.parametrize(
        ("action", "arguments", "expected_type"),
        [
            ("double_click", {"coordinates": [0, 0]}, "double_click"),
            ("middle_click", {"coordinates": [0, 0]}, "click"),
            ("right_click", {"coordinates": [0, 0]}, "click"),
            ("mouse_move", {"coordinates": [0, 0]}, "move"),
            (
                "drag",
                {"start_coordinates": [0, 0], "coordinates": [1000, 1000]},
                "drag",
            ),
            (
                "scroll",
                {"coordinates": [500, 500], "direction": "left", "amount": 2},
                "scroll",
            ),
            ("type", {"text": "hello"}, "type"),
            ("key_press", {"key": "cmd+c"}, "keypress"),
            ("wait", {"duration": 10}, "wait"),
            ("screenshot", {}, "screenshot"),
        ],
    )
    def test_all_individual_actions_translate(self, action, arguments, expected_type):
        translated = translate_n2_action(action, arguments, 1280, 800)

        assert translated[0]["type"] == expected_type

    def test_triple_click_stays_one_yutori_call_with_two_internal_actions(self):
        translated = translate_n2_action("triple_click", {"coordinates": [500, 500]}, 1280, 800)

        assert [action["type"] for action in translated] == ["double_click", "click"]

    def test_batch_prevalidates_every_member_and_forbids_screenshot(self):
        valid, translated = translate_n2_batch(
            {
                "actions": [
                    {"action": "mouse_move", "coordinates": [0, 0]},
                    {"action": "wait"},
                ]
            },
            1280,
            800,
        )

        assert len(valid) == 2
        assert [action["batch_index"] for action in translated] == [0, 1]
        with pytest.raises(N2ActionValidationError, match="screenshot"):
            translate_n2_batch({"actions": [{"action": "screenshot"}]}, 1280, 800)

    @pytest.mark.parametrize(
        ("action", "arguments"),
        [
            ("left_click", {"coordinates": [-1, 0]}),
            ("left_click", {"coordinates": [1, 2], "unknown": True}),
            ("scroll", {"coordinates": [1, 2], "direction": "diagonal", "amount": 1}),
            ("wait", {"duration": 10.1}),
            ("key_press", {"key": "hyper+c"}),
            ("unknown", {}),
        ],
    )
    def test_invalid_actions_are_rejected(self, action, arguments):
        with pytest.raises(N2ActionValidationError):
            translate_n2_action(action, arguments, 1280, 800)


class TestScreenshotsAndRetention:
    @pytest.mark.parametrize(
        ("source_size", "expected_size"),
        [
            ((2880, 1800), (1280, 800)),
            ((1600, 1200), (1067, 800)),
            ((640, 400), (640, 400)),
        ],
    )
    def test_model_image_is_jpeg_aspect_preserving_and_never_upscaled(
        self, source_size, expected_size
    ):
        result = prepare_n2_image_data_url(_png_data_url(*source_size))
        encoded = result.split(",", 1)[1]
        with Image.open(io.BytesIO(base64.b64decode(encoded))) as image:
            assert image.format == "JPEG"
            assert image.size == expected_size
            assert image.width <= N2_MODEL_IMAGE_MAX_WIDTH
            assert image.height <= N2_MODEL_IMAGE_MAX_HEIGHT

    def test_retention_keeps_all_images_in_two_newest_image_messages_only(self):
        messages = [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": "old"},
                    {"type": "image_url", "image_url": {"url": "old-1"}},
                    {"type": "image_url", "image_url": {"url": "old-2"}},
                ],
            },
            {"role": "assistant", "content": "reasoning remains"},
            {
                "role": "tool",
                "tool_call_id": "one",
                "content": [
                    {"type": "image_url", "image_url": {"url": "recent-1"}},
                    {"type": "image_url", "image_url": {"url": "recent-2"}},
                ],
            },
            {"role": "assistant", "content": "text does not consume a slot"},
            {
                "role": "tool",
                "tool_call_id": "two",
                "content": [
                    {"type": "image_url", "image_url": {"url": "latest-1"}},
                    {"type": "image_url", "image_url": {"url": "latest-2"}},
                ],
            },
        ]
        original = copy.deepcopy(messages)

        retained = retain_n2_request_images(messages, max_messages_bytes=100_000)
        urls = [
            part["image_url"]["url"]
            for message in retained
            for part in message.get("content", [])
            if isinstance(part, dict) and part.get("type") == "image_url"
        ]

        assert urls == ["recent-1", "recent-2", "latest-1", "latest-2"]
        assert retained[1]["content"] == "reasoning remains"
        assert retained[3]["content"] == "text does not consume a slot"
        assert messages == original

    def test_budget_drops_older_retained_message_but_never_partial_newest(self):
        messages = [
            {
                "role": "tool",
                "content": [
                    {"type": "image_url", "image_url": {"url": "a" * 1000}},
                ],
            },
            {
                "role": "tool",
                "content": [
                    {"type": "image_url", "image_url": {"url": "b" * 1000}},
                    {"type": "image_url", "image_url": {"url": "c" * 1000}},
                ],
            },
        ]

        retained = retain_n2_request_images(messages, max_messages_bytes=2300)

        assert retained[0]["content"] == []
        assert len(retained[1]["content"]) == 2
        with pytest.raises(ValueError, match="newest n2 screenshot message"):
            retain_n2_request_images(messages, max_messages_bytes=100)


class TestPredictStep:
    @pytest.mark.asyncio
    async def test_server_injected_tools_and_native_coordinate_mapping(self):
        loop = YutoriN2Config()
        handler = AsyncMock()
        handler.screenshot.return_value = _png_data_url(200, 100).split(",", 1)[1]
        captured = {}
        response = _response(
            [_tool_call("call-1", "left_click", {"coordinates": [500, 500]})],
            content="Clicking",
        )

        async def on_api_start(kwargs):
            captured.update(kwargs)

        with patch("litellm.acompletion", new=AsyncMock(return_value=response)):
            result = await loop.predict_step(
                messages=[{"role": "user", "content": "click"}],
                model="yutori/n2-preview",
                computer_handler=handler,
                tools=[{"type": "computer", "display_width": 999}],
                tool_set=TOOL_SET_COMPUTER_USE_BATCH,
                api_key="test",
                _on_api_start=on_api_start,
            )

        assert "tools" not in captured
        assert captured["stream"] is False
        assert captured["temperature"] == 0.0
        assert captured["extra_body"] == {"tool_set": TOOL_SET_COMPUTER_USE_BATCH}
        image_url = next(
            part["image_url"]["url"]
            for message in captured["messages"]
            for part in message.get("content", [])
            if isinstance(part, dict) and part.get("type") == "image_url"
        )
        assert image_url.startswith("data:image/jpeg;base64,")
        call = next(item for item in result["output"] if item["type"] == "function_call")
        assert call["name"] == "left_click"
        assert call["_computer_actions"] == [{"type": "click", "x": 100, "y": 50, "button": "left"}]

    @pytest.mark.asyncio
    async def test_batch_is_one_internal_item_and_invalid_batch_is_inline_error(self):
        loop = YutoriN2Config()
        image_message = {
            "role": "user",
            "content": [
                {"type": "input_image", "image_url": _png_data_url(1280, 800)},
                {"type": "input_text", "text": "go"},
            ],
        }
        valid_response = _response(
            [
                _tool_call(
                    "batch-1",
                    "computer_batch",
                    {
                        "actions": [
                            {"action": "mouse_move", "coordinates": [1, 2]},
                            {"action": "wait", "duration": 0},
                        ]
                    },
                )
            ]
        )
        invalid_response = _response(
            [_tool_call("batch-2", "computer_batch", {"actions": [{"action": "screenshot"}]})]
        )

        with patch(
            "litellm.acompletion",
            new=AsyncMock(side_effect=[valid_response, invalid_response]),
        ):
            valid = await loop.predict_step(
                messages=[image_message], model="yutori/n2-preview", computer_handler=AsyncMock()
            )
            invalid = await loop.predict_step(
                messages=[image_message], model="yutori/n2-preview", computer_handler=AsyncMock()
            )

        valid_calls = [item for item in valid["output"] if item["type"] == "function_call"]
        assert len(valid_calls) == 1
        assert len(valid_calls[0]["_computer_actions"]) == 2
        assert [item["type"] for item in invalid["output"]] == [
            "function_call",
            "function_call_output",
        ]
        assert "screenshot" in invalid["output"][1]["output"]


def _bare_agent(confirmation=None):
    agent = ComputerAgent.__new__(ComputerAgent)
    agent.callbacks = []
    agent.action_confirmation_callback = confirmation
    agent.screenshot_delay = 0
    agent.telemetry_enabled = False
    return agent


class TestOrchestration:
    @pytest.mark.asyncio
    async def test_batch_executes_in_order_and_returns_one_screenshot_result(self):
        order = []
        computer = AsyncMock()
        computer.move.side_effect = lambda **_: order.append("move")
        computer.type.side_effect = lambda **_: order.append("type")
        computer.screenshot.return_value = "fresh"
        item = _function_call_with_execution(
            "computer_batch",
            {
                "actions": [
                    {"action": "mouse_move", "coordinates": [1, 2]},
                    {"action": "type", "text": "hello"},
                ]
            },
            "batch-1",
            [
                {"type": "move", "x": 1, "y": 2, "batch_index": 0},
                {"type": "type", "text": "hello", "batch_index": 1},
            ],
            batch_actions=[
                {"action": "mouse_move", "coordinates": [1, 2]},
                {"action": "type", "text": "hello"},
            ],
        )

        result = await _bare_agent()._handle_item(item, computer)

        assert order == ["move", "type"]
        computer.screenshot.assert_awaited_once()
        metadata = result[0]["output"]["result"]
        assert metadata["completed"] == 2
        assert metadata["failed"] == 0
        messages = convert_responses_items_to_completion_messages(
            [item, result[0]], allow_images_in_tool_results=True
        )
        assert messages[0]["tool_calls"][0]["function"]["name"] == "computer_batch"
        assert messages[1]["role"] == "tool"
        assert len(messages[1]["content"]) == 2

    @pytest.mark.asyncio
    async def test_runtime_failure_stops_remaining_members(self):
        computer = AsyncMock()
        computer.move.return_value = None
        computer.type.side_effect = RuntimeError("keyboard offline")
        computer.keypress.return_value = None
        computer.screenshot.return_value = "fresh"
        item = _function_call_with_execution(
            "computer_batch",
            {"actions": []},
            "batch-1",
            [
                {"type": "move", "x": 1, "y": 2, "batch_index": 0},
                {"type": "type", "text": "hello", "batch_index": 1},
                {"type": "keypress", "keys": ["enter"], "batch_index": 2},
            ],
            batch_actions=[{"action": "move"}, {"action": "type"}, {"action": "key_press"}],
        )

        result = await _bare_agent()._handle_item(item, computer)

        computer.keypress.assert_not_awaited()
        assert result[0]["output"]["result"]["completed"] == 1
        assert result[0]["output"]["result"]["failed"] == 1
        assert result[0]["output"]["result"]["skipped"] == 1
        assert result[0]["output"]["result"]["failed_action_index"] == 1

    @pytest.mark.asyncio
    async def test_confirmation_once_and_screenshot_action_not_duplicated(self):
        confirmation = AsyncMock(return_value=False)
        computer = AsyncMock()
        item = _function_call_with_execution(
            "computer_batch",
            {"actions": [{"action": "type", "text": "secret"}]},
            "batch-1",
            [{"type": "type", "text": "secret", "batch_index": 0}],
            batch_actions=[{"action": "type", "text": "secret"}],
        )

        denied = await _bare_agent(confirmation)._handle_item(item, computer)

        confirmation.assert_awaited_once()
        computer.type.assert_not_awaited()
        computer.screenshot.assert_not_awaited()
        assert "not confirmed" in denied[0]["output"]

        screenshot_item = _function_call_with_execution(
            "screenshot", {}, "shot-1", [{"type": "screenshot"}]
        )
        computer.screenshot.return_value = "only-shot"
        result = await _bare_agent()._handle_item(screenshot_item, computer)
        computer.screenshot.assert_awaited_once()
        assert result[0]["output"]["image_url"].endswith("only-shot")

    @pytest.mark.asyncio
    async def test_deadline_is_checked_before_each_batch_member(self):
        computer = AsyncMock()
        computer.screenshot.return_value = "fresh"
        item = _function_call_with_execution(
            "computer_batch",
            {"actions": [{"action": "wait"}]},
            "batch-1",
            [{"type": "wait", "ms": 0, "batch_index": 0}],
            batch_actions=[{"action": "wait"}],
            execution_deadline=time.monotonic() - 1,
        )

        result = await _bare_agent()._handle_item(item, computer)

        computer.wait.assert_not_awaited()
        assert result[0]["output"]["result"]["error"] == "deadline_reached"
        assert result[0]["output"]["result"]["skipped"] == 1
