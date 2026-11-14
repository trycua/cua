"""The generic VLM loop replays its own computer calls in the Qwen tool schema.

The loop advertises Qwen's ``computer`` tool (``action`` plus 0..1000 ``coordinate``)
but stores each call as a Computer Calls action in screen pixels. The history it sends
back must use the advertised schema, or the model drifts to pixel ``x``/``y`` calls that
skip coordinate scaling.
"""

import asyncio
import base64
import io
import json
import sys
import types

import litellm
import pytest
from cua_agent.loops import generic_vlm
from cua_agent.loops.generic_vlm import (
    GenericVlmConfig,
    _computer_action_to_qwen_args,
    _unnormalize_coordinate,
    convert_qwen_tool_args_to_computer_action,
)
from PIL import Image

DIMS = (1920, 1080)


def _screenshot_url() -> str:
    buf = io.BytesIO()
    Image.new("RGB", DIMS, "white").save(buf, "PNG")
    return "data:image/png;base64," + base64.b64encode(buf.getvalue()).decode()


@pytest.fixture
def sent_messages(monkeypatch):
    """Capture what predict_step sends, without the qwen extras or a model server."""
    fake_utils = types.ModuleType("qwen_vl_utils")
    fake_utils.smart_resize = lambda h, w, **_: (h, w)
    monkeypatch.setitem(sys.modules, "qwen_vl_utils", fake_utils)
    monkeypatch.setattr(generic_vlm, "_build_nous_system", lambda functions: None)
    sent: list = []

    async def acompletion(**kwargs):
        sent.extend(kwargs["messages"])
        return litellm.ModelResponse(
            choices=[{"message": {"role": "assistant", "content": "done"}}],
            usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
        )

    monkeypatch.setattr(generic_vlm.litellm, "acompletion", acompletion)
    return sent


def _replayed_arguments(sent: list) -> list:
    return [
        json.loads(call["function"]["arguments"])
        for message in sent
        for call in message.get("tool_calls") or []
        if call["function"]["name"] == "computer"
    ]


async def test_replayed_computer_calls_use_the_qwen_schema(sent_messages):
    history = [
        {"role": "user", "content": "Save the file."},
        {
            "type": "computer_call",
            "call_id": "call_1",
            "status": "completed",
            "action": {"type": "click", "button": "right", "x": 960, "y": 540},
        },
        {
            "type": "computer_call_output",
            "call_id": "call_1",
            "output": {"type": "input_image", "image_url": _screenshot_url()},
        },
        {
            "type": "computer_call",
            "call_id": "call_2",
            "status": "completed",
            "action": {"type": "keypress", "keys": ["ctrl", "s"]},
        },
        {
            "type": "computer_call_output",
            "call_id": "call_2",
            "output": {"type": "input_image", "image_url": _screenshot_url()},
        },
    ]

    await GenericVlmConfig().predict_step(messages=history, model="openai/qwen3-vl")

    assert _replayed_arguments(sent_messages) == [
        {"action": "right_click", "coordinate": [500, 500]},
        {"action": "key", "keys": ["ctrl", "s"]},
    ]


@pytest.mark.parametrize(
    "qwen_args",
    [
        {"action": "left_click", "coordinate": [204, 736]},
        {"action": "right_click", "coordinate": [500, 500]},
        {"action": "middle_click", "coordinate": [1, 999]},
        {"action": "double_click", "coordinate": [10, 990]},
        {"action": "mouse_move", "coordinate": [333, 667]},
        {"action": "type", "text": "=D3*F3*24"},
        {"action": "key", "keys": ["ctrl", "s"]},
        {"action": "scroll", "pixels": -5, "coordinate": [400, 400]},
        {"action": "wait"},
    ],
)
def test_restated_calls_round_trip(qwen_args):
    pixels = asyncio.run(_unnormalize_coordinate(qwen_args, DIMS))
    action = dict(convert_qwen_tool_args_to_computer_action(pixels))
    kind = action.pop("action")
    if kind in ("left_click", "right_click", "middle_click"):
        action = {"type": "click", "button": kind.split("_")[0], **action}
    else:
        action = {"type": kind, **action}

    assert _computer_action_to_qwen_args(action, DIMS) == qwen_args


def test_held_keys_and_single_key_strings_are_restated():
    assert _computer_action_to_qwen_args({"type": "key_down", "keys": "shift"}, DIMS) == {
        "action": "key_down",
        "keys": ["shift"],
    }
    assert _computer_action_to_qwen_args({"type": "keypress", "keys": "enter"}, DIMS) == {
        "action": "key",
        "keys": ["enter"],
    }


def test_calls_already_in_the_qwen_schema_are_left_alone():
    assert (
        _computer_action_to_qwen_args({"action": "left_click", "coordinate": [1, 2]}, DIMS) is None
    )
