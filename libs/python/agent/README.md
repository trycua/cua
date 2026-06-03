# Cua Agent

Computer-Use framework with liteLLM integration for running agentic workflows on macOS, Windows, and Linux sandboxes.

**[Documentation](https://cua.ai/docs/cua/reference/agent-sdk)** - Installation, guides, and configuration.

## Supported Models

| Provider | Models | Prefix |
|----------|--------|--------|
| Anthropic | Claude 3.5+, Claude 4 | `anthropic/` |
| OpenAI | GPT-4, computer-use-preview | `openai/` |
| Google | Gemini 2.5+, Gemini 3 | `gemini-*` |
| MiniMax | MiniMax-M3 (default), MiniMax-M2.7, MiniMax-M2.7-highspeed | `minimax/` |
| Yutori | n1 | `yutori/` |
| CUA | Any model via CUA inference | `cua/` |

### MiniMax Setup

Set the `MINIMAX_API_KEY` environment variable and use the `minimax/` prefix:

```python
agent = ComputerAgent(
    model="minimax/MiniMax-M3",
    tools=[computer],
)
```

MiniMax-M3 is the latest flagship model with a 512K context window, up to 128K max output, and image input support. The `MiniMax-M2.7` and `MiniMax-M2.7-highspeed` variants from the previous generation remain available.
