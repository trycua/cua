"""Core type definitions."""

from dataclasses import dataclass
from enum import StrEnum
from typing import Any, Dict, List, Optional, TypedDict, Union


class AgentLoop(StrEnum):
    """Enumeration of available loop types."""

    ANTHROPIC = "anthropic"  # Anthropic implementation
    OMNI = "omni"  # OmniLoop implementation
    OPENAI = "openai"  # OpenAI implementation
    OLLAMA = "ollama"  # OLLAMA implementation
    UITARS = "uitars"  # UI-TARS implementation
    # Add more loop types as needed


class LLMProvider(StrEnum):
    """Supported LLM providers."""

    ANTHROPIC = "anthropic"
    OPENAI = "openai"
    OLLAMA = "ollama"
    OAICOMPAT = "oaicompat"
    MLXVLM= "mlxvlm"


@dataclass
class LLM:
    """Configuration for LLM model and provider."""

    provider: LLMProvider
    name: Optional[str] = None
    provider_base_url: Optional[str] = None

    def __post_init__(self):
        """Set default model name if not provided."""
        if self.name is None:
            from .provider_config import DEFAULT_MODELS

            self.name = DEFAULT_MODELS.get(self.provider)

        # Set default provider URL if none provided
        if self.provider_base_url is None and self.provider == LLMProvider.OAICOMPAT:
            # Default for vLLM
            self.provider_base_url = "http://localhost:8000/v1"
            # Common alternatives:
            # - LM Studio: "http://localhost:1234/v1"
            # - LocalAI: "http://localhost:8080/v1"
            # - Ollama with OpenAI compatible API: "http://localhost:11434/v1"


# For backward compatibility
LLMModel = LLM
Model = LLM


class AgentResponse(TypedDict, total=False):
    """Agent response format."""

    id: str
    object: str
    created_at: int
    status: str
    error: Optional[str]
    incomplete_details: Optional[Any]
    instructions: Optional[Any]
    max_output_tokens: Optional[int]
    model: str
    output: List[Dict[str, Any]]
    parallel_tool_calls: bool
    previous_response_id: Optional[str]
    reasoning: Dict[str, str]
    store: bool
    temperature: float
    text: Dict[str, Dict[str, str]]
    tool_choice: str
    tools: List[Dict[str, Union[str, int]]]
    top_p: float
    truncation: str
    usage: Dict[str, Any]
    user: Optional[str]
    metadata: Dict[str, Any]
    response: Dict[str, List[Dict[str, Any]]]
    # Additional fields for error responses
    role: str
    content: Union[str, List[Dict[str, Any]]]
