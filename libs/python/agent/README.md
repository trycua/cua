# Cua Agent

Computer-Use framework with liteLLM integration for running agentic workflows on macOS, Windows, and Linux sandboxes.

**[Documentation](https://cua.ai/docs/cua/reference/agent-sdk)** - Installation, guides, and configuration.

## Supported Models

| Provider | Models | Prefix |
|----------|--------|--------|
| Anthropic | Claude 3.5+, Claude 4 | `anthropic/` |
| OpenAI | GPT-4, computer-use-preview | `openai/` |
| Google | Gemini 2.5+, Gemini 3 | `gemini-*` |
| MiniMax | MiniMax-M2.5, MiniMax-M2.5-highspeed | `minimax/` |
| Yutori | n1 | `yutori/` |
| CUA | Any model via CUA inference | `cua/` |

### MiniMax Setup

Set the `MINIMAX_API_KEY` environment variable and use the `minimax/` prefix:

```python
agent = ComputerAgent(
    model="minimax/MiniMax-M2.5",
    tools=[computer],
)
```

MiniMax M2.5 offers a 204K context window. The `MiniMax-M2.5-highspeed` variant provides faster inference.
