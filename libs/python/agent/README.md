<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

[![Python](https://img.shields.io/badge/Python-333333?logo=python&logoColor=white&labelColor=333333)](#)
[![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)
[![PyPI](https://img.shields.io/pypi/v/cua-computer?color=333333)](https://pypi.org/project/cua-computer/)

</h1>
</div>

**cua-agent** is a general Computer-Use framework with liteLLM integration for running agentic workflows on macOS, Windows, and Linux sandboxes. It provides a unified interface for computer-use agents across multiple LLM providers with advanced callback system for extensibility.

## Features

- **Safe Computer-Use/Tool-Use**: Using Computer SDK for sandboxed desktops
- **Multi-Agent Support**: Anthropic Claude, OpenAI computer-use-preview, UI-TARS, Omniparser + any LLM
- **Multi-API Support**: Take advantage of liteLLM supporting 100+ LLMs / model APIs, including local models (`huggingface-local/`, `ollama_chat/`, `mlx/`)
- **Cross-Platform**: Works on Windows, macOS, and Linux with cloud and local computer instances
- **Extensible Callbacks**: Built-in support for image retention, cache control, PII anonymization, budget limits, and trajectory tracking

## Install

```bash
pip install "cua-agent[all]"
```

## Quick Start

```python
import asyncio
import os
from agent import ComputerAgent
from computer import Computer

async def main():
    # Set up computer instance
    async with Computer(
        os_type="linux",
        provider_type="cloud",
        name=os.getenv("CUA_CONTAINER_NAME"),
        api_key=os.getenv("CUA_API_KEY")
    ) as computer:

        # Create agent
        agent = ComputerAgent(
            model="anthropic/claude-3-5-sonnet-20241022",
            tools=[computer],
            only_n_most_recent_images=3,
            trajectory_dir="trajectories",
            max_trajectory_budget=5.0  # $5 budget limit
        )

        # Run agent
        messages = [{"role": "user", "content": "Take a screenshot and tell me what you see"}]

        async for result in agent.run(messages):
            for item in result["output"]:
                if item["type"] == "message":
                    print(item["content"][0]["text"])

if __name__ == "__main__":
    asyncio.run(main())
```

## Docs

- [Agent Loops](https://cua.ai/docs/agent-sdk/agent-loops)
- [Supported Agents](https://cua.ai/docs/agent-sdk/supported-agents/computer-use-agents)
- [Supported Models](https://cua.ai/docs/agent-sdk/supported-model-providers)
- [Chat History](https://cua.ai/docs/agent-sdk/chat-history)
- [Callbacks](https://cua.ai/docs/agent-sdk/callbacks)
- [Custom Tools](https://cua.ai/docs/agent-sdk/custom-tools)
- [Custom Computer Handlers](https://cua.ai/docs/agent-sdk/custom-computer-handlers)
- [Prompt Caching](https://cua.ai/docs/agent-sdk/prompt-caching)
- [Usage Tracking](https://cua.ai/docs/agent-sdk/usage-tracking)
- [Benchmarks](https://cua.ai/docs/agent-sdk/benchmarks)

## License

MIT License - see LICENSE file for details.
