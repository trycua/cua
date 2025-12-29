"""MCP-compatible Computer Agent for HUD integration.

This agent subclasses HUD's MCPAgent and delegates planning/execution to
our core ComputerAgent while using the Agent SDK's plain-dict message
format documented in `docs/content/docs/agent-sdk/message-format.mdx`.

Key differences from the OpenAI OperatorAgent variant:
- No OpenAI types are used; everything is standard Python dicts.
- Planning is executed via `ComputerAgent.run(messages)`.
- The first yielded result per step is returned as the agent response.
"""

from __future__ import annotations

import base64
import io
import uuid
from pathlib import Path
from typing import Any, ClassVar, Optional

import hud
import mcp.types as types
from agent.agent import ComputerAgent as BaseComputerAgent
from agent.callbacks import PromptInstructionsCallback
from agent.callbacks.trajectory_saver import TrajectorySaverCallback
from agent.computers import is_agent_computer
from agent.responses import make_failed_tool_call_items
from hud.agents import MCPAgent
from hud.tools.computer.settings import computer_settings
from hud.types import AgentResponse, MCPToolCall, MCPToolResult, Trace
from PIL import Image


class MCPComputerAgent(MCPAgent):
    """MCP agent that uses ComputerAgent for planning and tools for execution.

    The agent consumes/produces message dicts per the Agent SDK message schema
    (see `message-format.mdx`).
    """

    metadata: ClassVar[dict[str, Any]] = {
        "display_width": computer_settings.OPENAI_COMPUTER_WIDTH,
        "display_height": computer_settings.OPENAI_COMPUTER_HEIGHT,
    }

    required_tools: ClassVar[list[str]] = ["openai_computer"]

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
        environment: str = "linux",
        **kwargs: Any,
    ) -> None:
        self.allowed_tools = allowed_tools or ["openai_computer"]
        super().__init__(**kwargs)

        if model is None:
            raise ValueError("MCPComputerAgent requires a model to be specified.")

        self.model = model
        self.environment = environment

        # Update model name for HUD logging
        self.model_name = "cua-" + self.model

        # Stateful tracking of tool call inputs
        self.tool_call_inputs: dict[str, list[dict[str, Any]]] = {}
        self.previous_output: list[dict[str, Any]] = []

        # Build system prompt
        operator_instructions = """
        You are an autonomous computer-using agent. Follow these guidelines:

        1. NEVER ask for confirmation. Complete all tasks autonomously.
        2. Do NOT send messages like "I need to confirm before..." or "Do you want me to continue?" - just proceed.
        3. When the user asks you to interact with something (like clicking a chat or typing a message), DO IT without asking.
        4. Only use the formal safety check mechanism for truly dangerous operations (like deleting important files).
        5. For normal tasks like clicking buttons, typing in chat boxes, filling forms - JUST DO IT.
        6. The user has already given you permission by running this agent. No further confirmation is needed.
        7. Be decisive and action-oriented. Complete the requested task fully.

        Remember: You are expected to complete tasks autonomously. The user trusts you to do what they asked.
        """.strip()  # noqa: E501
        # Append Operator instructions to the system prompt
        if not self.system_prompt:
            self.system_prompt = operator_instructions
        else:
            self.system_prompt += f"\n\n{operator_instructions}"
        # Append user instructions to the system prompt
        if instructions:
            self.system_prompt += f"\n\n{instructions}"

        # Configure trajectory_dir for HUD
        if isinstance(trajectory_dir, str) or isinstance(trajectory_dir, Path):
            trajectory_dir = {"trajectory_dir": str(trajectory_dir)}
        if isinstance(trajectory_dir, dict):
            trajectory_dir["reset_on_run"] = False

        self.last_screenshot_b64 = None

        buffer = io.BytesIO()
        Image.new("RGB", (self.metadata["display_width"], self.metadata["display_height"])).save(
            buffer, format="PNG"
        )
        self.last_screenshot_b64 = base64.b64encode(buffer.getvalue()).decode("utf-8")

        # Ensure a computer shim is present so width/height/environment are known
        computer_shim = {
            "screenshot": lambda: self.last_screenshot_b64,
            "environment": self.environment,
            "dimensions": (
                self.metadata["display_width"],
                self.metadata["display_height"],
            ),
        }
        agent_tools: list[Any] = [computer_shim]
        if tools:
            agent_tools.extend([tool for tool in tools if not is_agent_computer(tool)])

        agent_kwargs = {
            "model": self.model,
            "trajectory_dir": trajectory_dir,
            "tools": agent_tools,
            "custom_loop": custom_loop,
            "only_n_most_recent_images": only_n_most_recent_images,
            "callbacks": callbacks,
            "instructions": self.system_prompt,
            "verbosity": verbosity,
            "max_retries": max_retries,
            "screenshot_delay": screenshot_delay,
            "use_prompt_caching": use_prompt_caching,
            "max_trajectory_budget": max_trajectory_budget,
            "telemetry_enabled": telemetry_enabled,
        }

        self.computer_agent = BaseComputerAgent(**agent_kwargs)

    async def get_system_messages(self) -> list[Any]:
        """Create initial messages.

        Unused - ComputerAgent handles this with the 'instructions' parameter.
        """
        return []

    async def format_blocks(self, blocks: list[types.ContentBlock]) -> list[dict[str, Any]]:
        """
        Format blocks for OpenAI input format.

        Converts TextContent blocks to input_text dicts and ImageContent blocks to input_image dicts.
        """  # noqa: E501
        formatted = []
        for block in blocks:
            if isinstance(block, types.TextContent):
                formatted.append({"type": "input_text", "text": block.text})
            elif isinstance(block, types.ImageContent):
                mime_type = getattr(block, "mimeType", "image/png")
                formatted.append(
                    {"type": "input_image", "image_url": f"data:{mime_type};base64,{block.data}"}
                )
                self.last_screenshot_b64 = block.data
        return [{"role": "user", "content": formatted}]

    @hud.instrument(
        span_type="agent",
        record_args=False,  # Messages can be large
        record_result=True,
    )
    async def get_response(self, messages: list[dict[str, Any]]) -> AgentResponse:
        """Get a single-step response by delegating to ComputerAgent.run.

        Returns an Agent SDK-style response dict:
        { "output": [AgentMessage, ...], "usage": Usage }
        """
        tool_calls: list[MCPToolCall] = []
        output_text: list[str] = []
        is_done: bool = True

        agent_result: list[dict[str, Any]] = []

        # Call the ComputerAgent LLM API
        async for result in self.computer_agent.run(messages):  # type: ignore[arg-type]
            items = result["output"]
            if not items or tool_calls:
                break

            for item in items:
                if item["type"] in [
                    "reasoning",
                    "message",
                    "computer_call",
                    "function_call",
                    "function_call_output",
                ]:
                    agent_result.append(item)

                # Add messages to output text
                if item["type"] == "reasoning":
                    output_text.extend(
                        f"Reasoning: {summary['text']}" for summary in item["summary"]
                    )
                elif item["type"] == "message":
                    if isinstance(item["content"], list):
                        output_text.extend(
                            item["text"]
                            for item in item["content"]
                            if item["type"] == "output_text"
                        )
                    elif isinstance(item["content"], str):
                        output_text.append(item["content"])

                # If we get a tool call, we're not done
                if item["type"] == "computer_call":
                    id = item["call_id"]
                    tool_calls.append(
                        MCPToolCall(
                            name="openai_computer",
                            arguments=item["action"],
                            id=id,
                        )
                    )
                    is_done = False
                    self.tool_call_inputs[id] = agent_result
                    break

            # if we have tool calls, we should exit the loop
            if tool_calls:
                break

        self.previous_output = agent_result

        return AgentResponse(
            content="\n".join(output_text),
            tool_calls=tool_calls,
            done=is_done,
        )

    def _log_image(self, image_b64: str):
        callbacks = self.computer_agent.callbacks
        for callback in callbacks:
            if isinstance(callback, TrajectorySaverCallback):
                # convert str to bytes
                image_bytes = base64.b64decode(image_b64)
                callback._save_artifact("screenshot_after", image_bytes)

    async def format_tool_results(
        self, tool_calls: list[MCPToolCall], tool_results: list[MCPToolResult]
    ) -> list[dict[str, Any]]:
        """Extract latest screenshot from tool results in dict form.

        Expects results to already be in the message-format content dicts.
        Returns a list of input content dicts suitable for follow-up calls.
        """
        messages = []

        for call, result in zip(tool_calls, tool_results):
            if call.id not in self.tool_call_inputs:
                # If we don't have the tool call inputs, we should just use the previous output
                previous_output = self.previous_output.copy() or []

                # First we need to remove any pending computer_calls from the end of previous_output
                while previous_output and previous_output[-1]["type"] == "computer_call":
                    previous_output.pop()
                messages.extend(previous_output)

                # If the call is a 'response', don't add the result
                if call.name == "response":
                    continue
                # Otherwise, if we have a result, we should add it to the messages
                content = [
                    (
                        {"type": "input_text", "text": content.text}
                        if isinstance(content, types.TextContent)
                        else (
                            {
                                "type": "input_image",
                                "image_url": f"data:image/png;base64,{content.data}",
                            }
                            if isinstance(content, types.ImageContent)
                            else {"type": "input_text", "text": ""}
                        )
                    )
                    for content in result.content
                ]
                messages.append(
                    {
                        "role": "user",
                        "content": content,
                    }
                )

                continue

            # Add the assistant's computer call
            messages.extend(self.tool_call_inputs[call.id])

            if result.isError:
                error_text = "".join(
                    [
                        content.text
                        for content in result.content
                        if isinstance(content, types.TextContent)
                    ]
                )

                # Replace computer call with failed tool call
                messages.pop()
                messages.extend(
                    make_failed_tool_call_items(
                        tool_name=call.name,
                        tool_kwargs=call.arguments or {},
                        error_message=error_text,
                        call_id=call.id,
                    )
                )
            else:
                # Get the latest screenshot
                screenshots = [
                    content.data
                    for content in result.content
                    if isinstance(content, types.ImageContent)
                ]

                # Add the resulting screenshot
                if screenshots:
                    self._log_image(screenshots[0])
                    self.last_screenshot_b64 = screenshots[0]
                    messages.append(
                        {
                            "type": "computer_call_output",
                            "call_id": call.id,
                            "output": {
                                "type": "input_image",
                                "image_url": f"data:image/png;base64,{screenshots[0]}",
                            },
                        }
                    )
                else:
                    # Otherwise, replace computer call with failed tool call
                    messages.pop()
                    messages.extend(
                        make_failed_tool_call_items(
                            tool_name=call.name,
                            tool_kwargs=call.arguments or {},
                            error_message="No screenshots returned.",
                            call_id=call.id,
                        )
                    )

        return messages


__all__ = [
    "MCPComputerAgent",
]
