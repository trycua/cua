"""
Doubao (火山引擎 Ark) Responses API agent loop implementation.
Based on OpenAI adapter but customized for Doubao's specific API requirements.
"""

import asyncio
import base64
import json
import logging
import time
from io import BytesIO
from typing import Any, Dict, List, Optional, Tuple, Union

import litellm
from PIL import Image

from ..decorators import register_agent
from ..types import AgentCapability, AgentResponse, Messages, Tools
from .openai import _map_computer_tool_to_openai, _prepare_tools_for_openai

logger = logging.getLogger(__name__)


def _normalize_xy(x: int, y: int, width: int, height: int) -> Tuple[int, int]:
    """将物理坐标归一化到 1000x1000 空间"""
    width = max(1, int(width))
    height = max(1, int(height))
    nx = max(0, min(1000, int(round((x / width) * 1000))))
    ny = max(0, min(1000, int(round((y / height) * 1000))))
    return nx, ny


def _denormalize_xy(
    nx: float, ny: float, target_w: int = 1024, target_h: int = 768
) -> Tuple[int, int]:
    """
    将 1000x1000 空间的归一化坐标还原为 Computer Server 的物理坐标系。
    """
    x = int(round((nx / 1000.0) * target_w))
    y = int(round((ny / 1000.0) * target_h))
    return x, y


@register_agent(models=r".*doubao.*", priority=10)
class DoubaoComputerAgentConfig:
    """
    Doubao (火山引擎) agent configuration using litellm responses.
    Specially handles Doubao's 'reasoning' and 'input' field requirements.
    Uses 1000x1000 normalized coordinates for model communication.
    Converts back to 1024x768 target coordinates for Computer Server.
    """

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
        tools = tools or []

        # 1. 获取屏幕物理真实尺寸（用于将模型输出的 1000x1000 还原到物理坐标）
        physical_width, physical_height = 1024, 768
        if computer_handler and hasattr(computer_handler, "get_dimensions"):
            try:
                physical_width, physical_height = await computer_handler.get_dimensions()
                logger.info(
                    f"📏 [物理尺寸] 从 computer_handler 获取到物理分辨率: {physical_width}x{physical_height}"
                )
            except Exception as e:
                logger.warning(f"⚠️ [物理尺寸] 无法获取物理分辨率: {e}")

        # 2. Prepare tools for OpenAI-compatible API
        # 强制告诉模型屏幕是 1000x1000 (归一化空间)
        openai_tools = []
        for schema in tools:
            if schema["type"] == "computer":
                computer_tool = {
                    "type": "function",
                    "name": "computer",
                    "description": (
                        "Use a mouse and keyboard to interact with a computer, and take screenshots.\n"
                        "Screen resolution: 1000x1000 units.\n"
                        "Environment: windows."
                    ),
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "action": {
                                "type": "string",
                                "enum": [
                                    "click",
                                    "double_click",
                                    "right_click",
                                    "type",
                                    "keypress",
                                    "scroll",
                                    "move",
                                    "drag",
                                    "screenshot",
                                    "wait",
                                    "terminate",
                                ],
                            },
                            "x": {"type": "integer", "description": "X coordinate (0-1000)"},
                            "y": {"type": "integer", "description": "Y coordinate (0-1000)"},
                            "text": {"type": "string"},
                            "keys": {"type": "array", "items": {"type": "string"}},
                        },
                        "required": ["action"],
                    },
                }
                openai_tools.append(computer_tool)
            elif schema["type"] == "function":
                func = schema["function"]
                openai_tools.append(
                    {
                        "type": "function",
                        "name": func["name"],
                        "description": func.get("description", ""),
                        "parameters": func.get("parameters", {}),
                    }
                )

        # 3. Call API
        api_kwargs = {
            "model": model,
            "input": messages,
            "tools": openai_tools if openai_tools else None,
            "stream": stream,
            "reasoning": {},
            "num_retries": max_retries,
            **kwargs,
        }

        if _on_api_start:
            await _on_api_start(api_kwargs)
        response = await litellm.aresponses(**api_kwargs)
        if _on_api_end:
            await _on_api_end(api_kwargs, response)

        # 4. 核心转换：将模型输出的 1000x1000 坐标还原回物理坐标
        output_dict = response if isinstance(response, dict) else response.model_dump()
        for item in output_dict.get("output", []):
            if item.get("type") == "function_call" and item.get("name") == "computer":
                args = item.get("arguments", "{}")
                if isinstance(args, str):
                    try:
                        args = json.loads(args)
                    except json.JSONDecodeError as e:
                        logger.warning(f"⚠️ [JSON解析失败] 无法解析工具调用参数: {args}. 错误: {e}")
                        # 尝试简单的清洗：去掉可能存在的 markdown 代码块标记
                        cleaned_args = args.strip()
                        if cleaned_args.startswith("```json"):
                            cleaned_args = cleaned_args[7:]
                        if cleaned_args.endswith("```"):
                            cleaned_args = cleaned_args[:-3]
                        cleaned_args = cleaned_args.strip()

                        try:
                            args = json.loads(cleaned_args)
                        except json.JSONDecodeError:
                            # 如果还是失败，跳过这个 item，让后续的 agent.py 逻辑处理（它也会报错或处理）
                            continue

                if "x" in args and "y" in args:
                    nx, ny = float(args["x"]), float(args["y"])
                    target_x, target_y = _denormalize_xy(nx, ny, physical_width, physical_height)
                    logger.info(
                        f"🎯 [坐标还原] 模型预测({nx}, {ny}) -> 实际物理点击({target_x}, {target_y}) (基于屏幕: {physical_width}x{physical_height})"
                    )
                    args["x"], args["y"] = target_x, target_y
                    item["arguments"] = json.dumps(args)

        # Extract usage and return
        usage = (
            response.get("usage", {}) if isinstance(response, dict) else response.usage.model_dump()
        )
        if hasattr(response, "_hidden_params"):
            usage["response_cost"] = response._hidden_params.get("response_cost", 0.0)
        if _on_usage:
            await _on_usage(usage)
        output_dict["usage"] = usage
        return output_dict

    async def predict_click(
        self, model: str, image_b64: str, instruction: str, computer_handler=None, **kwargs
    ) -> Optional[Tuple[int, int]]:
        """Predict click coordinates specifically for Doubao with 1000x1000 scaling."""
        # 获取真实物理尺寸用于还原
        physical_width, physical_height = 1024, 768
        if computer_handler and hasattr(computer_handler, "get_dimensions"):
            try:
                physical_width, physical_height = await computer_handler.get_dimensions()
                logger.info(
                    f"📏 [物理尺寸] predict_click 识别到物理分辨率: {physical_width}x{physical_height}"
                )
            except Exception as e:
                logger.warning(f"⚠️ [物理尺寸] predict_click 无法获取物理分辨率: {e}")
        else:
            try:
                image_data = base64.b64decode(image_b64)
                image = Image.open(BytesIO(image_data))
                physical_width, physical_height = image.size
                logger.info(
                    f"📏 [物理尺寸] predict_click 回退使用图像分辨率: {physical_width}x{physical_height}"
                )
            except Exception:
                pass

        input_items = [
            {
                "role": "user",
                "content": f"Task: Click {instruction}. Output ONLY a click action on the target element using 1000x1000 coordinate system.",
            },
            {
                "role": "user",
                "content": [
                    {"type": "input_image", "image_url": f"data:image/png;base64,{image_b64}"}
                ],
            },
        ]

        computer_tool = {
            "type": "function",
            "name": "computer",
            "description": "Screen resolution: 1000x1000 units.",
            "parameters": {
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["click"]},
                    "x": {"type": "integer"},
                    "y": {"type": "integer"},
                },
                "required": ["action", "x", "y"],
            },
        }

        api_kwargs = {
            "model": model,
            "input": input_items,
            "tools": [computer_tool],
            "stream": False,
            "reasoning": {},
            "max_tokens": 200,
            **kwargs,
        }

        response = await litellm.aresponses(**api_kwargs)
        output_dict = response if isinstance(response, dict) else response.model_dump()

        for item in output_dict.get("output", []):
            if item.get("type") == "function_call" and item.get("name") == "computer":
                args = item.get("arguments", "{}")
                if isinstance(args, str):
                    args = json.loads(args)
                if args.get("x") is not None and args.get("y") is not None:
                    nx, ny = float(args["x"]), float(args["y"])
                    target_x, target_y = _denormalize_xy(nx, ny, physical_width, physical_height)
                    logger.info(
                        f"🎯 [点击还原] 模型({nx}, {ny}) -> 实际物理({target_x}, {target_y})"
                    )
                    return (target_x, target_y)
        return None

    def get_capabilities(self) -> List[AgentCapability]:
        return ["click", "step"]
