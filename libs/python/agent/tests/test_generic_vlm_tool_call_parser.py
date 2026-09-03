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


async def run_predict(monkeypatch, content: str, **kwargs):
    async def fake_acompletion(**api_kwargs):
        return MockResponse(content)

    monkeypatch.setattr(generic_vlm.litellm, "acompletion", fake_acompletion)
    return await generic_vlm.GenericVlmConfig().predict_step(
        messages=input_messages(),
        model="openai/test-model",
        tools=[],
        **kwargs,
    )


@pytest.mark.asyncio
async def test_xml_remains_text_without_tool_call_parser(monkeypatch, generic_vlm_test_env):
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
async def test_qwen_xml_parser_converts_xml_to_computer_call(monkeypatch, generic_vlm_test_env):
    xml = """
    I will type the URL.
    <tool_call>
    <function=computer>
    <parameter=type>type</parameter>
    <parameter=text>https://example.com</parameter>
    </function>
    </tool_call>
    """

    result = await run_predict(monkeypatch, xml, tool_call_parser="qwen_xml")

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
async def test_qwen_xml_parser_ignores_incomplete_text_action(monkeypatch, generic_vlm_test_env):
    xml = """
    <tool_call>
    <function=computer>
    <parameter=type>text</parameter>
    </function>
    </tool_call>
    """

    result = await run_predict(monkeypatch, xml, tool_call_parser="qwen_xml")

    assert not any(item.get("type") == "computer_call" for item in result["output"])
    [message] = [item for item in result["output"] if item.get("type") == "message"]
    assert message["content"][0]["text"] == xml


@pytest.mark.asyncio
async def test_unknown_tool_call_parser_raises_clear_error():
    with pytest.raises(ValueError, match="Unsupported tool_call_parser"):
        await generic_vlm.GenericVlmConfig().predict_step(
            messages=[],
            model="openai/test-model",
            tools=[],
            tool_call_parser="json",  # type: ignore[arg-type]
        )
