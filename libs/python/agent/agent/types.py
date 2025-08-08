"""
Type definitions for agent
"""

from typing import Dict, List, Any, Optional, Callable, Protocol, Literal
from pydantic import BaseModel
import re
from litellm import ResponseInputParam, ResponsesAPIResponse, ToolParam
from collections.abc import Iterable

# Agent input types
Messages = str | ResponseInputParam | List[Dict[str, Any]]
Tools = Optional[Iterable[ToolParam]]

# Agent output types
AgentResponse = ResponsesAPIResponse 
AgentCapability = Literal["step", "click"]


# Agent config registration
class AgentConfigInfo(BaseModel):
    """Information about a registered agent config"""
    agent_class: type
    models_regex: str
    priority: int = 0
    
    def matches_model(self, model: str) -> bool:
        """Check if this agent config matches the given model"""
        return bool(re.match(self.models_regex, model))
