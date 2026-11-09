import asyncio
import os
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict

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


@register_agent("qwen35")
class Qwen35Agent(BaseAgent):
    """Agent implementation using a Qwen3.5 model via the CUA Computer Agent SDK."""

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.model = kwargs.get("model", "openai/qwen35-4b")
        self.base_url = kwargs.get("base_url") or os.environ.get("OPENAI_ENDPOINT")
        self.api_key = kwargs.get("api_key") or os.environ.get("OPENAI_API_KEY", "EMPTY")
        self.max_steps = kwargs.get("max_steps", 50)
        self.task_retries = int(kwargs.get("task_retries", 0))

    @staticmethod
    def name() -> str:
        return "qwen35"

    async def perform_task(
        self,
        task_description: str,
        session: "DesktopSession",
        logging_dir: Path | None = None,
        tracer=None,
    ) -> AgentResult:
        """Run the task using the Qwen3.5 model via the CUA Computer Agent SDK."""
        try:
            from cua_agent import ComputerAgent
        except ImportError as e:
            raise RuntimeError(
                "qwen35-agent requires the `cua-agent` package to be installed. "
                "Install it with: pip install cua-agent"
            ) from e

        instruction = f"Task: {self._render_instruction(task_description)}"

        # print(f"Instruction: {instruction}")

        trajectory_dir = None
        if logging_dir:
            trajectory_dir = logging_dir / "trajectories"
            trajectory_dir.mkdir(parents=True, exist_ok=True)

        custom_computer = self._create_custom_computer(session)

        # Extra litellm kwargs forwarded to the loop
        extra_kwargs: Dict[str, Any] = {}
        if self.base_url:
            extra_kwargs["base_url"] = self.base_url
        if self.api_key:
            extra_kwargs["api_key"] = self.api_key

        agent = ComputerAgent(
            model=self.model,
            tools=[custom_computer],
            only_n_most_recent_images=3,
            trajectory_dir=trajectory_dir,
            telemetry_enabled=False,
            instructions=(
                "Use the provided tools to interact with the computer (click, type, scroll, etc.) to accomplish the task. When the task is complete, indicate so clearly by outputting 'DONE'."
            ),
            **extra_kwargs,
        )

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
            # elif step >= self.max_steps:
            #     failure_mode = FailureMode.MAX_STEPS_EXCEEDED
            else:
                failure_mode = FailureMode.NONE

            return AgentResult(
                total_input_tokens=total_usage.get("prompt_tokens", 0),
                total_output_tokens=total_usage.get("completion_tokens", 0),
                failure_mode=failure_mode,
            )

        except Exception as exc:
            import traceback

            # print(f"Agent execution failed: {exc}")
            traceback.print_exc()

            failure_mode = (
                FailureMode.API_ERROR if _is_retryable_api_error(exc) else FailureMode.UNKNOWN
            )
            return AgentResult(
                total_input_tokens=0,
                total_output_tokens=0,
                failure_mode=failure_mode,
            )
