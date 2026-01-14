"""Gemini Agent implementation using Google's Gemini API with Computer Use capabilities."""

import os
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict, List, Optional

from . import register_agent
from .base import AgentResult, BaseAgent, FailureMode

if TYPE_CHECKING:
    from ..computers import DesktopSession


@register_agent("gemini")
class GeminiAgent(BaseAgent):
    """Agent implementation using Google's Gemini API with Computer Use."""

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.model = kwargs.get("model", "gemini-3-pro-preview")
        self.api_key = kwargs.get("api_key", os.getenv("GOOGLE_API_KEY"))
        self.thinking_level = kwargs.get("thinking_level", "low")
        self.media_resolution = kwargs.get("media_resolution", "high")
        self.max_steps = kwargs.get("max_steps", 50)

    @staticmethod
    def name() -> str:
        return "gemini"

    def _lazy_import_genai(self):
        """Import google.genai lazily to avoid hard dependency unless used."""
        try:
            from google import genai
            from google.genai import types

            return genai, types
        except Exception as e:
            raise RuntimeError(
                "google.genai is required for the Gemini agent. "
                "Install it with: pip install google-genai"
            ) from e

    def _build_custom_function_declarations(self, types: Any) -> List[Any]:
        """
        Build custom function declarations for Gemini models.
        These map to cua_bench action types.
        """
        return [
            # Click actions
            types.FunctionDeclaration(
                name="click",
                description="Click at the specified x,y coordinates on the screen.",
                parameters={
                    "type": "object",
                    "properties": {
                        "x": {"type": "integer", "description": "X coordinate in pixels"},
                        "y": {"type": "integer", "description": "Y coordinate in pixels"},
                    },
                    "required": ["x", "y"],
                },
            ),
            types.FunctionDeclaration(
                name="right_click",
                description="Right-click at the specified x,y coordinates.",
                parameters={
                    "type": "object",
                    "properties": {
                        "x": {"type": "integer", "description": "X coordinate in pixels"},
                        "y": {"type": "integer", "description": "Y coordinate in pixels"},
                    },
                    "required": ["x", "y"],
                },
            ),
            types.FunctionDeclaration(
                name="double_click",
                description="Double-click at the specified x,y coordinates.",
                parameters={
                    "type": "object",
                    "properties": {
                        "x": {"type": "integer", "description": "X coordinate in pixels"},
                        "y": {"type": "integer", "description": "Y coordinate in pixels"},
                    },
                    "required": ["x", "y"],
                },
            ),
            types.FunctionDeclaration(
                name="middle_click",
                description="Middle-click at the specified x,y coordinates.",
                parameters={
                    "type": "object",
                    "properties": {
                        "x": {"type": "integer", "description": "X coordinate in pixels"},
                        "y": {"type": "integer", "description": "Y coordinate in pixels"},
                    },
                    "required": ["x", "y"],
                },
            ),
            # Type and keyboard actions
            types.FunctionDeclaration(
                name="type_text",
                description="Type the specified text.",
                parameters={
                    "type": "object",
                    "properties": {
                        "text": {"type": "string", "description": "Text to type"},
                    },
                    "required": ["text"],
                },
            ),
            types.FunctionDeclaration(
                name="key",
                description="Press a single key (e.g., 'enter', 'escape', 'tab', 'backspace').",
                parameters={
                    "type": "object",
                    "properties": {
                        "key": {"type": "string", "description": "Key to press"},
                    },
                    "required": ["key"],
                },
            ),
            types.FunctionDeclaration(
                name="hotkey",
                description="Press a key combination (e.g., ['ctrl', 'c'] for copy, ['ctrl', 'v'] for paste).",
                parameters={
                    "type": "object",
                    "properties": {
                        "keys": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Array of keys to press together",
                        },
                    },
                    "required": ["keys"],
                },
            ),
            # Mouse movement and drag
            types.FunctionDeclaration(
                name="move_to",
                description="Move the mouse cursor to the specified x,y coordinates without clicking.",
                parameters={
                    "type": "object",
                    "properties": {
                        "x": {"type": "integer", "description": "X coordinate in pixels"},
                        "y": {"type": "integer", "description": "Y coordinate in pixels"},
                        "duration": {
                            "type": "number",
                            "description": "Duration of movement in seconds (default 0.0)",
                        },
                    },
                    "required": ["x", "y"],
                },
            ),
            types.FunctionDeclaration(
                name="drag",
                description="Drag from one coordinate to another.",
                parameters={
                    "type": "object",
                    "properties": {
                        "from_x": {
                            "type": "integer",
                            "description": "Starting X coordinate in pixels",
                        },
                        "from_y": {
                            "type": "integer",
                            "description": "Starting Y coordinate in pixels",
                        },
                        "to_x": {
                            "type": "integer",
                            "description": "Destination X coordinate in pixels",
                        },
                        "to_y": {
                            "type": "integer",
                            "description": "Destination Y coordinate in pixels",
                        },
                        "duration": {
                            "type": "number",
                            "description": "Duration of drag in seconds (default 1.0)",
                        },
                    },
                    "required": ["from_x", "from_y", "to_x", "to_y"],
                },
            ),
            # Scroll actions
            types.FunctionDeclaration(
                name="scroll",
                description="Scroll in the specified direction.",
                parameters={
                    "type": "object",
                    "properties": {
                        "direction": {
                            "type": "string",
                            "enum": ["up", "down"],
                            "description": "Direction to scroll",
                        },
                        "amount": {
                            "type": "integer",
                            "description": "Amount to scroll in pixels (default 100)",
                        },
                    },
                    "required": ["direction"],
                },
            ),
            # Control actions
            types.FunctionDeclaration(
                name="wait",
                description="Wait for the specified number of seconds before the next action.",
                parameters={
                    "type": "object",
                    "properties": {
                        "seconds": {
                            "type": "number",
                            "description": "Number of seconds to wait (default 1.0)",
                        },
                    },
                },
            ),
            types.FunctionDeclaration(
                name="done",
                description="Indicate that the task has been completed successfully.",
                parameters={
                    "type": "object",
                    "properties": {},
                },
            ),
        ]

    async def _map_function_call_to_action(
        self,
        function_name: str,
        args: Dict[str, Any],
        session: "DesktopSession",
    ) -> Optional[Any]:
        """
        Map a Gemini function call to a cua_bench action and execute it.

        Returns the action object if successful, None otherwise.
        """
        from ..types import (
            ClickAction,
            DoneAction,
            DoubleClickAction,
            DragAction,
            HotkeyAction,
            KeyAction,
            MiddleClickAction,
            MoveToAction,
            RightClickAction,
            ScrollAction,
            TypeAction,
            WaitAction,
        )

        action = None

        # Click actions
        if function_name == "click":
            action = ClickAction(x=int(args["x"]), y=int(args["y"]))
        elif function_name == "right_click":
            action = RightClickAction(x=int(args["x"]), y=int(args["y"]))
        elif function_name == "double_click":
            action = DoubleClickAction(x=int(args["x"]), y=int(args["y"]))
        elif function_name == "middle_click":
            action = MiddleClickAction(x=int(args["x"]), y=int(args["y"]))

        # Type and keyboard actions
        elif function_name == "type_text":
            action = TypeAction(text=args["text"])
        elif function_name == "key":
            action = KeyAction(key=args["key"])
        elif function_name == "hotkey":
            action = HotkeyAction(keys=args["keys"])

        # Mouse movement and drag
        elif function_name == "move_to":
            duration = args.get("duration", 0.0)
            action = MoveToAction(x=int(args["x"]), y=int(args["y"]), duration=float(duration))
        elif function_name == "drag":
            duration = args.get("duration", 1.0)
            action = DragAction(
                from_x=int(args["from_x"]),
                from_y=int(args["from_y"]),
                to_x=int(args["to_x"]),
                to_y=int(args["to_y"]),
                duration=float(duration),
            )

        # Scroll actions
        elif function_name == "scroll":
            direction = args["direction"]
            amount = args.get("amount", 100)
            action = ScrollAction(direction=direction, amount=int(amount))

        # Control actions
        elif function_name == "wait":
            seconds = args.get("seconds", 1.0)
            action = WaitAction(seconds=float(seconds))
        elif function_name == "done":
            action = DoneAction()

        else:
            print(f"[WARN] Unknown function: {function_name}")
            return None

        # Execute the action
        if action is not None:
            await session.execute_action(action)
            return action

        return None

    async def perform_task(
        self,
        task_description: str,
        session: "DesktopSession",
        logging_dir: Path | None = None,
        tracer=None,
    ) -> AgentResult:
        """
        Perform a task using the Gemini Computer Use agent.

        Args:
            task_description: The task description/instruction
            session: The desktop session to interact with
            logging_dir: Optional directory for logging agent execution
            tracer: Optional tracer object for recording agent actions

        Returns:
            AgentResult with token counts and failure mode
        """
        genai, types = self._lazy_import_genai()

        # Render instruction with template if provided
        instruction = self._render_instruction(task_description)

        # Create client
        if self.api_key:
            client = genai.Client(api_key=self.api_key)
        else:
            # Vertex AI mode
            client = genai.Client()

        # Build function declarations
        custom_functions = self._build_custom_function_declarations(types)

        # Build thinking config
        thinking_config = None
        if self.thinking_level:
            level_map = {
                "minimal": types.ThinkingLevel.MINIMAL,
                "low": types.ThinkingLevel.LOW,
                "medium": types.ThinkingLevel.MEDIUM,
                "high": types.ThinkingLevel.HIGH,
            }
            if self.thinking_level.lower() in level_map:
                thinking_config = types.ThinkingConfig(
                    thinking_level=level_map[self.thinking_level.lower()]
                )

        # Build media resolution
        resolved_media_resolution = None
        if self.media_resolution:
            resolution_map = {
                "low": types.MediaResolution.MEDIA_RESOLUTION_LOW,
                "medium": types.MediaResolution.MEDIA_RESOLUTION_MEDIUM,
                "high": types.MediaResolution.MEDIA_RESOLUTION_HIGH,
            }
            if self.media_resolution.lower() in resolution_map:
                resolved_media_resolution = resolution_map[self.media_resolution.lower()]

        # Build config
        generate_content_config = types.GenerateContentConfig(
            tools=[
                types.Tool(function_declarations=custom_functions),
            ],
            thinking_config=thinking_config,
            media_resolution=resolved_media_resolution,
        )

        # Initialize conversation
        contents: List[Any] = []

        # Add initial instruction with screenshot
        screenshot_bytes = await session.screenshot()

        # Add DONE instruction to the task description
        full_instruction = f"{instruction}\n\nUse the provided computer to complete the task as described. When the task is complete, indicate so clearly by outputting 'DONE'."

        initial_parts = [
            types.Part(text=full_instruction),
            types.Part.from_bytes(data=screenshot_bytes, mime_type="image/png"),
        ]
        contents.append(types.Content(role="user", parts=initial_parts))

        # Track usage
        total_usage = {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
        }

        # Agent loop
        step = 0
        task_completed = False

        try:
            while step < self.max_steps and not task_completed:
                step += 1
                print(f"\n[Step {step}]")

                # Generate response
                response = client.models.generate_content(
                    model=self.model,
                    contents=contents,
                    config=generate_content_config,
                )

                # Record thinking/response to tracer
                if tracer:
                    try:
                        # Get model text response if available
                        model_text = ""
                        if response.candidates and len(response.candidates) > 0:
                            candidate = response.candidates[0]
                            for part in candidate.content.parts:
                                if hasattr(part, "text") and part.text:
                                    model_text += part.text

                        if model_text:
                            # Take screenshot
                            screenshot_bytes = await session.screenshot()
                            tracer.record(
                                "agent_thinking",
                                {
                                    "step": step,
                                    "agent": self.name(),
                                    "model": self.model,
                                    "thinking": model_text[:500],  # Truncate to first 500 chars
                                },
                                [screenshot_bytes],
                            )
                    except Exception as e:
                        print(f"Warning: Failed to record thinking to tracer: {e}")

                # Update usage
                if hasattr(response, "usage_metadata") and response.usage_metadata:
                    md = response.usage_metadata
                    total_usage["prompt_tokens"] += getattr(md, "prompt_token_count", 0) or 0
                    total_usage["completion_tokens"] += (
                        getattr(md, "candidates_token_count", 0) or 0
                    )
                    total_usage["total_tokens"] += getattr(md, "total_token_count", 0) or 0

                # Parse response
                candidate = response.candidates[0]
                model_parts = []
                function_calls = []

                for part in candidate.content.parts:
                    if hasattr(part, "text") and part.text:
                        print(f"[Model]: {part.text}", flush=True)
                        model_parts.append(part)
                        # Check if model indicates task completion
                        if "DONE" in part.text:
                            print(f"\n[Task completed] Model indicated completion at step {step}")
                            task_completed = True
                    if hasattr(part, "function_call") and part.function_call:
                        fc = {
                            "name": getattr(part.function_call, "name", None),
                            "args": dict(getattr(part.function_call, "args", {}) or {}),
                        }
                        function_calls.append(fc)
                        model_parts.append(part)

                # Add model response to conversation
                if model_parts:
                    contents.append(types.Content(role="model", parts=model_parts))

                # Execute function calls
                if function_calls:
                    function_responses = []

                    for fc in function_calls:
                        func_name = fc["name"]
                        func_args = fc["args"]
                        print(f"[Action]: {func_name}({func_args})")

                        # Check if task is done
                        if func_name == "done":
                            task_completed = True
                            function_responses.append(
                                types.Part(
                                    function_response=types.FunctionResponse(
                                        name=func_name,
                                        response={"status": "success", "message": "Task completed"},
                                    )
                                )
                            )
                            break

                        # Execute action
                        action = await self._map_function_call_to_action(
                            func_name, func_args, session
                        )

                        if action is not None:
                            # Take screenshot after action
                            screenshot_bytes = await session.screenshot()

                            # Record action to tracer
                            if tracer:
                                try:
                                    tracer.record(
                                        "agent_action",
                                        {
                                            "step": step,
                                            "agent": self.name(),
                                            "model": self.model,
                                            "action": func_name,
                                            "args": func_args,
                                        },
                                        [screenshot_bytes],
                                    )
                                except Exception as e:
                                    print(f"Warning: Failed to record action to tracer: {e}")

                            # Save debug screenshot with crosshair if logging_dir is provided
                            if logging_dir:
                                try:
                                    import io

                                    from PIL import Image, ImageDraw

                                    # Load screenshot
                                    img = Image.open(io.BytesIO(screenshot_bytes))
                                    draw = ImageDraw.Draw(img)

                                    # Draw crosshair if action has x,y coordinates
                                    if hasattr(action, "x") and hasattr(action, "y"):
                                        x, y = int(action.x), int(action.y)
                                        crosshair_size = 20
                                        crosshair_color = (255, 0, 0)  # Red
                                        crosshair_width = 2

                                        # Draw crosshair lines
                                        draw.line(
                                            [(x - crosshair_size, y), (x + crosshair_size, y)],
                                            fill=crosshair_color,
                                            width=crosshair_width,
                                        )
                                        draw.line(
                                            [(x, y - crosshair_size), (x, y + crosshair_size)],
                                            fill=crosshair_color,
                                            width=crosshair_width,
                                        )

                                        # Draw circle at center
                                        circle_radius = 3
                                        draw.ellipse(
                                            [
                                                (x - circle_radius, y - circle_radius),
                                                (x + circle_radius, y + circle_radius),
                                            ],
                                            fill=crosshair_color,
                                        )

                                    # Save to logging directory
                                    debug_path = logging_dir / f"step_{step:03d}_action.png"
                                    img.save(debug_path)
                                except Exception as e:
                                    print(f"[WARN] Failed to save debug screenshot: {e}")

                            # Add function response with screenshot
                            function_responses.append(
                                types.Part(
                                    function_response=types.FunctionResponse(
                                        name=func_name,
                                        response={"status": "success"},
                                    )
                                )
                            )
                            function_responses.append(
                                types.Part.from_bytes(data=screenshot_bytes, mime_type="image/png")
                            )
                        else:
                            function_responses.append(
                                types.Part(
                                    function_response=types.FunctionResponse(
                                        name=func_name,
                                        response={"status": "error", "message": "Unknown function"},
                                    )
                                )
                            )

                    # Add function responses to conversation
                    if function_responses:
                        contents.append(types.Content(role="user", parts=function_responses))
                else:
                    # No function calls, model might be stuck or finished
                    print("[WARN] No function calls in response")
                    break

            print(f"\n[Completed] Steps: {step}, Tokens: {total_usage}")

            # Determine failure mode
            if task_completed:
                failure_mode = FailureMode.NONE
            elif step >= self.max_steps:
                failure_mode = FailureMode.MAX_STEPS_EXCEEDED
            else:
                failure_mode = FailureMode.UNKNOWN

            return AgentResult(
                total_input_tokens=total_usage["prompt_tokens"],
                total_output_tokens=total_usage["completion_tokens"],
                failure_mode=failure_mode,
            )

        except Exception as e:
            print(f"[ERROR] Agent execution failed: {e}")
            import traceback

            traceback.print_exc()
            return AgentResult(
                total_input_tokens=total_usage["prompt_tokens"],
                total_output_tokens=total_usage["completion_tokens"],
                failure_mode=FailureMode.UNKNOWN,
            )
