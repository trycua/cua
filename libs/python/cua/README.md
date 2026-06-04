# cua

The unified Python SDK for Computer-Use Agents.

## Installation

```bash
pip install cua
```

## Quick Start

```python
from cua import Sandbox, Image, ComputerAgent

# Ephemeral local sandbox with an agent
async with Sandbox.ephemeral(Image.linux(), local=True) as sb:
    await sb.shell.run("uname -a")

    agent = ComputerAgent(model="anthropic/claude-sonnet-4-5", tools=[sb])
    async for response in agent.run("Open the browser and go to example.com"):
        print(response)
```

## What's included

| Package | Import | Purpose |
|---|---|---|
| `cua-sandbox` | `from cua import Sandbox, Image` | VM/container sandboxes |
| `cua-agent` | `from cua import ComputerAgent` | LLM-driven computer-use agent |
| `cua-cli` | `cua` command | CLI for managing sandboxes and images |

`cua-agent[cloud]` extras are included by default (OpenAI, Anthropic, Gemini API backends).

## Extras

```bash
pip install cua[omni]       # SOM-based visual grounding
pip install cua[uitars-mlx] # UiTars via MLX (Apple Silicon)
pip install cua[uitars-hf]  # UiTars via HuggingFace
pip install cua[all]        # Everything
```

## Python Version

Requires Python 3.12 or 3.13. For Python 3.11, install `cua-sandbox` directly.

## Telemetry

Cua collects anonymous usage statistics by default. Opt out with:

```bash
export CUA_TELEMETRY_ENABLED=false
```

Or per-instance:

```python
async with Sandbox.ephemeral(Image.linux(), telemetry_enabled=False) as sb:
    ...

agent = ComputerAgent(..., telemetry_enabled=False)
```

## Links

- [Documentation](https://docs.trycua.com)
- [GitHub](https://github.com/trycua/cua)
- [Issues](https://github.com/trycua/cua/issues)
