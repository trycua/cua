"""HUD ComputerAgent wrapper and Fake AsyncOpenAI client.

Provides FakeAsyncOpenAI that adapts our ComputerAgent to the OpenAI Responses
interface needed by HUD's OperatorAgent. It implements only `responses.create`
and returns an OpenAI Response object with `id` and `output` fields, where `output` is a list of
OpenAI-like response blocks. We intentionally only support a single-step call
by consuming the first yielded result from `ComputerAgent.run()`.
"""

import time
import traceback
import uuid
from typing import Any, Dict, List, Optional

from agent.agent import ComputerAgent as BaseComputerAgent
from agent.callbacks import PromptInstructionsCallback
from hud.agents import OperatorAgent
from hud.tools.computer.settings import computer_settings

# OpenAI Responses typed models (required)
from openai.types.responses import (
    Response,
    ResponseComputerToolCall,
    ResponseInputParam,
    ResponseOutputItem,
    ResponseOutputMessage,
    ResponseOutputText,
    ResponseReasoningItem,
    ResponseUsage,
)
from PIL import Image


def _map_agent_output_to_openai_blocks(
    output_items: List[Dict[str, Any]],
) -> List[ResponseOutputItem]:
    """Map our agent output items to OpenAI ResponseOutputItem typed models.

    Only a subset is supported: computer_call, assistant message (text), and reasoning.
    Unknown types are ignored.
    """
    blocks: List[ResponseOutputItem] = []
    for item in output_items or []:
        t = item.get("type")
        if t == "computer_call":
            comp = ResponseComputerToolCall.model_validate(
                {
                    "id": item.get("id") or f"cu_{uuid.uuid4().hex}",
                    "type": "computer_call",
                    "call_id": item["call_id"],
                    "action": item["action"],
                    "pending_safety_checks": item.get("pending_safety_checks", []),
                    "status": "completed",
                }
            )
            blocks.append(comp)
            # we will exit early here as the responses api only supports a single step
            break
        elif t == "message" and item.get("role") == "assistant":
            content_blocks: List[ResponseOutputText] = []
            for c in item.get("content", []) or []:
                content_blocks.append(
                    ResponseOutputText.model_validate(
                        {
                            "type": "output_text",
                            "text": c["text"],
                            "annotations": [],
                        }
                    )
                )
            if content_blocks:
                msg = ResponseOutputMessage.model_validate(
                    {
                        "id": item.get("id") or f"msg_{uuid.uuid4()}",
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "content": [ct.model_dump() for ct in content_blocks],
                    }
                )
                blocks.append(msg)
        elif t == "reasoning":
            reasoning = ResponseReasoningItem.model_validate(
                {
                    "id": item.get("id") or f"rsn_{uuid.uuid4()}",
                    "type": "reasoning",
                    "summary": item["summary"],
                }
            )
            blocks.append(reasoning)
        # Unhandled types are ignored
    return blocks


def _to_plain_dict_list(items: Any) -> List[Dict[str, Any]]:
    out: List[Dict[str, Any]] = []
    for it in list(items):
        if hasattr(it, "model_dump"):
            out.append(it.model_dump())  # type: ignore[attr-defined]
        elif isinstance(it, dict):
            out.append(it)
        else:
            # Strict: rely on default __dict__ if present
            out.append(dict(it))  # may raise if not mapping
    return out


class FakeAsyncOpenAI:
    """Minimal fake OpenAI client with only `responses.create` implemented.

    It uses a provided `ComputerAgent` instance to produce a single-step
    response compatible with HUD's OperatorAgent loop.
    """

    def __init__(self, computer_agent: BaseComputerAgent) -> None:
        self._agent = computer_agent
        self.responses = self._Responses(self)

    class _Responses:
        def __init__(self, parent: "FakeAsyncOpenAI") -> None:
            # Caches for cross-call context when using previous_response_id
            self.blocks_cache: Dict[str, ResponseInputParam | ResponseOutputItem] = {}
            self.context_cache: Dict[str, List[str]] = {}
            self.agent = parent._agent

        async def create(
            self,
            *,
            model: str,
            input: ResponseInputParam,
            tools: Optional[List[Dict[str, Any]]] = None,
            instructions: Optional[str] = None,
            previous_response_id: Optional[str] = None,
            max_retries: int = 5,
            **_: Any,
        ) -> Any:
            for attempt in range(max_retries):
                # Prepend cached blocks from previous_response_id to input
                full_input = input
                if previous_response_id is not None:
                    prev_block_ids = self.context_cache[previous_response_id]
                    prev_blocks = [self.blocks_cache[b_id] for b_id in prev_block_ids]
                    full_input = _to_plain_dict_list(prev_blocks + input)

                # Pre-pend instructions message
                effective_input = full_input
                if instructions:
                    effective_input = [
                        {
                            "role": "user",
                            "content": instructions,
                        }
                    ] + full_input

                # Run a single iteration of the ComputerAgent
                agent_result: Optional[Dict[str, Any]] = None
                async for result in self.agent.run(effective_input):  # type: ignore[arg-type]
                    agent_result = result
                    break
                assert agent_result is not None, "Agent failed to produce result"

                output = _map_agent_output_to_openai_blocks(agent_result["output"])
                usage = agent_result["usage"]

                # Cache conversation context using the last response id
                block_ids: List[str] = []
                blocks_to_cache = full_input + output
                for b in blocks_to_cache:
                    bid = getattr(b, "id", None) or f"tmp-{hash(repr(b))}"
                    self.blocks_cache[bid] = b  # type: ignore[assignment]
                    block_ids.append(bid)
                response_id = agent_result.get("id") or f"fake-{int(time.time()*1000)}"
                self.context_cache[response_id] = block_ids

                try:
                    return Response.model_validate(
                        {
                            "id": response_id,
                            "created_at": time.time(),
                            "object": "response",
                            "model": model,
                            "output": output,
                            "parallel_tool_calls": False,
                            "tool_choice": "auto",
                            "tools": [],
                            "previous_response_id": previous_response_id,
                            "usage": ResponseUsage.model_validate(
                                {
                                    "input_tokens": usage.get("input_tokens", 0),
                                    "output_tokens": usage.get("output_tokens", 0),
                                    "total_tokens": usage.get("total_tokens", 0),
                                    "input_tokens_details": usage.get(
                                        "input_tokens_details", {"cached_tokens": 0}
                                    ),
                                    "output_tokens_details": usage.get(
                                        "output_tokens_details", {"reasoning_tokens": 0}
                                    ),
                                }
                            ),
                        }
                    )
                except Exception as e:
                    print(
                        f"Error while validating agent response (attempt {attempt + 1}/{max_retries}): ",
                        e,
                    )
                    if attempt == max_retries - 1:
                        print(traceback.format_exc())
                        raise e


# ---------------------------------------------------------------------------
# Proxy OperatorAgent (moved from __init__.py)
# ---------------------------------------------------------------------------


class ProxyOperatorAgent(OperatorAgent):
    """OperatorAgent that proxies model calls through our ComputerAgent.

    Accepts the same config keys we pass via hud.run_dataset `agent_config`:
    - model: str | None
    - allowed_tools: list[str] | None
    Additional kwargs are forwarded to OperatorAgent (if any are supported).
    """

    def __init__(
        self,
        *,
        model: str | None = None,
        allowed_tools: list[str] | None = None,
        trajectory_dir: str | dict | None = None,
        # === ComputerAgent kwargs ===
        tools: list[Any] | None = None,
        custom_loop: Any | None = None,
        only_n_most_recent_images: int | None = None,
        callbacks: list[Any] | None = None,
        instructions: str | None = None,
        verbosity: int | None = None,
        max_retries: int | None = 3,
        screenshot_delay: float | int = 0.5,
        use_prompt_caching: bool | None = False,
        max_trajectory_budget: float | dict | None = None,
        telemetry_enabled: bool | None = True,
        **kwargs: Any,
    ) -> None:
        model = model or "computer-use-preview"
        allowed_tools = allowed_tools or ["openai_computer"]

        computer_shim = {
            "screenshot": lambda: Image.new(
                "RGB",
                (computer_settings.OPENAI_COMPUTER_WIDTH, computer_settings.OPENAI_COMPUTER_HEIGHT),
            ),
            "environment": "linux",
            "dimensions": (
                computer_settings.OPENAI_COMPUTER_WIDTH,
                computer_settings.OPENAI_COMPUTER_HEIGHT,
            ),
        }
        # Build tools ensuring the computer_shim is included
        agent_tools: list[Any] = [computer_shim]
        if tools:
            agent_tools.extend(tools)

        # Build callbacks, injecting prompt instructions if provided
        agent_callbacks = list(callbacks or [])
        if instructions:
            agent_callbacks.append(PromptInstructionsCallback(instructions))

        computer_agent = BaseComputerAgent(
            model=model,
            tools=agent_tools,
            custom_loop=custom_loop,
            only_n_most_recent_images=only_n_most_recent_images,
            callbacks=agent_callbacks,
            verbosity=verbosity,
            trajectory_dir=trajectory_dir,
            max_retries=max_retries,
            screenshot_delay=screenshot_delay,
            use_prompt_caching=use_prompt_caching,
            max_trajectory_budget=max_trajectory_budget,
            telemetry_enabled=telemetry_enabled,
        )
        model_client = FakeAsyncOpenAI(computer_agent)

        super().__init__(
            model_client=model_client,  # type: ignore[arg-type]
            model=model,
            allowed_tools=allowed_tools,
            **kwargs,
        )


__all__ = [
    "FakeAsyncOpenAI",
    "ProxyOperatorAgent",
]
