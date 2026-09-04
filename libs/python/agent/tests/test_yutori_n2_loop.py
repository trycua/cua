import base64
import json
from pathlib import Path

import pytest

from cua_agent.loops import yutori_n2
from cua_agent.responses import convert_responses_items_to_completion_messages

PNG_1X1 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII="
)


class MockUsage:
    def model_dump(self):
        return {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}


class MockResponse:
    def __init__(
        self,
        content: str = "",
        tool_calls=None,
        request_id: str | None = None,
        finish_reason: str = "stop",
    ):
        self.usage = {}
        self._hidden_params = {}
        self._content = content
        self._tool_calls = tool_calls or []
        self.request_id = request_id
        self.finish_reason = finish_reason

    def model_dump(self):
        payload = {
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": self._content,
                        "tool_calls": self._tool_calls,
                    },
                    "finish_reason": self.finish_reason,
                }
            ]
        }
        if self.request_id is not None:
            payload["request_id"] = self.request_id
        return payload


class MockComputerHandler:
    def __init__(self, fail_on: str | None = None):
        self.calls = []
        self.fail_on = fail_on
        self.screenshot_count = 0

    async def get_dimensions(self):
        return 2000, 1000

    async def screenshot(self):
        self.screenshot_count += 1
        return PNG_1X1

    async def click(self, x, y, button="left"):
        self.calls.append(("click", x, y, button))
        if self.fail_on == "click":
            raise RuntimeError("click failed")

    async def double_click(self, x, y):
        self.calls.append(("double_click", x, y))

    async def scroll(self, x, y, scroll_x, scroll_y):
        self.calls.append(("scroll", x, y, scroll_x, scroll_y))

    async def type(self, text):
        self.calls.append(("type", text))
        if self.fail_on == "type":
            raise RuntimeError("type failed")

    async def wait(self, ms=1000):
        self.calls.append(("wait", ms))

    async def move(self, x, y):
        self.calls.append(("move", x, y))

    async def keypress(self, keys):
        self.calls.append(("keypress", keys))

    async def drag(self, path):
        self.calls.append(("drag", path))

    async def left_mouse_down(self, x=None, y=None):
        self.calls.append(("left_mouse_down", x, y))

    async def left_mouse_up(self, x=None, y=None):
        self.calls.append(("left_mouse_up", x, y))

    async def key_down(self, key):
        self.calls.append(("key_down", key))

    async def key_up(self, key):
        self.calls.append(("key_up", key))


@pytest.fixture
def yutori_n2_test_env(monkeypatch):
    monkeypatch.setattr(
        yutori_n2.LiteLLMCompletionResponsesConfig,
        "_transform_chat_completion_usage_to_responses_usage",
        lambda usage: MockUsage(),
    )


def input_messages():
    return [
        {
            "role": "user",
            "content": [
                {"type": "input_text", "text": "Use the computer."},
                {"type": "input_image", "image_url": f"data:image/png;base64,{PNG_1X1}"},
            ],
        }
    ]


def structured_tool_call(name, arguments, call_id="call_0"):
    return {
        "id": call_id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": json.dumps(arguments),
        },
    }


def tool_result_text(item):
    output = item["output"]
    if isinstance(output, dict):
        return output.get("result", "")
    return output


async def run_predict(
    monkeypatch,
    *,
    content: str = "",
    tool_calls=None,
    computer_handler=None,
    **kwargs,
):
    captured = {}

    async def fake_acompletion(**api_kwargs):
        captured.update(api_kwargs)
        return MockResponse(content=content, tool_calls=tool_calls)

    monkeypatch.setattr(yutori_n2.litellm, "acompletion", fake_acompletion)
    result = await yutori_n2.YutoriN2Config().predict_step(
        messages=input_messages(),
        model="yutori/yutori-admin/n2os-joint-test",
        tools=[],
        computer_handler=computer_handler or MockComputerHandler(),
        **kwargs,
    )
    return result, captured


@pytest.mark.asyncio
async def test_custom_api_base_receives_native_n2_tool_surface(monkeypatch, yutori_n2_test_env):
    result, captured = await run_predict(
        monkeypatch,
        content="done",
        api_base="https://baseten.example/v1",
    )

    assert result["output"][0]["content"][0]["text"] == "done"
    assert captured["messages"][0]["role"] == "system"
    assert captured["tool_choice"] == "auto"
    assert captured["parallel_tool_calls"] is True
    assert "tool_set" not in captured
    assert [tool["function"]["name"] for tool in captured["tools"]] == [
        "computer_batch",
        "edit",
        "read",
        "write",
        "bash",
    ]


@pytest.mark.asyncio
async def test_default_yutori_api_receives_native_tool_set(monkeypatch, yutori_n2_test_env):
    _, captured = await run_predict(monkeypatch, content="done")

    assert captured["tool_set"] == yutori_n2.YUTORI_N2_TOOL_SET
    assert "tools" not in captured
    assert captured["parallel_tool_calls"] is True
    assert captured["messages"][0]["role"] == "user"


@pytest.mark.asyncio
async def test_default_yutori_api_forwards_disable_tools(monkeypatch, yutori_n2_test_env):
    _, captured = await run_predict(
        monkeypatch,
        content="done",
        disable_tools=["bash", "read"],
    )

    assert captured["disable_tools"] == ["bash", "read"]


@pytest.mark.asyncio
async def test_custom_api_base_removes_disabled_native_tools(monkeypatch, yutori_n2_test_env):
    _, captured = await run_predict(
        monkeypatch,
        content="done",
        api_base="https://baseten.example/v1",
        disable_tools=["bash", "read"],
    )

    assert [tool["function"]["name"] for tool in captured["tools"]] == [
        "computer_batch",
        "edit",
        "write",
    ]


@pytest.mark.asyncio
async def test_custom_tool_cannot_shadow_native_n2_tool(monkeypatch, yutori_n2_test_env):
    async def fake_acompletion(**api_kwargs):
        raise AssertionError("model should not be called")

    monkeypatch.setattr(yutori_n2.litellm, "acompletion", fake_acompletion)

    with pytest.raises(ValueError, match="shadows a native Yutori N2 tool"):
        await yutori_n2.YutoriN2Config().predict_step(
            messages=input_messages(),
            model="yutori/yutori-admin/n2os-joint-test",
            tools=[
                {
                    "type": "function",
                    "function": {"name": "bash", "parameters": {"type": "object"}},
                }
            ],
            computer_handler=MockComputerHandler(),
        )


@pytest.mark.asyncio
async def test_structured_computer_batch_executes_as_one_logical_tool_result(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {
                "actions": [
                    {"name": "left_click", "arguments": {"coordinates": [500, 500]}},
                    {"action": "type", "text": "done"},
                ]
            },
            call_id="call_batch",
        )
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [("click", 1000, 500, "left"), ("type", "done")]
    assert computer.screenshot_count == 1
    assert not any(item.get("type") == "computer_call" for item in result["output"])
    assert [item["type"] for item in result["output"]] == [
        "function_call",
        "function_call_output",
    ]
    assert result["output"][1]["call_id"] == "call_batch"
    assert result["output"][1]["output"]["type"] == "input_image"
    assert result["output"][1]["output"]["image_url"].startswith("data:image/png;base64,")
    assert result["output"][1]["output"]["result"] == "[0:left_click] \n[1:type] "


@pytest.mark.asyncio
async def test_computer_batch_screenshot_action_reports_result_text(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {"actions": [{"action": "screenshot"}]},
            call_id="call_batch",
        )
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.screenshot_count == 1
    assert result["output"][1]["output"]["result"] == (
        "[0:screenshot] screenshot queued (delivered after the batch)"
    )


@pytest.mark.asyncio
async def test_computer_batch_stops_later_actions_after_runtime_error(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler(fail_on="click")
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {
                "actions": [
                    {"action": "left_click", "coordinates": [500, 500]},
                    {"action": "type", "text": "done"},
                ]
            },
        )
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [("click", 1000, 500, "left")]
    assert computer.screenshot_count == 1
    assert tool_result_text(result["output"][1]) == (
        "[0:left_click] ERROR: RuntimeError: click failed\n"
        "batch stopped at actions[0] (0:left_click): ERROR: RuntimeError: click failed "
        "(0 completed, 1 skipped)"
    )


@pytest.mark.asyncio
async def test_computer_batch_validates_all_members_before_execution(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {
                "actions": [
                    {"action": "left_click", "coordinates": [1200, 500]},
                    {"action": "type", "text": "done"},
                ]
            },
        )
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == []
    assert computer.screenshot_count == 0
    assert "Batch validation failed" in result["output"][1]["output"]
    assert "inclusive 0-1000 range" in result["output"][1]["output"]


def test_computer_batch_accepts_twenty_actions():
    actions = [{"action": "wait", "duration": 0}] * 20

    result = yutori_n2._validate_computer_batch({"actions": actions}, dimensions=(2000, 1000))

    assert len(result) == 20


def test_computer_batch_rejects_twenty_one_actions():
    actions = [{"action": "wait", "duration": 0}] * 21

    with pytest.raises(ValueError, match="at most 20 actions, got 21"):
        yutori_n2._validate_computer_batch({"actions": actions}, dimensions=(2000, 1000))


@pytest.mark.asyncio
async def test_text_xml_tool_call_is_recovered_and_text_is_preserved(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    content = """
    I will type the URL.
    <tool_call>
    <function=computer>
    <parameter=type>type</parameter>
    <parameter=text>https://example.com</parameter>
    </function>
    </tool_call>
    """

    result, _ = await run_predict(
        monkeypatch,
        content=content,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert result["output"][0]["content"][0]["text"] == "I will type the URL."
    assert computer.calls == [("type", "https://example.com")]
    assert result["output"][2]["output"]["type"] == "input_image"
    assert result["output"][2]["output"]["result"] == "[0:type] "


@pytest.mark.asyncio
async def test_recovered_click_with_button_parameter_executes(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    content = """
    <tool_call>
    <function=computer>
    <parameter=x>250</parameter>
    <parameter=y>60</parameter>
    <parameter=type>click</parameter>
    <parameter=button>left</parameter>
    </function>
    </tool_call>
    """

    result, _ = await run_predict(
        monkeypatch,
        content=content,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [("click", 500, 60, "left")]
    assert result["output"][1]["output"]["type"] == "input_image"
    assert result["output"][1]["output"]["result"] == "[0:left_click] "


@pytest.mark.asyncio
async def test_malformed_tool_call_only_response_returns_tool_error(
    monkeypatch,
    yutori_n2_test_env,
):
    content = """
    <tool_call>
    <function=computer>
    <parameter=triple_click>
    <parameter=coordinate>
    [1116
    </parameter>
    <parameter=type>
    click
    </parameter>
    </function>
    </tool_call>
    """

    result, _ = await run_predict(
        monkeypatch,
        content=content,
        api_base="https://baseten.example/v1",
    )

    assert result["output"]
    assert result["output"][1]["type"] == "function_call_output"
    assert "Computer action validation failed" in result["output"][1]["output"]


@pytest.mark.asyncio
async def test_triple_click_executes_three_clicks_with_one_screenshot(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {"actions": [{"action": "triple_click", "coordinates": [500, 500]}]},
        )
    ]

    await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [
        ("click", 1000, 500, "left"),
        ("click", 1000, 500, "left"),
        ("click", 1000, 500, "left"),
    ]
    assert computer.screenshot_count == 1


@pytest.mark.asyncio
async def test_modifier_is_held_around_click(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {
                "actions": [
                    {
                        "action": "left_click",
                        "coordinates": [500, 500],
                        "modifier": "ctrl+shift",
                    }
                ]
            },
        )
    ]

    await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [
        ("key_down", "ctrl"),
        ("key_down", "shift"),
        ("click", 1000, 500, "left"),
        ("key_up", "shift"),
        ("key_up", "ctrl"),
    ]


@pytest.mark.asyncio
async def test_horizontal_scroll_uses_scroll_x(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {
                "actions": [
                    {
                        "action": "scroll",
                        "coordinates": [500, 500],
                        "direction": "left",
                        "amount": 2,
                    }
                ]
            },
        )
    ]

    await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [("scroll", 1000, 500, -400, 0)]


@pytest.mark.asyncio
async def test_key_press_space_separated_sequence_executes_sequentially(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {"actions": [{"action": "key_press", "key": "down down enter"}]},
        )
    ]

    await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls[:3] == [
        ("keypress", ["down"]),
        ("keypress", ["down"]),
        ("keypress", ["enter"]),
    ]


@pytest.mark.asyncio
async def test_wait_duration_is_passed_to_computer_handler(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {"actions": [{"action": "wait", "duration": 2.5}]},
        )
    ]

    await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [("wait", 2500)]


@pytest.mark.asyncio
async def test_wait_defaults_to_five_seconds(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {"actions": [{"action": "wait"}]},
        )
    ]

    await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [("wait", 5000)]


@pytest.mark.asyncio
async def test_hold_key_uses_key_down_and_key_up(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {"actions": [{"action": "hold_key", "key": "shift", "duration": 0}]},
        )
    ]

    await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [("key_down", "shift"), ("key_up", "shift")]


@pytest.mark.asyncio
async def test_durationless_hold_key_wraps_next_batch_member(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler()
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {
                "actions": [
                    {"action": "hold_key", "key": "shift"},
                    {"action": "type", "text": "A"},
                    {"action": "type", "text": "B"},
                ]
            },
        )
    ]

    await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [
        ("key_down", "shift"),
        ("type", "A"),
        ("key_up", "shift"),
        ("type", "B"),
    ]


@pytest.mark.asyncio
async def test_durationless_hold_key_releases_after_next_member_fails(
    monkeypatch,
    yutori_n2_test_env,
):
    computer = MockComputerHandler(fail_on="type")
    tool_calls = [
        structured_tool_call(
            "computer_batch",
            {
                "actions": [
                    {"action": "hold_key", "key": "shift"},
                    {"action": "type", "text": "A"},
                    {"action": "type", "text": "B"},
                ]
            },
        )
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        computer_handler=computer,
        api_base="https://baseten.example/v1",
    )

    assert computer.calls == [
        ("key_down", "shift"),
        ("type", "A"),
        ("key_up", "shift"),
    ]
    assert tool_result_text(result["output"][1]) == (
        "[0:hold_key] \n"
        "[1:type] ERROR: RuntimeError: type failed\n"
        "batch stopped at actions[1] (1:type): ERROR: RuntimeError: type failed "
        "(1 completed, 1 skipped)"
    )


@pytest.mark.asyncio
async def test_bash_tool_executes_and_returns_output(
    monkeypatch,
    yutori_n2_test_env,
    tmp_path: Path,
):
    tool_calls = [
        structured_tool_call("bash", {"command": "printf ok", "timeout": 5}, call_id="call_bash")
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        api_base="https://baseten.example/v1",
        n2_cwd=str(tmp_path),
    )

    assert result["output"][0]["name"] == "bash"
    assert result["output"][1]["output"] == "ok"


@pytest.mark.asyncio
async def test_bash_timeout_returns_normal_tool_result(tmp_path: Path):
    output, updated_cwd = await yutori_n2._execute_bash(
        {
            "command": "python -c 'import time; time.sleep(1)'",
            "timeout": 0.01,
        },
        tmp_path,
    )

    assert output == "Command timed out after 0.01s"
    assert updated_cwd is None


@pytest.mark.asyncio
async def test_bash_working_directory_persists_between_calls(
    monkeypatch,
    yutori_n2_test_env,
    tmp_path: Path,
):
    tool_calls = [
        structured_tool_call(
            "bash",
            {"command": "mkdir -p child && cd child && pwd", "timeout": 5},
            call_id="call_cd",
        ),
        structured_tool_call("bash", {"command": "pwd", "timeout": 5}, call_id="call_pwd"),
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        api_base="https://baseten.example/v1",
        n2_cwd=str(tmp_path),
    )

    outputs = [
        item["output"] for item in result["output"] if item["type"] == "function_call_output"
    ]
    assert outputs == [f"{tmp_path / 'child'}\n", f"{tmp_path / 'child'}\n"]


@pytest.mark.asyncio
async def test_write_read_and_edit_tools_execute_locally(
    monkeypatch,
    yutori_n2_test_env,
    tmp_path: Path,
):
    target = tmp_path / "file.txt"
    tool_calls = [
        structured_tool_call(
            "write",
            {"file_path": "file.txt", "content": "alpha\nbeta\n"},
            call_id="call_write",
        ),
        structured_tool_call(
            "edit",
            {"file_path": "file.txt", "old_string": "beta\n", "new_string": "gamma\n"},
            call_id="call_edit",
        ),
        structured_tool_call(
            "read",
            {"file_path": "file.txt", "offset": 2, "limit": 1},
            call_id="call_read",
        ),
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        api_base="https://baseten.example/v1",
        n2_cwd=str(tmp_path),
    )

    assert target.read_text(encoding="utf-8") == "alpha\ngamma\n"
    outputs = [
        item["output"] for item in result["output"] if item["type"] == "function_call_output"
    ]
    assert outputs[0] == "File created successfully at: file.txt"
    assert outputs[1].startswith("The file file.txt has been updated successfully:")
    assert "     2\tgamma" in outputs[1]
    assert outputs[-1] == "     2\tgamma"


@pytest.mark.asyncio
async def test_read_image_file_returns_visible_image_result(
    monkeypatch,
    yutori_n2_test_env,
    tmp_path: Path,
):
    target = tmp_path / "image.png"
    target.write_bytes(base64.b64decode(PNG_1X1))
    tool_calls = [
        structured_tool_call("read", {"file_path": "image.png"}, call_id="call_read"),
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        api_base="https://baseten.example/v1",
        n2_cwd=str(tmp_path),
    )

    output = result["output"][1]["output"]
    assert output["type"] == "input_image"
    assert output["image_url"].startswith("data:image/webp;base64,")
    assert output["result"] == "Loaded image image.png (1x1)"


@pytest.mark.asyncio
async def test_read_text_output_uses_sdk_truncation_marker(tmp_path: Path):
    target = tmp_path / "large.txt"
    target.write_text("x" * (yutori_n2.YUTORI_N2_READ_MAX_OUTPUT_CHARS + 20), encoding="utf-8")

    output = await yutori_n2._execute_read(
        {"file_path": "large.txt"},
        tmp_path,
        {},
    )

    assert output.startswith("     1\t")
    assert "[... output truncated, " in output
    assert " more chars ...]" in output


@pytest.mark.asyncio
async def test_edit_requires_prior_read(
    monkeypatch,
    yutori_n2_test_env,
    tmp_path: Path,
):
    target = tmp_path / "file.txt"
    target.write_text("alpha\nbeta\n", encoding="utf-8")
    tool_calls = [
        structured_tool_call(
            "edit",
            {"file_path": "file.txt", "old_string": "beta", "new_string": "gamma"},
            call_id="call_edit",
        )
    ]

    result, _ = await run_predict(
        monkeypatch,
        tool_calls=tool_calls,
        api_base="https://baseten.example/v1",
        n2_cwd=str(tmp_path),
    )

    assert target.read_text(encoding="utf-8") == "alpha\nbeta\n"
    assert result["output"][1]["output"].startswith("ERROR: you must read file.txt")


@pytest.mark.asyncio
async def test_edit_rejects_non_boolean_replace_all(tmp_path: Path):
    target = tmp_path / "file.txt"
    target.write_text("alpha\nalpha\n", encoding="utf-8")
    read_fingerprints = {}
    data = target.read_bytes()
    yutori_n2._record_read_fingerprint(read_fingerprints, target, data)

    with pytest.raises(ValueError, match="replace_all must be a boolean"):
        await yutori_n2._execute_edit(
            {
                "file_path": "file.txt",
                "old_string": "alpha",
                "new_string": "beta",
                "replace_all": "false",
            },
            tmp_path,
            read_fingerprints,
        )

    assert target.read_text(encoding="utf-8") == "alpha\nalpha\n"


@pytest.mark.asyncio
async def test_default_yutori_api_chains_previous_request_id(monkeypatch, yutori_n2_test_env):
    calls = []
    responses = [
        MockResponse(content="first", request_id="req_1"),
        MockResponse(content="second", request_id="req_2"),
    ]

    async def fake_acompletion(**api_kwargs):
        calls.append(api_kwargs)
        return responses.pop(0)

    monkeypatch.setattr(yutori_n2.litellm, "acompletion", fake_acompletion)
    config = yutori_n2.YutoriN2Config()

    for _ in range(2):
        await config.predict_step(
            messages=input_messages(),
            model="yutori/yutori-admin/n2os-joint-test",
            tools=[],
            computer_handler=MockComputerHandler(),
        )

    assert "extra_body" not in calls[0]
    assert calls[1]["extra_body"] == {"prev_request_id": "req_1"}


@pytest.mark.asyncio
async def test_malformed_tool_call_text_retries_once(monkeypatch, yutori_n2_test_env):
    calls = []
    responses = [
        MockResponse(content="<tool_call>not parseable", request_id="req_1"),
        MockResponse(content="done", request_id="req_2"),
    ]

    async def fake_acompletion(**api_kwargs):
        calls.append(api_kwargs)
        return responses.pop(0)

    monkeypatch.setattr(yutori_n2.litellm, "acompletion", fake_acompletion)

    result = await yutori_n2.YutoriN2Config().predict_step(
        messages=input_messages(),
        model="yutori/yutori-admin/n2os-joint-test",
        tools=[],
        computer_handler=MockComputerHandler(),
    )

    assert len(calls) == 2
    assert calls[1]["messages"][-2] == {
        "role": "assistant",
        "content": "<tool_call>not parseable",
    }
    assert (
        calls[1]["messages"][-1]["content"] == yutori_n2.YUTORI_N2_MALFORMED_TOOL_CALL_RETRY_MESSAGE
    )
    assert calls[1]["extra_body"] == {"prev_request_id": "req_1"}
    assert result["output"][0]["content"][0]["text"] == "done"


@pytest.mark.asyncio
async def test_length_response_without_tool_calls_retries_once(monkeypatch, yutori_n2_test_env):
    calls = []
    responses = [
        MockResponse(content="partial", finish_reason="length"),
        MockResponse(content="done"),
    ]

    async def fake_acompletion(**api_kwargs):
        calls.append(api_kwargs)
        return responses.pop(0)

    monkeypatch.setattr(yutori_n2.litellm, "acompletion", fake_acompletion)

    result = await yutori_n2.YutoriN2Config().predict_step(
        messages=input_messages(),
        model="yutori/yutori-admin/n2os-joint-test",
        tools=[],
        computer_handler=MockComputerHandler(),
    )

    assert len(calls) == 2
    assert calls[1]["messages"][-2] == {"role": "assistant", "content": "partial"}
    assert calls[1]["messages"][-1]["content"] == yutori_n2.YUTORI_N2_LENGTH_RETRY_MESSAGE
    assert result["output"][0]["content"][0]["text"] == "done"


def test_image_tool_output_result_text_is_preserved_for_next_model_call():
    messages = [
        {
            "type": "function_call_output",
            "call_id": "call_0",
            "output": {
                "type": "input_image",
                "image_url": f"data:image/png;base64,{PNG_1X1}",
                "result": (
                    "[0:left_click] ERROR: RuntimeError: click failed\n"
                    "batch stopped at actions[0] (0:left_click): ERROR: RuntimeError: "
                    "click failed (0 completed, 1 skipped)"
                ),
            },
        }
    ]

    converted = convert_responses_items_to_completion_messages(
        messages,
        allow_images_in_tool_results=True,
    )

    assert converted == [
        {
            "role": "tool",
            "tool_call_id": "call_0",
            "content": [
                {
                    "type": "text",
                    "text": (
                        "[0:left_click] ERROR: RuntimeError: click failed\n"
                        "batch stopped at actions[0] (0:left_click): ERROR: RuntimeError: "
                        "click failed (0 completed, 1 skipped)"
                    ),
                },
                {
                    "type": "image_url",
                    "image_url": {"url": f"data:image/png;base64,{PNG_1X1}"},
                },
            ],
        }
    ]


@pytest.mark.asyncio
async def test_computer_agent_resets_yutori_n2_state_between_independent_runs(
    monkeypatch,
    yutori_n2_test_env,
    tmp_path: Path,
):
    from cua_agent import ComputerAgent

    calls = []
    responses = [
        MockResponse(
            tool_calls=[
                structured_tool_call(
                    "bash",
                    {"command": "mkdir -p child && cd child && pwd", "timeout": 5},
                    call_id="call_cd",
                )
            ],
            request_id="req_1",
        ),
        MockResponse(content="done first", request_id="req_2"),
        MockResponse(
            tool_calls=[
                structured_tool_call(
                    "bash",
                    {"command": "pwd", "timeout": 5},
                    call_id="call_pwd",
                )
            ],
            request_id="req_3",
        ),
        MockResponse(content="done second", request_id="req_4"),
    ]

    async def fake_acompletion(**api_kwargs):
        calls.append(api_kwargs)
        return responses.pop(0)

    monkeypatch.setattr(yutori_n2.litellm, "acompletion", fake_acompletion)
    monkeypatch.chdir(tmp_path)

    agent = ComputerAgent(
        model="yutori/yutori-admin/n2os-joint-test",
        telemetry_enabled=False,
    )

    async def collect_run_outputs():
        outputs = []
        async for chunk in agent.run(input_messages()):
            outputs.extend(chunk["output"])
        return outputs

    await collect_run_outputs()
    second_outputs = await collect_run_outputs()

    second_tool_outputs = [
        item["output"] for item in second_outputs if item["type"] == "function_call_output"
    ]
    assert second_tool_outputs == [f"{tmp_path}\n"]
    assert "extra_body" not in calls[0]
    assert calls[1]["extra_body"] == {"prev_request_id": "req_1"}
    assert "extra_body" not in calls[2]
