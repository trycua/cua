"""
Novita AI agent loop implementation using litellm with function/tool calling.
Uses Novita AI's OpenAI-compatible endpoint (https://api.novita.ai/openai).
"""

from __future__ import annotations

import os
from typing import Any, Dict, List, Optional, Tuple

import litellm
from litellm.responses.litellm_completion_transformation.transformation import (
    LiteLLMCompletionResponsesConfig,
)

from ..decorators import register_agent
from ..loops.base import AsyncAgentConfig
from ..responses import (
    convert_completion_messages_to_responses_items,
    convert_responses_items_to_completion_messages,
    make_reasoning_item,
)
from ..types import AgentCapability

NOVITA_API_BASE = "https://api.novita.ai/openai"


@register_agent(models=r"novita/.*", priority=0)
class NovitaConfig(AsyncAgentConfig):
    async def predict_step(
        self,
        messages: List[Dict[str, Any]],
        model: str,
        tools: Optional[List[Dict[str, Any]]] = None,
        max_retries: Optional[int] = None,
        stream: bool = False,
        computer_handler=None,
        use_prompt_caching: Optional[bool] = False,
        _on_api_start=None,
        _on_api_end=None,
        _on_usage=None,
        _on_screenshot=None,
        **kwargs,
    ) -> Dict[str, Any]:
        # Strip the "novita/" prefix to get the actual model name for the API
        bare_model = model[len("novita/"):] if model.startswith("novita/") else model

        # Build completion messages from responses items
        converted_msgs = convert_responses_items_to_completion_messages(
            messages,
            allow_images_in_tool_results=False,
        )

        # Build function tools from tools array
        openai_tools = []
        if tools:
            from ..computers import is_agent_computer

            for tool in tools:
                tool_type = tool.get("type")
                if tool_type == "function":
                    function_schema = tool.get("function")
                    if function_schema:
                        openai_tools.append({"type": "function", "function": function_schema})
                elif tool_type == "computer":
                    computer = tool.get("computer")
                    if computer and is_agent_computer(computer):
                        openai_tools.append(
                            {
                                "type": "function",
                                "function": {
                                    "name": "computer",
                                    "description": (
                                        "Use a mouse and keyboard to interact with a computer, "
                                        "and take screenshots."
                                    ),
                                    "parameters": {
                                        "type": "object",
                                        "properties": {
                                            "action": {
                                                "type": "string",
                                                "enum": [
                                                    "key",
                                                    "type",
                                                    "mouse_move",
                                                    "left_click",
                                                    "left_click_drag",
                                                    "right_click",
                                                    "middle_click",
                                                    "double_click",
                                                    "screenshot",
                                                    "scroll",
                                                    "wait",
                                                ],
                                            },
                                            "keys": {
                                                "type": "array",
                                                "items": {"type": "string"},
                                            },
                                            "text": {"type": "string"},
                                            "coordinate": {
                                                "type": "array",
                                                "items": {"type": ["number", "integer"]},
                                                "minItems": 2,
                                                "maxItems": 2,
                                            },
                                            "pixels": {"type": "number"},
                                            "time": {"type": "number"},
                                        },
                                        "required": ["action"],
                                    },
                                },
                            }
                        )

        api_key = kwargs.pop("api_key", None) or os.environ.get("NOVITA_API_KEY")
        api_base = kwargs.pop("api_base", NOVITA_API_BASE)

        api_kwargs: Dict[str, Any] = {
            "model": f"openai/{bare_model}",
            "messages": converted_msgs,
            "max_retries": max_retries,
            "stream": stream,
            "api_base": api_base,
            "api_key": api_key,
            **{k: v for k, v in kwargs.items()},
        }
        if openai_tools:
            api_kwargs["tools"] = openai_tools
        if use_prompt_caching:
            api_kwargs["use_prompt_caching"] = use_prompt_caching

        if _on_api_start:
            await _on_api_start(api_kwargs)

        response = await litellm.acompletion(**api_kwargs)

        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        usage = {
            **LiteLLMCompletionResponsesConfig._transform_chat_completion_usage_to_responses_usage(  # type: ignore
                response.usage
            ).model_dump(),
            "response_cost": response._hidden_params.get("response_cost", 0.0),
        }
        if _on_usage:
            await _on_usage(usage)

        resp_dict = response.model_dump()  # type: ignore
        choice = (resp_dict.get("choices") or [{}])[0]
        message = choice.get("message") or {}
        content_text = message.get("content") or ""
        tool_calls_array = message.get("tool_calls") or []
        reasoning_text = message.get("reasoning") or ""

        output_items: List[Dict[str, Any]] = []

        if reasoning_text:
            output_items.append(make_reasoning_item(reasoning_text))

        if tool_calls_array:
            fake_cm = {
                "role": "assistant",
                "content": content_text if content_text else "",
                "tool_calls": tool_calls_array,
            }
            output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))
        else:
            fake_cm = {"role": "assistant", "content": content_text}
            output_items.extend(convert_completion_messages_to_responses_items([fake_cm]))

        return {"output": output_items, "usage": usage}

    def get_capabilities(self) -> List[AgentCapability]:
        return ["step"]

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """Predict click coordinates using Novita AI via litellm.acompletion."""
        import json

        bare_model = model[len("novita/"):] if model.startswith("novita/") else model

        api_key = kwargs.pop("api_key", None) or os.environ.get("NOVITA_API_KEY")
        api_base = kwargs.pop("api_base", NOVITA_API_BASE)

        click_tool = {
            "type": "function",
            "function": {
                "name": "computer",
                "description": "Click at a coordinate on the screen.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["left_click"]},
                        "coordinate": {
                            "description": "(x, y) pixel coordinates",
                            "type": "array",
                            "items": {"type": ["number", "integer"]},
                            "minItems": 2,
                            "maxItems": 2,
                        },
                    },
                    "required": ["action", "coordinate"],
                },
            },
        }

        messages = [
            {
                "role": "user",
                "content": [
                    {
                        "type": "image_url",
                        "image_url": {"url": f"data:image/png;base64,{image_b64}"},
                    },
                    {"type": "text", "text": instruction},
                ],
            }
        ]

        api_kwargs: Dict[str, Any] = {
            "model": f"openai/{bare_model}",
            "messages": messages,
            "tools": [click_tool],
            "tool_choice": {"type": "function", "function": {"name": "computer"}},
            "api_base": api_base,
            "api_key": api_key,
            **{k: v for k, v in kwargs.items()},
        }
        response = await litellm.acompletion(**api_kwargs)
        resp = response.model_dump()  # type: ignore
        choice = (resp.get("choices") or [{}])[0]
        tool_calls = ((choice.get("message") or {}).get("tool_calls")) or []
        if tool_calls:
            args_str = (tool_calls[0].get("function") or {}).get("arguments", "{}")
            try:
                args = json.loads(args_str)
                coord = args.get("coordinate")
                if isinstance(coord, (list, tuple)) and len(coord) >= 2:
                    return int(coord[0]), int(coord[1])
            except Exception:
                pass
        return None
