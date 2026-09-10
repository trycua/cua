"""MiniMax-M3 computer-use loop using native multimodal function calling."""

from __future__ import annotations

import base64
import json
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)
from PIL import Image

from ..decorators import register_agent
from ..responses import (
    convert_completion_messages_to_responses_items,
    convert_responses_items_to_completion_messages,
)
from ..types import AgentCapability
from .openai import _map_computer_tool_to_openai

_SCREENSHOT_NOTICE = "Taking a screenshot to see the current computer screen."


def _has_image(messages: List[Dict[str, Any]]) -> bool:
    for message in messages:
        content = message.get("content")
        if not isinstance(content, list):
            continue
        if any(isinstance(part, dict) and part.get("type") == "image_url" for part in content):
            return True
    return False


def _has_screenshot_notice(messages: List[Dict[str, Any]]) -> bool:
    for message in messages:
        content = message.get("content")
        if isinstance(content, str) and _SCREENSHOT_NOTICE in content:
            return True
        if isinstance(content, list):
            for part in content:
                if isinstance(part, dict) and _SCREENSHOT_NOTICE in (part.get("text") or ""):
                    return True
    return False


def _as_chat_completion_tool(tool: Dict[str, Any]) -> Dict[str, Any]:
    return {
        "type": "function",
        "function": {
            "name": tool["name"],
            "description": tool.get("description", ""),
            "parameters": tool.get("parameters", {}),
        },
    }


async def _prepare_chat_tools(tool_schemas: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    tools: List[Dict[str, Any]] = []
    for schema in tool_schemas:
        if schema["type"] == "computer":
            tool = await _map_computer_tool_to_openai(schema["computer"], use_native_tool=False)
            tools.append(
                _as_chat_completion_tool(
                    {
                        "name": tool["name"],
                        "description": tool.get("description", ""),
                        "parameters": tool.get("parameters", {}),
                    }
                )
            )
        elif schema["type"] == "function":
            tools.append(_as_chat_completion_tool(schema["function"]))
    return tools


def _usage_from_response(response: Any) -> Dict[str, Any]:
    usage = LiteLLMCompletionResponsesConfig._transform_chat_completion_usage_to_responses_usage(
        response.usage
    ).model_dump()
    hidden_params = getattr(response, "_hidden_params", {}) or {}
    usage["response_cost"] = hidden_params.get("response_cost", 0.0)
    return usage


class _StaticComputer:
    def __init__(self, width: int, height: int):
        self.width = width
        self.height = height

    async def get_dimensions(self) -> Tuple[int, int]:
        return self.width, self.height

    async def get_environment(self) -> str:
        return "computer"


@register_agent(models=r"(?i)(?:.*/)?MiniMax-M3$")
class MiniMaxComputerUseConfig:
    """Run MiniMax-M3 with screenshots and standard function tools."""

    async def predict_step(
        self,
        messages: List[Dict[str, Any]],
        model: str,
        tools: Optional[List[Dict[str, Any]]] = None,
        max_retries: Optional[int] = None,
        stream: bool = False,
        computer_handler: Any = None,
        use_prompt_caching: Optional[bool] = False,
        _on_api_start: Any = None,
        _on_api_end: Any = None,
        _on_usage: Any = None,
        _on_screenshot: Any = None,
        **kwargs: Any,
    ) -> Dict[str, Any]:
        completion_messages = convert_responses_items_to_completion_messages(
            messages,
            allow_images_in_tool_results=False,
        )

        pre_output_items: List[Dict[str, Any]] = []
        if not _has_image(completion_messages):
            if computer_handler is None or not hasattr(computer_handler, "screenshot"):
                raise RuntimeError(
                    "No screenshots present and computer_handler.screenshot is not available."
                )
            screenshot_b64 = await computer_handler.screenshot()
            if not screenshot_b64:
                raise RuntimeError("Failed to capture screenshot from computer_handler.")
            completion_messages.append(
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": f"data:image/png;base64,{screenshot_b64}",
                                "detail": "default",
                            },
                        },
                        {"type": "text", "text": "Current screen"},
                    ],
                }
            )
            if not _has_screenshot_notice(messages):
                pre_output_items.append(
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "text", "text": _SCREENSHOT_NOTICE}],
                    }
                )

        chat_tools = await _prepare_chat_tools(tools or [])
        api_kwargs: Dict[str, Any] = {
            "model": model,
            "messages": completion_messages,
            "max_retries": max_retries,
            "stream": stream,
            **kwargs,
        }
        if chat_tools:
            api_kwargs["tools"] = chat_tools
        if use_prompt_caching:
            api_kwargs["use_prompt_caching"] = True

        if _on_api_start:
            await _on_api_start(api_kwargs)
        response = await litellm.acompletion(**api_kwargs)
        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        usage = _usage_from_response(response)
        if _on_usage:
            await _on_usage(usage)

        response_dict = response.model_dump()
        message = (response_dict.get("choices") or [{}])[0].get("message") or {}
        assistant_message = {
            "role": "assistant",
            "content": message.get("content") or "",
            "tool_calls": message.get("tool_calls") or [],
        }
        output_items = convert_completion_messages_to_responses_items([assistant_message])
        return {"output": pre_output_items + output_items, "usage": usage}

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs: Any
    ) -> Optional[Tuple[int, int]]:
        try:
            image = Image.open(BytesIO(base64.b64decode(image_b64)))
            width, height = image.size
        except Exception:
            width, height = 1024, 768

        responses_tool = await _map_computer_tool_to_openai(
            _StaticComputer(width, height), use_native_tool=False
        )
        responses_tool["parameters"]["properties"]["action"]["enum"] = ["click"]
        responses_tool["parameters"]["required"] = ["action", "x", "y"]
        chat_tool = _as_chat_completion_tool(responses_tool)

        response = await litellm.acompletion(
            model=model,
            messages=[
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": instruction},
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": f"data:image/png;base64,{image_b64}",
                                "detail": "default",
                            },
                        },
                    ],
                }
            ],
            tools=[chat_tool],
            **kwargs,
        )
        response_dict = response.model_dump()
        message = (response_dict.get("choices") or [{}])[0].get("message") or {}
        tool_calls = message.get("tool_calls") or []
        if not tool_calls:
            return None

        arguments = tool_calls[0].get("function", {}).get("arguments", "{}")
        if isinstance(arguments, str):
            try:
                arguments = json.loads(arguments)
            except json.JSONDecodeError:
                return None
        if not isinstance(arguments, dict):
            return None

        x, y = arguments.get("x"), arguments.get("y")
        if x is None or y is None:
            return None
        return int(x), int(y)

    def get_capabilities(self) -> List[AgentCapability]:
        return ["click", "step"]
