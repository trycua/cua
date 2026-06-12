import asyncio
import base64
import os
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


@register_agent("opencua")
class OpenCUAAgent(BaseAgent):
    """Agent implementation using an OpenCUA model via the CUA Computer Agent SDK."""

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.model = kwargs.get("model", "openai/opencua-7b")
        self.base_url = kwargs.get("base_url") or os.environ.get("OPENAI_ENDPOINT")
        self.api_key = kwargs.get("api_key") or os.environ.get("OPENAI_API_KEY", "EMPTY")
        self.max_steps = kwargs.get("max_steps", 50)
        self.task_retries = int(kwargs.get("task_retries", 0))

    @staticmethod
    def name() -> str:
        return "opencua"

    def _create_custom_computer(self, session: "DesktopSession") -> Dict[str, Any]:
        """Build a computer-handler dict from a DesktopSession."""
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

        async def screenshot():
            raw = await session.screenshot()
            return base64.b64encode(raw).decode("utf-8")

        async def click(x: int, y: int, button: str = "left"):
            if button == "left":
                action = ClickAction(x=x, y=y)
            elif button == "right":
                action = RightClickAction(x=x, y=y)
            elif button == "middle":
                action = MiddleClickAction(x=x, y=y)
            else:
                raise ValueError(f"Unknown button type: {button}")
            await session.execute_action(action)

        async def double_click(x: int, y: int):
            await session.execute_action(DoubleClickAction(x=x, y=y))

        async def type_text(text: str):
            await session.execute_action(TypeAction(text=text))

        async def keypress(keys):
            if isinstance(keys, str):
                await session.execute_action(KeyAction(key=keys))
            else:
                await session.execute_action(HotkeyAction(keys=list(keys)))

        async def move(x: int, y: int):
            await session.execute_action(MoveToAction(x=x, y=y))

        async def scroll(x: int, y: int, scroll_x: int, scroll_y: int):
            direction = "up" if scroll_y < 0 else "down"
            await session.execute_action(ScrollAction(direction=direction, amount=abs(scroll_y)))

        async def drag(path: List[Dict[str, int]]):
            if len(path) < 2:
                raise ValueError("Path must have at least 2 points")
            start, end = path[0], path[-1]
            await session.execute_action(
                DragAction(from_x=start["x"], from_y=start["y"], to_x=end["x"], to_y=end["y"])
            )

        async def wait(ms: int = 1000):
            await session.execute_action(WaitAction(seconds=ms / 1000.0))

        async def get_dimensions():
            return (1280, 800)

        async def get_environment():
            return "linux"

        return {
            "screenshot": screenshot,
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

    async def perform_task(
        self,
        task_description: str,
        session: "DesktopSession",
        logging_dir: Path | None = None,
        tracer=None,
    ) -> AgentResult:
        """Run the task using the OpenCUA model via the CUA Computer Agent SDK."""
        try:
            from cua_agent import ComputerAgent
        except ImportError as e:
            raise RuntimeError(
                "opencua-agent requires the `cua-agent` package to be installed. "
                "Install it with: pip install cua-agent"
            ) from e

        instruction = self._render_instruction(task_description)

        print(f"Instruction: {instruction}")

        trajectory_dir = None
        if logging_dir:
            trajectory_dir = logging_dir / "trajectories"
            trajectory_dir.mkdir(parents=True, exist_ok=True)

        custom_computer = self._create_custom_computer(session)

        # Extra litellm kwargs forwarded to the OpenCUA loop
        extra_kwargs: Dict[str, Any] = {}
        if self.base_url:
            extra_kwargs["base_url"] = self.base_url
        if self.api_key:
            extra_kwargs["api_key"] = self.api_key

        # OpenCUAConfig only declares "click" capability, so it cannot be used
        # directly as the top-level agent loop.  Using "model+model" routes
        # through ComposedGroundedConfig (capabilities: ["click", "step"]),
        # which uses the same OpenCUA model for both grounding (click prediction)
        # and thinking (task reasoning) — identical to what OpenCUAConfig does
        # internally in its own predict_step.
        composed_model = f"{self.model}+{self.model}"

        agent = ComputerAgent(
            model=composed_model,
            tools=[custom_computer],
            only_n_most_recent_images=3,
            trajectory_dir=trajectory_dir,
            telemetry_enabled=False,
            instructions=(
                "Use the provided computer to complete the task as described. "
                "When the task is complete, indicate so clearly by outputting 'DONE'."
            ),
            **extra_kwargs,
        )

        print(f"OpenCUA Agent initialized with model: {composed_model}")

        total_usage = {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
            "response_cost": 0.0,
        }

        try:
            step = 0
            task_completed = False

            async for result in agent.run(instruction):
                sys.stdout.flush()
                step += 1

                for k in total_usage:
                    total_usage[k] += result["usage"].get(k, 0)

                if tracer:
                    try:
                        screenshot_bytes = await session.screenshot()
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
                            [screenshot_bytes],
                        )
                    except Exception as e:
                        print(f"Warning: Failed to record agent step to tracer: {e}")

                if step >= self.max_steps:
                    print(f"\n[Max steps reached] Stopped at step {step}/{self.max_steps}")
                    break

                for item in result["output"]:
                    if item["type"] == "message":
                        content = item.get("content", [])
                        if content and "DONE" in content[0].get("text", ""):
                            print(f"\n[Task completed] Agent indicated completion at step {step}")
                            task_completed = True
                            break

            print(f"\nTotal usage: {total_usage}")
            print(f"Steps completed: {step}/{self.max_steps}")

            if task_completed:
                failure_mode = FailureMode.NONE
            elif step >= self.max_steps:
                failure_mode = FailureMode.MAX_STEPS_EXCEEDED
            else:
                failure_mode = FailureMode.NONE

            return AgentResult(
                total_input_tokens=total_usage.get("prompt_tokens", 0),
                total_output_tokens=total_usage.get("completion_tokens", 0),
                failure_mode=failure_mode,
            )

        except Exception as exc:
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
