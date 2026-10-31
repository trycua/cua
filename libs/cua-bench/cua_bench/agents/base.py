"""Base agent interface for cua-bench."""

import base64
from abc import ABC, abstractmethod
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict, List

if TYPE_CHECKING:
    from ..computers import DesktopSession


class FailureMode(Enum):
    """Failure mode for agent execution."""

    UNSET = "unset"
    NONE = "none"
    UNKNOWN = "unknown"
    MAX_STEPS_EXCEEDED = "max_steps_exceeded"
    API_ERROR = "api_error"


@dataclass
class AgentResult:
    """Result of agent execution."""

    total_input_tokens: int = 0
    total_output_tokens: int = 0
    failure_mode: FailureMode = FailureMode.UNSET


class BaseAgent(ABC):
    """Base class for agents that can perform tasks."""

    def __init__(self, **kwargs):
        super().__init__()
        self._version = kwargs.get("version", None)
        self._prompt_template = kwargs.get("prompt_template", None)

    @staticmethod
    @abstractmethod
    def name() -> str:
        """Return the name of the agent."""
        raise NotImplementedError("Agents must implement this method.")

    @property
    def version(self) -> str | None:
        """
        The version of the agent. Can be any string (e.g. could be
        a date or a semantic version, a single digit, etc.).

        Can be dynamic based on a kwarg in the constructor.

        Examples:
            # Default version (latest, or undefined)
            agent = ClaudeCodeAgent()
            agent.version  # -> None

            # Custom version
            agent = ClaudeCodeAgent(version="1.2.3")
            agent.version  # -> "1.2.3"

            # With other parameters
            agent = ClaudeCodeAgent(
                model_name="anthropic/claude-3-sonnet-20240229",
                version="0.5.0"
            )
            agent.version  # -> "0.5.0"

        The version, if defined, is used in the installation script:
            npm install -g @anthropic-ai/claude-code@{version}
        """
        return self._version

    @property
    def prompt_template(self) -> str | None:
        """
        The path to a custom prompt template file. If specified, this template
        will be used to render the instruction before passing it to the agent.

        The template must be a Jinja2 template that includes an "instruction" variable.

        Examples:
            # Default behavior (no template)
            agent = OpenHandsAgent()
            agent.prompt_template  # -> None

            # Custom template
            agent = OpenHandsAgent(prompt_template="./custom_prompt.j2")
            agent.prompt_template  # -> "./custom_prompt.j2"

        Usage with CLI:
            cb run --agent openhands --task-id hello-world \
                --agent-kwarg prompt_template=./openhands_template.j2
        """
        return self._prompt_template

    def _render_instruction(self, instruction: str) -> str:
        """
        Render the instruction using the prompt template if provided.

        Args:
            instruction: The raw instruction text

        Returns:
            The rendered instruction (or original if no template is provided)

        Raises:
            FileNotFoundError: If the template file doesn't exist
            ValueError: If the template doesn't include an "instruction" variable
        """
        if self.prompt_template is None:
            return instruction

        from jinja2 import Template

        template_path = Path(self.prompt_template)
        if not template_path.exists():
            raise FileNotFoundError(f"Template file not found: {template_path}")

        template = Template(template_path.read_text())
        return template.render(instruction=instruction)

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


    @abstractmethod
    async def perform_task(
        self,
        task_description: str,
        session: "DesktopSession",
        logging_dir: Path | None = None,
        tracer=None,
    ) -> AgentResult:
        """
        Perform a task using the agent.

        Args:
            task_description: The task description/instruction
            session: The desktop or mobile session to interact with
            logging_dir: Optional directory for logging agent execution
            tracer: Optional tracer object for recording agent actions

        Returns:
            AgentResult with token counts and failure mode
        """
        raise NotImplementedError("Agents must implement this method.")
