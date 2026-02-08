"""Abstract base classes for tools that can be used with any provider."""

from abc import ABCMeta, abstractmethod
from dataclasses import dataclass, fields, replace
from typing import Any, Dict


class BaseTool(metaclass=ABCMeta):
    """Abstract base class for provider-agnostic tools."""

    name: str

    @abstractmethod
    async def __call__(self, **kwargs) -> Any:
        """Executes the tool with the given arguments."""
        ...

    @abstractmethod
    def to_params(self) -> Dict[str, Any]:
        """Convert tool to provider-specific API parameters.

        Returns:
            Dictionary with tool parameters specific to the LLM provider
        """
        raise NotImplementedError


@dataclass(kw_only=True, frozen=True)
class ToolResult:
    """Represents the result of a tool execution."""

    output: str | None = None
    error: str | None = None
    base64_image: str | None = None
    system: str | None = None
    content: list[dict] | None = None

    def __bool__(self):
        return any(getattr(self, field.name) for field in fields(self))

    def __add__(self, other: "ToolResult"):
        def combine_fields(field: str | None, other_field: str | None, concatenate: bool = True):
            if field and other_field:
                if concatenate:
                    return field + other_field
                raise ValueError("Cannot combine tool results")
            return field or other_field

        return ToolResult(
            output=combine_fields(self.output, other.output),
            error=combine_fields(self.error, other.error),
            base64_image=combine_fields(self.base64_image, other.base64_image, False),
            system=combine_fields(self.system, other.system),
            content=self.content or other.content,  # Use first non-None content
        )

    def replace(self, **kwargs):
        """Returns a new ToolResult with the given fields replaced."""
        return replace(self, **kwargs)


class CLIResult(ToolResult):
    """A ToolResult that can be rendered as a CLI output."""


class ToolFailure(ToolResult):
    """A ToolResult that represents a failure."""


class ToolError(Exception):
    """Raised when a tool encounters an error."""

    def __init__(self, message):
        self.message = message
