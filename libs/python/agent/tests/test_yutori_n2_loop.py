import json
from pathlib import Path

import pytest

from cua_agent.loops import yutori_n2

PNG_1X1 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII="
)


class MockUsage:
    def model_dump(self):
        return {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}


class MockResponse:
    def __init__(self, content: str = "", tool_calls=None):
        self.usage = {}
        self._hidden_params = {}
        self._content = content
        self._tool_calls = tool_calls or []

    def model_dump(self):
        return {
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": self._content,
                        "tool_calls": self._tool_calls,
                    }
                }
            ]
        }


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
        "message",
    ]
    assert result["output"][1]["call_id"] == "call_batch"
    assert "[0:left_click] success" in result["output"][1]["output"]
    assert "[1:type] success" in result["output"][1]["output"]
    assert result["output"][2]["role"] == "user"


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
    assert "failed at member index 0" in result["output"][1]["output"]
    assert "completed=0 skipped=1" in result["output"][1]["output"]


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
    assert result["output"][2]["output"] == "[0:type] success"


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
    assert result["output"][1]["output"] == "[0:left_click] success"


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
    assert "exit_code=0" in result["output"][1]["output"]
    assert "stdout:\nok" in result["output"][1]["output"]


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
    assert any("Wrote 11 characters" in output for output in outputs)
    assert any("Replaced 1 occurrence" in output for output in outputs)
    assert outputs[-1] == "gamma\n"
