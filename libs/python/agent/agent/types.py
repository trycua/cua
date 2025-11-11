"""
Type definitions for agent
"""

import re
from collections.abc import Iterable
from typing import Any, Callable, Dict, List, Literal, Optional, Protocol

from litellm import ResponseInputParam, ResponsesAPIResponse, ToolParam
from pydantic import BaseModel

# Agent input types
Messages = str | ResponseInputParam | List[Dict[str, Any]]
Tools = Optional[Iterable[ToolParam]]

# Agent output types
AgentResponse = ResponsesAPIResponse
AgentCapability = Literal["step", "click"]


# Exception types
class ToolError(RuntimeError):
    """Base exception for tool-related errors"""

    pass


class IllegalArgumentError(ToolError):
    """Exception raised when function arguments are invalid"""

    pass


# Agent config registration
class AgentConfigInfo(BaseModel):
    """Information about a registered agent config"""

    agent_class: type
    models_regex: str
    priority: int = 0

    def matches_model(self, model: str) -> bool:
        """Check if this agent config matches the given model"""
        return bool(re.match(self.models_regex, model))
