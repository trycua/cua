"""CUA Agent implementation using the Computer Agent SDK."""

import asyncio
import base64
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict, List

from . import register_agent
from .base import AgentResult, BaseAgent, FailureMode


def _is_retryable_api_error(exc: BaseException) -> bool:
    """Return True if the exception is a transient error that can be retried."""
    if isinstance(exc, (asyncio.TimeoutError, TimeoutError)):
        return True
    try:
        import litellm.exceptions as _le

        if isinstance(
            exc,
            (
                _le.RateLimitError,
                _le.ServiceUnavailableError,
                _le.APIConnectionError,
                _le.Timeout,
                _le.InternalServerError,
            ),
        ):
            return True
    except Exception:
        pass
    msg = str(exc).lower()
    return any(k in msg for k in ("timeout", "rate limit", "503", "502", "429", "connection"))


if TYPE_CHECKING:
    from ..computers import DesktopSession


@register_agent("cua-agent")
class CuaAgent(BaseAgent):
    """Agent implementation using the CUA Computer Agent SDK."""

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.model = kwargs.get("model", "anthropic/claude-sonnet-4-20250514")
        self.max_steps = kwargs.get("max_steps", 100)
        # Number of times to retry the entire task when a transient API error occurs.
        self.task_retries = int(kwargs.get("task_retries", 2))

    @staticmethod
    def name() -> str:
        return "cua-agent"

    def _create_custom_computer(self, session: "DesktopSession") -> Dict[str, Any]:
        """Create a custom computer dict from a DesktopSession.

        Args:
            session: The desktop session to wrap

        Returns:
            Dict with computer functions compatible with cua-agent
        """
        from ..types import (
            ClickAction,
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

        # Screenshot function (required)
        async def screenshot():
            """Take a screenshot and return as base64 string."""
            screenshot_bytes = await session.screenshot()
            return base64.b64encode(screenshot_bytes).decode("utf-8")

        # Click function
        async def click(x: int, y: int, button: str = "left"):
            """Click at coordinates with specified button."""
            if button == "left":
                action = ClickAction(x=x, y=y)
            elif button == "right":
                action = RightClickAction(x=x, y=y)
            elif button == "middle":
                action = MiddleClickAction(x=x, y=y)
            else:
                raise ValueError(f"Unknown button type: {button}")
            await session.execute_action(action)

        # Double click function
        async def double_click(x: int, y: int):
            """Double click at coordinates."""
            action = DoubleClickAction(x=x, y=y)
            await session.execute_action(action)

        # Type function
        async def type_text(text: str):
            """Type text."""
            action = TypeAction(text=text)
            await session.execute_action(action)

        # Keypress function
        async def keypress(keys):
            """Press key combination."""
            if isinstance(keys, str):
                action = KeyAction(key=keys)
            else:
                action = HotkeyAction(keys=list(keys))
            await session.execute_action(action)

        # Move function
        async def move(x: int, y: int):
            """Move cursor to coordinates."""
            action = MoveToAction(x=x, y=y)
            await session.execute_action(action)

        # Scroll function
        async def scroll(x: int, y: int, scroll_x: int, scroll_y: int):
            """Scroll at coordinates."""
            if scroll_y < 0:
                direction = "up"
                amount = abs(scroll_y)
            else:
                direction = "down"
                amount = abs(scroll_y)
            action = ScrollAction(direction=direction, amount=amount)
            await session.execute_action(action)

        # Drag function
        async def drag(path: List[Dict[str, int]]):
            """Drag along specified path."""
            if len(path) < 2:
                raise ValueError("Path must have at least 2 points")
            start = path[0]
            end = path[-1]
            action = DragAction(from_x=start["x"], from_y=start["y"], to_x=end["x"], to_y=end["y"])
            await session.execute_action(action)

        # Wait function
        async def wait(ms: int = 1000):
            """Wait for specified milliseconds."""
            action = WaitAction(seconds=ms / 1000.0)
            await session.execute_action(action)

        # Get dimensions
        async def get_dimensions():
            """Get screen dimensions."""
            # Default dimensions
            return (1024, 768)

        # Get environment type
        async def get_environment():
            """Get environment type."""
            # Default to linux
            return "linux"

        # Build custom computer dict
        custom_computer = {
            "screenshot": screenshot,  # Required
            "dimensions": get_dimensions,
            "environment": get_environment,
            "click": click,
            "double_click": double_click,
            "type": type_text,
            "keypress": keypress,
            "move": move,
            "scroll": scroll,
            "drag": drag,
            "wait": wait,
        }

        return custom_computer

    async def perform_task(
        self,
        task_description: str,
        session: "DesktopSession",
        logging_dir: Path | None = None,
        tracer=None,
    ) -> AgentResult:
        """
        Perform a task using the CUA Computer Agent.

        Args:
            task_description: The task description/instruction
            session: The desktop session to interact with
            logging_dir: Optional directory for logging agent execution
            tracer: Optional tracer object for recording agent actions

        Returns:
            AgentResult with token counts and failure mode
        """
        try:
            from agent import ComputerAgent
        except ImportError as e:
            raise RuntimeError(
                "cua-agent requires the `cua-agent` package to be installed. "
                "Install it with: pip install cua-agent"
            ) from e

        # Render instruction with template if provided
        instruction = self._render_instruction(task_description)

        # Create trajectory directory if logging_dir is provided
        trajectory_dir = None
        if logging_dir:
            trajectory_dir = logging_dir / "trajectories"
            trajectory_dir.mkdir(parents=True, exist_ok=True)

        # Create custom computer dict from session
        custom_computer = self._create_custom_computer(session)

        # Create agent with custom computer
        agent = ComputerAgent(
            model=self.model,
            tools=[custom_computer],
            only_n_most_recent_images=3,
            trajectory_dir=trajectory_dir,
            instructions="Use the provided computer to complete the task as described. When the task is complete, indicate so clearly by outputting 'DONE'.",
        )
        print("CUA Agent initialized with model:", self.model)

        # Run the agent and track usage (with task-level retry on transient API errors)
        last_exc: BaseException | None = None
        for attempt in range(self.task_retries + 1):
            if attempt > 0:
                delay = 5.0 * (2 ** (attempt - 1))
                print(
                    f"[cua-agent] Retrying task (attempt {attempt + 1}/{self.task_retries + 1}) "
                    f"after transient error: {last_exc!r}. Waiting {delay:.0f}s …"
                )
                await asyncio.sleep(delay)

            try:
                total_usage = {
                    "prompt_tokens": 0,
                    "completion_tokens": 0,
                    "total_tokens": 0,
                    "response_cost": 0.0,
                }

                step = 0
                task_completed = False

                async for result in agent.run(instruction):
                    sys.stdout.flush()  # Flush output

                    step += 1
                    for k in total_usage:
                        total_usage[k] += result["usage"].get(k, 0)

                    # Record agent step to tracer
                    if tracer:
                        try:
                            # Take screenshot
                            screenshot = await session.screenshot()
                            # Record the step with metadata
                            usage = result["usage"]
                            if hasattr(usage, "model_dump"):
                                usage = usage.model_dump()
                            elif not isinstance(usage, dict):
                                usage = dict(usage)
                            tracer.record(
                                "agent_step",
                                {
                                    "step": step,
                                    "agent": self.name(),
                                    "model": self.model,
                                    "usage": usage,
                                    "output": result["output"],
                                },
                                [screenshot],
                            )
                        except Exception as e:
                            print(f"Warning: Failed to record agent step to tracer: {e}")

                    # Check if we've reached max_steps
                    if step >= self.max_steps:
                        print(f"\n[Max steps reached] Stopped at step {step}/{self.max_steps}")
                        break

                    # Check if task is completed (agent returned done or similar)
                    for item in result["output"]:
                        if item["type"] == "message":
                            if "DONE" in item["content"][0]["text"]:
                                print(
                                    f"\n[Task completed] Agent indicated completion at step {step}"
                                )
                                task_completed = True
                                break

                print(f"\nTotal usage: {total_usage}")
                print(f"Steps completed: {step}/{self.max_steps}")

                # Determine failure mode
                if task_completed:
                    failure_mode = FailureMode.NONE
                elif step >= self.max_steps:
                    failure_mode = FailureMode.MAX_STEPS_EXCEEDED
                else:
                    failure_mode = FailureMode.NONE  # Completed within max_steps

                return AgentResult(
                    total_input_tokens=total_usage.get("prompt_tokens", 0),
                    total_output_tokens=total_usage.get("completion_tokens", 0),
                    failure_mode=failure_mode,
                )

            except Exception as exc:
                last_exc = exc
                if attempt < self.task_retries and _is_retryable_api_error(exc):
                    # Will retry in the next loop iteration
                    continue

                # Non-retryable or out of retries
                import traceback

                print(f"Agent execution failed: {exc}")
                traceback.print_exc()

                failure_mode = (
                    FailureMode.API_ERROR if _is_retryable_api_error(exc) else FailureMode.UNKNOWN
                )
                return AgentResult(
                    total_input_tokens=0,
                    total_output_tokens=0,
                    failure_mode=failure_mode,
                )
