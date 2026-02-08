"""Type definitions for the OpenAI provider."""

from dataclasses import dataclass
from enum import StrEnum, auto
from typing import Any, Dict, List, Optional, Union


class LLMProvider(StrEnum):
    """OpenAI LLM provider types."""

    OPENAI = "openai"


class ResponseItemType(StrEnum):
    """Types of items in OpenAI Agent Response output."""

    MESSAGE = "message"
    COMPUTER_CALL = "computer_call"
    COMPUTER_CALL_OUTPUT = "computer_call_output"
    REASONING = "reasoning"


@dataclass
class ComputerAction:
    """Represents a computer action to be performed."""

    type: str
    x: Optional[int] = None
    y: Optional[int] = None
    text: Optional[str] = None
    button: Optional[str] = None
    keys: Optional[List[str]] = None
    ms: Optional[int] = None
    scroll_x: Optional[int] = None
    scroll_y: Optional[int] = None
    path: Optional[List[Dict[str, int]]] = None
