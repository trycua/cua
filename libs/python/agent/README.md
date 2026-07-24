# Cua Agent

Computer-Use framework with liteLLM integration for running agentic workflows on macOS, Windows, and Linux sandboxes.

**[Documentation](https://cua.ai/docs/cua/reference/agent-sdk)** - Installation, guides, and configuration.

## MiniMax-M3 image input

MiniMax-M3 can receive Cua screenshots and return standard computer function calls through either compatible API. Pass the matching LiteLLM model prefix and base URL to `ComputerAgent`:

```python
import os

from cua_agent import ComputerAgent

agent = ComputerAgent(
    model="openai/MiniMax-M3",
    api_base="https://api.minimax.io/v1",
    api_key=os.environ["MINIMAX_API_KEY"],
    tools=[computer],
)
```

| Protocol | Region | Model | Base URL |
| --- | --- | --- | --- |
| OpenAI-compatible | Global | `openai/MiniMax-M3` | `https://api.minimax.io/v1` |
| OpenAI-compatible | China | `openai/MiniMax-M3` | `https://api.minimaxi.com/v1` |
| Anthropic-compatible | Global | `anthropic/MiniMax-M3` | `https://api.minimax.io/anthropic` |
| Anthropic-compatible | China | `anthropic/MiniMax-M3` | `https://api.minimaxi.com/anthropic` |
