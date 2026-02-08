"""Provider-specific configurations and constants."""

from .types import LLMProvider

# Default models for different providers
DEFAULT_MODELS = {
    LLMProvider.OPENAI: "gpt-4o",
    LLMProvider.ANTHROPIC: "claude-3-7-sonnet-20250219",
    LLMProvider.OLLAMA: "gemma3:4b-it-q4_K_M",
    LLMProvider.OAICOMPAT: "Qwen2.5-VL-7B-Instruct",
    LLMProvider.MLXVLM: "mlx-community/UI-TARS-1.5-7B-4bit",
}

# Map providers to their environment variable names
ENV_VARS = {
    LLMProvider.OPENAI: "OPENAI_API_KEY",
    LLMProvider.ANTHROPIC: "ANTHROPIC_API_KEY",
    LLMProvider.OLLAMA: "none",
    LLMProvider.OAICOMPAT: "none",  # OpenAI-compatible API typically doesn't require an API key
    LLMProvider.MLXVLM: "none",  # MLX VLM typically doesn't require an API key
}
