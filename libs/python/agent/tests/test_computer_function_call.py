"""Tests for executing the computer tool when it arrives as a function call.

Models without native computer-use support are given the computer tool as a
plain function (`loops/openai.py::_map_computer_tool_to_openai` with
`use_native_tool=False`), so they answer with `function_call` items named
"computer" instead of `computer_call` items. This file covers that dispatch
path: argument mapping, execution against the computer handler, and the shape
of the items fed back to the model.

Following SRP: this file tests ONE behavior (the computer function-call path).
"""

import json
from typing import Any, Dict, List, Optional, Union
from unittest.mock import patch

import pytest


class FakeComputerHandler:
    """Minimal computer handler that records the calls it receives."""

    def __init__(self) -> None:
        self.calls: List[Dict[str, Any]] = []

    async def get_environment(self) -> str:
        return "linux"

    async def get_dimensions(self):
        return (1024, 768)

    async def screenshot(self, text: Optional[str] = None) -> str:
        self.calls.append({"action": "screenshot"})
        return "c2NyZWVuc2hvdA=="

    async def click(self, x: int, y: int, button: str = "left") -> None:
        self.calls.append({"action": "click", "x": x, "y": y, "button": button})

    async def keypress(self, keys: Union[List[str], str]) -> None:
        self.calls.append({"action": "keypress", "keys": keys})

    async def type(self, text: str) -> None:
        self.calls.append({"action": "type", "text": text})

    async def drag(self, path: List[Dict[str, int]]) -> None:
        self.calls.append({"action": "drag", "path": path})

    async def terminate(self, status: str = "success") -> Dict[str, Any]:
        self.calls.append({"action": "terminate", "status": status})
        return {"success": True, "status": status, "terminated": True}


def make_agent(**kwargs):
    from cua_agent import ComputerAgent

    return ComputerAgent(model="openai/gpt-5.4", **kwargs)


def computer_function_call(arguments: Any, call_id: str = "call_1") -> Dict[str, Any]:
    return {
        "type": "function_call",
        "id": "fc_1",
        "call_id": call_id,
        "name": "computer",
        "arguments": arguments,
    }


def error_of(items: List[Dict[str, Any]]) -> Optional[str]:
    """The error message of a tool-error item, or None if there isn't one."""
    if len(items) != 1 or items[0].get("type") != "function_call_output":
        return None
    try:
        payload = json.loads(items[0]["output"])
    except (json.JSONDecodeError, TypeError):
        return None
    return payload.get("error") if isinstance(payload, dict) else None


@patch("cua_agent.agent.litellm")
class TestComputerFunctionCallDispatch:
    """A function_call named "computer" runs the requested computer action."""

    @pytest.mark.asyncio
    async def test_flat_arguments_reach_the_computer_handler(self, _litellm, disable_telemetry):
        """The advertised flat schema (`{"action": ..., "x": ...}`) executes.

        Regression for the dispatch gap: this used to fall through to the
        custom-tool lookup and fail with "Function computer not found", so the
        function-calling harness could never actually drive a computer.
        """
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(
            computer_function_call(json.dumps({"action": "click", "x": 12, "y": 34})),
            computer,
        )

        assert error_of(items) is None, error_of(items)
        assert computer.calls[0] == {"action": "click", "x": 12, "y": 34, "button": "left"}

        # The model holds the function tool, not computer_use_preview, so the
        # reply must be a function_call_output; the screenshot rides along as
        # its own user message because that output carries text only.
        assert items[0]["type"] == "function_call_output"
        assert items[0]["call_id"] == "call_1"
        assert "screenshot" in items[0]["output"]
        assert items[1] == {
            "role": "user",
            "content": [
                {
                    "type": "input_image",
                    "image_url": "data:image/png;base64,c2NyZWVuc2hvdA==",
                }
            ],
        }

    @pytest.mark.asyncio
    async def test_native_action_shape_is_accepted(self, _litellm, disable_telemetry):
        """`{"type": "click"}` works too — that is what a retried computer_call carries.

        `replace_failed_computer_calls_with_function_calls` rewrites a failed
        computer_call into a function_call whose arguments are the native
        action dict, so both spellings reach this path.
        """
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(
            computer_function_call(
                json.dumps({"type": "click", "x": 5, "y": 6, "button": "right"})
            ),
            computer,
        )

        assert error_of(items) is None, error_of(items)
        assert computer.calls[0] == {"action": "click", "x": 5, "y": 6, "button": "right"}

    @pytest.mark.asyncio
    async def test_unrelated_and_null_fields_are_dropped(self, _litellm, disable_telemetry):
        """The flat schema is one parameter object shared by every action.

        Models routinely echo fields that belong to other actions — the
        reported trace shows a `keypress` carrying `status: "success"` — and
        send omitted optionals as null. Neither is an argument to `keypress`.
        """
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(
            computer_function_call(
                json.dumps(
                    {
                        "action": "keypress",
                        "keys": ["ctrl", "alt", "t"],
                        "status": "success",
                        "x": None,
                    }
                )
            ),
            computer,
        )

        assert error_of(items) is None, error_of(items)
        assert computer.calls[0] == {"action": "keypress", "keys": ["ctrl", "alt", "t"]}

    @pytest.mark.asyncio
    async def test_drag_start_and_end_become_a_path(self, _litellm, disable_telemetry):
        """The flat schema spells a drag as coordinates; the handler takes a path."""
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(
            computer_function_call(
                json.dumps(
                    {
                        "action": "drag",
                        "start_x": 1,
                        "start_y": 2,
                        "end_x": 30,
                        "end_y": 40,
                    }
                )
            ),
            computer,
        )

        assert error_of(items) is None, error_of(items)
        assert computer.calls[0] == {
            "action": "drag",
            "path": [{"x": 1, "y": 2}, {"x": 30, "y": 40}],
        }

    @pytest.mark.asyncio
    async def test_terminate_reports_its_result_without_a_screenshot(
        self, _litellm, disable_telemetry
    ):
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(
            computer_function_call(json.dumps({"action": "terminate", "status": "success"})),
            computer,
        )

        assert computer.calls == [{"action": "terminate", "status": "success"}]
        assert len(items) == 1
        assert json.loads(items[0]["output"]) == {
            "success": True,
            "status": "success",
            "terminated": True,
        }

    @pytest.mark.asyncio
    async def test_screenshot_action_returns_the_capture(self, _litellm, disable_telemetry):
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(
            computer_function_call(json.dumps({"action": "screenshot"})),
            computer,
        )

        assert error_of(items) is None, error_of(items)
        assert items[1]["content"][0]["type"] == "input_image"

    @pytest.mark.asyncio
    async def test_a_registered_tool_named_computer_still_wins(self, _litellm, disable_telemetry):
        """Built-in computer semantics are the fallback, not an override."""
        seen: List[Dict[str, Any]] = []

        def computer(**kwargs):
            """A caller-registered tool that takes the "computer" name."""
            seen.append(kwargs)
            return "custom tool ran"

        handler = FakeComputerHandler()
        agent = make_agent(tools=[computer])

        items = await agent._handle_item(
            computer_function_call(json.dumps({"action": "click", "x": 1, "y": 2})),
            handler,
        )

        assert seen == [{"action": "click", "x": 1, "y": 2}]
        assert handler.calls == []
        assert items == [
            {"type": "function_call_output", "call_id": "call_1", "output": "custom tool ran"}
        ]

    @pytest.mark.asyncio
    async def test_unknown_action_is_reported_to_the_model(self, _litellm, disable_telemetry):
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(
            computer_function_call(json.dumps({"action": "teleport", "x": 1})),
            computer,
        )

        assert "teleport" in (error_of(items) or "")
        assert computer.calls == []

    @pytest.mark.asyncio
    async def test_missing_action_is_reported_to_the_model(self, _litellm, disable_telemetry):
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(computer_function_call(json.dumps({"x": 1})), computer)

        assert "action" in (error_of(items) or "")
        assert computer.calls == []

    @pytest.mark.asyncio
    async def test_malformed_arguments_are_reported_to_the_model(self, _litellm, disable_telemetry):
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(computer_function_call("{not json"), computer)

        assert "JSON" in (error_of(items) or "")
        assert computer.calls == []

    @pytest.mark.asyncio
    async def test_native_computer_call_path_is_unchanged(self, _litellm, disable_telemetry):
        """The native item type still answers with a computer_call_output."""
        computer = FakeComputerHandler()
        agent = make_agent()

        items = await agent._handle_item(
            {
                "type": "computer_call",
                "call_id": "call_native",
                "action": {"type": "click", "x": 7, "y": 8, "button": "left"},
            },
            computer,
        )

        assert computer.calls[0] == {"action": "click", "x": 7, "y": 8, "button": "left"}
        assert items == [
            {
                "type": "computer_call_output",
                "call_id": "call_native",
                "acknowledged_safety_checks": [],
                "output": {
                    "type": "input_image",
                    "image_url": "data:image/png;base64,c2NyZWVuc2hvdA==",
                },
            }
        ]


class TestFunctionCallScreenshotRetention:
    """Post-action screenshots from this path obey the image retention policy."""

    @staticmethod
    def trajectory(count: int) -> List[Dict[str, Any]]:
        messages: List[Dict[str, Any]] = []
        for i in range(count):
            messages.extend(
                [
                    {
                        "type": "function_call",
                        "call_id": f"call_{i}",
                        "name": "computer",
                        "arguments": json.dumps({"action": "click", "x": i, "y": i}),
                    },
                    {"type": "function_call_output", "call_id": f"call_{i}", "output": "click"},
                    {
                        "role": "user",
                        "content": [
                            {"type": "input_image", "image_url": f"data:image/png;base64,{i}"}
                        ],
                    },
                ]
            )
        return messages

    @pytest.mark.asyncio
    async def test_only_the_most_recent_screenshots_are_kept(self):
        from cua_agent.callbacks import ImageRetentionCallback

        kept = await ImageRetentionCallback(only_n_most_recent_images=2).on_llm_start(
            self.trajectory(4)
        )

        images = [m["content"][0]["image_url"] for m in kept if m.get("role") == "user"]
        assert images == ["data:image/png;base64,2", "data:image/png;base64,3"]

        # The text pairs stay intact: a function_call without its output (or an
        # output without its call) is rejected by the Responses API.
        calls = [m["call_id"] for m in kept if m.get("type") == "function_call"]
        outputs = [m["call_id"] for m in kept if m.get("type") == "function_call_output"]
        assert calls == outputs == [f"call_{i}" for i in range(4)]

    @pytest.mark.asyncio
    async def test_a_user_image_that_is_not_a_post_action_capture_is_left_alone(self):
        """The task's own attached image is input, not a trimmable capture."""
        from cua_agent.callbacks import ImageRetentionCallback

        messages: List[Dict[str, Any]] = [
            {
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "match this"},
                    {"type": "input_image", "image_url": "data:image/png;base64,task"},
                ],
            },
            *self.trajectory(2),
        ]

        kept = await ImageRetentionCallback(only_n_most_recent_images=1).on_llm_start(messages)

        images = [
            content["image_url"]
            for m in kept
            if m.get("role") == "user"
            for content in m["content"]
            if content.get("type") == "input_image"
        ]
        assert images == ["data:image/png;base64,task", "data:image/png;base64,1"]
