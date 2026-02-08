from enum import StrEnum


class LLMProvider(StrEnum):
    """Enum for supported API providers."""

    ANTHROPIC = "anthropic"
    BEDROCK = "bedrock"
    VERTEX = "vertex"


PROVIDER_TO_DEFAULT_MODEL_NAME: dict[LLMProvider, str] = {
    LLMProvider.ANTHROPIC: "claude-3-7-sonnet-20250219",
    LLMProvider.BEDROCK: "anthropic.claude-3-7-sonnet-20250219-v2:0",
    LLMProvider.VERTEX: "claude-3-5-sonnet-v2@20241022",
}
