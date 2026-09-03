import sys
import types

import pytest

from cua_agent.loops import generic_vlm

PNG_1X1 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII="
)


class MockUsage:
    def model_dump(self):
        return {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}


class MockResponse:
    def __init__(self, content: str):
        self.usage = {}
        self._hidden_params = {}
        self._content = content

    def model_dump(self):
        return {
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": self._content,
                    }
                }
            ]
        }


class MockComputerHandler:
    async def get_dimensions(self):
        return 2000, 1000


@pytest.fixture
def generic_vlm_test_env(monkeypatch):
    qwen_vl_utils = types.ModuleType("qwen_vl_utils")
    qwen_vl_utils.smart_resize = lambda h, w, **kwargs: (h, w)
    monkeypatch.setitem(sys.modules, "qwen_vl_utils", qwen_vl_utils)
    monkeypatch.setattr(generic_vlm, "_build_nous_system", lambda functions: None)
    monkeypatch.setattr(
        generic_vlm.LiteLLMCompletionResponsesConfig,
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


async def run_predict(monkeypatch, content: str, model: str = "openai/test-model", **kwargs):
    async def fake_acompletion(**api_kwargs):
        return MockResponse(content)

    monkeypatch.setattr(generic_vlm.litellm, "acompletion", fake_acompletion)
    return await generic_vlm.GenericVlmConfig().predict_step(
        messages=input_messages(),
        model=model,
        tools=[],
        **kwargs,
    )


@pytest.mark.asyncio
async def test_xml_remains_text_for_non_yutori_model(monkeypatch, generic_vlm_test_env):
    xml = """
    <tool_call>
    <function=computer>
    <parameter=type>type</parameter>
    <parameter=text>https://example.com</parameter>
    </function>
    </tool_call>
    """

    result = await run_predict(monkeypatch, xml)

    assert not any(item.get("type") == "computer_call" for item in result["output"])
    [message] = [item for item in result["output"] if item.get("type") == "message"]
    assert message["content"][0]["text"] == xml


@pytest.mark.asyncio
async def test_yutori_model_auto_parses_xml_to_computer_call(
    monkeypatch,
    generic_vlm_test_env,
):
    xml = """
    I will type the URL.
    <tool_call>
    <function=computer>
    <parameter=type>type</parameter>
    <parameter=text>https://example.com</parameter>
    </function>
    </tool_call>
    """

    result = await run_predict(
        monkeypatch,
        xml,
        model="yutori/yutori-admin/n2os-joint-test",
        computer_handler=MockComputerHandler(),
    )

    messages = [item for item in result["output"] if item.get("type") == "message"]
    computer_calls = [item for item in result["output"] if item.get("type") == "computer_call"]
    assert messages[0]["content"][0]["text"] == "I will type the URL."
    assert computer_calls == [
        {
            "type": "computer_call",
            "call_id": "call_0",
            "action": {"text": "https://example.com", "type": "type"},
            "status": "completed",
        }
    ]


@pytest.mark.asyncio
async def test_yutori_model_auto_parses_malformed_function_equals_opener(
    monkeypatch,
    generic_vlm_test_env,
):
    xml = """
    I will type the URL.
    <tool_call>
    {"function=computer>
    <parameter=action>
    type
    </parameter>
    <parameter=text>
    https://example.com

    </parameter>
    </function>
    </tool_call>
    """

    result = await run_predict(
        monkeypatch,
        xml,
        model="yutori/yutori-admin/n2os-joint-test",
        computer_handler=MockComputerHandler(),
    )

    messages = [item for item in result["output"] if item.get("type") == "message"]
    computer_calls = [item for item in result["output"] if item.get("type") == "computer_call"]
    assert messages[0]["content"][0]["text"] == "I will type the URL."
    assert computer_calls == [
        {
            "type": "computer_call",
            "call_id": "call_0",
            "action": {"text": "https://example.com", "type": "type"},
            "status": "completed",
        }
    ]


@pytest.mark.asyncio
async def test_yutori_model_auto_parses_malformed_json_nested_arguments(
    monkeypatch,
    generic_vlm_test_env,
):
    xml = """
    <tool_call>
    {"name": "computer", "arguments": {"action": "left_click", "arguments": {"coordinate": [153, 65]}}
    </tool_call>
    """

    result = await run_predict(
        monkeypatch,
        xml,
        model="yutori/yutori-admin/n2os-joint-test",
        computer_handler=MockComputerHandler(),
    )

    assert result["output"] == [
        {
            "type": "computer_call",
            "call_id": "call_0",
            "action": {"button": "left", "x": 306, "y": 65, "type": "click"},
            "status": "completed",
        }
    ]


@pytest.mark.asyncio
async def test_yutori_model_auto_parses_implicit_keypress_parameter(
    monkeypatch,
    generic_vlm_test_env,
):
    xml = """
    I will clear the address bar.
    <tool_call>
    <function=computer>
    <parameter=key>
    ["ctrl", "a"]
    </parameter>
    </function>
    </tool_call>
    """

    result = await run_predict(
        monkeypatch,
        xml,
        model="yutori/yutori-admin/n2os-joint-test",
        computer_handler=MockComputerHandler(),
    )

    messages = [item for item in result["output"] if item.get("type") == "message"]
    computer_calls = [item for item in result["output"] if item.get("type") == "computer_call"]
    assert messages[0]["content"][0]["text"] == "I will clear the address bar."
    assert computer_calls == [
        {
            "type": "computer_call",
            "call_id": "call_0",
            "action": {"keys": ["ctrl", "a"], "type": "keypress"},
            "status": "completed",
        }
    ]


@pytest.mark.asyncio
async def test_yutori_model_strips_unexecutable_partial_coordinate_tool_call(
    monkeypatch,
    generic_vlm_test_env,
):
    xml = """
    I will click the address bar.
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

    result = await run_predict(
        monkeypatch,
        xml,
        model="yutori/yutori-admin/n2os-joint-test",
        computer_handler=MockComputerHandler(),
    )

    assert not any(item.get("type") == "computer_call" for item in result["output"])
    [message] = [item for item in result["output"] if item.get("type") == "message"]
    assert message["content"][0]["text"] == "I will click the address bar."


@pytest.mark.asyncio
async def test_yutori_model_emits_message_for_tool_call_only_unexecutable_response(
    monkeypatch,
    generic_vlm_test_env,
):
    xml = """
    <tool_call>
    <function=computer>
    <parameter=triple_click>
    <parameter=coordinate>
    [1116
    </parameter>
    </function>
    </tool_call>
    """

    result = await run_predict(
        monkeypatch,
        xml,
        model="yutori/yutori-admin/n2os-joint-test",
        computer_handler=MockComputerHandler(),
    )

    assert not any(item.get("type") == "computer_call" for item in result["output"])
    assert result["output"] == [
        {
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "Skipped malformed Yutori N2 tool call."}],
        }
    ]


@pytest.mark.asyncio
async def test_yutori_model_auto_expands_computer_batch(monkeypatch, generic_vlm_test_env):
    xml = """
    I will click and type.
    <tool_call>
    <function=computer_batch>
    <parameter=actions>
    [
      {"name": "left_click", "arguments": {"coordinates": [500, 500]}},
      {"name": "type", "arguments": {"text": "done"}}
    ]
    </parameter>
    </function>
    </tool_call>
    """

    result = await run_predict(
        monkeypatch,
        xml,
        model="yutori/yutori-admin/n2os-joint-test",
        computer_handler=MockComputerHandler(),
    )

    messages = [item for item in result["output"] if item.get("type") == "message"]
    computer_calls = [item for item in result["output"] if item.get("type") == "computer_call"]
    assert messages[0]["content"][0]["text"] == "I will click and type."
    assert computer_calls == [
        {
            "type": "computer_call",
            "call_id": "call_0",
            "action": {"button": "left", "x": 1000, "y": 500, "type": "click"},
            "status": "completed",
        },
        {
            "type": "computer_call",
            "call_id": "call_1",
            "action": {"text": "done", "type": "type"},
            "status": "completed",
        },
    ]


@pytest.mark.asyncio
async def test_yutori_model_auto_emits_shell_function_call(
    monkeypatch,
    generic_vlm_test_env,
):
    xml = """
    <tool_call>
    <function=bash>
    <parameter=command>ls -la /tmp</parameter>
    </function>
    </tool_call>
    """

    result = await run_predict(
        monkeypatch,
        xml,
        model="yutori/yutori-admin/n2os-joint-test",
    )

    assert not any(item.get("type") == "computer_call" for item in result["output"])
    function_calls = [item for item in result["output"] if item.get("type") == "function_call"]
    assert function_calls == [
        {
            "type": "function_call",
            "call_id": "call_0",
            "name": "bash",
            "arguments": '{"command": "ls -la /tmp"}',
            "status": "completed",
        }
    ]
