"""Tool-related type definitions."""

from enum import StrEnum
from typing import Any, Dict, Optional

from pydantic import BaseModel, ConfigDict


class ToolInvocationState(StrEnum):
    """States for tool invocation."""
    CALL = 'call'
    PARTIAL_CALL = 'partial-call'
    RESULT = 'result'

class ToolInvocation(BaseModel):
    """Tool invocation type."""
    model_config = ConfigDict(extra='forbid')
    state: Optional[str] = None
    toolCallId: str
    toolName: Optional[str] = None
    args: Optional[Dict[str, Any]] = None

class ClientAttachment(BaseModel):
    """Client attachment type."""
    name: str
    contentType: str
    url: str

class ToolResult(BaseModel):
    """Result of a tool execution."""
    model_config = ConfigDict(extra='forbid')
    output: Optional[str] = None
    error: Optional[str] = None
    metadata: Optional[Dict[str, Any]] = None
