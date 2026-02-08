"""Omni-specific tool base classes."""

from abc import ABCMeta, abstractmethod
from typing import Any, Dict

from ....core.tools.base import BaseTool


class BaseOmniTool(BaseTool, metaclass=ABCMeta):
    """Abstract base class for Omni provider tools."""

    def __init__(self):
        """Initialize the base Omni tool."""
        # No specific initialization needed yet, but included for future extensibility
        pass

    @abstractmethod
    async def __call__(self, **kwargs) -> Any:
        """Executes the tool with the given arguments."""
        ...

    @abstractmethod
    def to_params(self) -> Dict[str, Any]:
        """Convert tool to Omni provider-specific API parameters.

        Returns:
            Dictionary with tool parameters for the specific API
        """
        raise NotImplementedError
