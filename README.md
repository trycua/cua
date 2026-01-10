<div align="center">
  <a href="https://cua.ai" target="_blank" rel="noopener noreferrer">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="Cua logo" width="150" srcset="img/logo_white.png">
      <source media="(prefers-color-scheme: light)" alt="Cua logo" width="150" srcset="img/logo_black.png">
      <img alt="Cua logo" width="500" src="img/logo_black.png">
    </picture>
  </a>

  <p align="center">Build and deploy AI agents that can reason, plan and act on any Computers</p>

  <p align="center">
    <a href="https://cua.ai" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/badge/cua.ai-0ea5e9" alt="cua.ai"></a>
    <a href="https://discord.com/invite/cua-ai" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/badge/Discord-Join%20Server-10b981?logo=discord&logoColor=white" alt="Discord"></a>
    <a href="https://x.com/trycua" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/twitter/follow/trycua?style=social" alt="Twitter"></a>
    <a href="https://cua.ai/docs" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/badge/Docs-0ea5e9.svg" alt="Documentation"></a>
    <br>
<a href="https://trendshift.io/repositories/13685" target="_blank"><img src="https://trendshift.io/api/badge/repositories/13685" alt="trycua%2Fcua | Trendshift" style="width: 250px; height: 55px;" width="250" height="55"/></a>
  </p>

</div>

**Cua** ("koo-ah") is an open-source platform for building computer-use agents—AI systems that see screens, click buttons, type, and complete tasks autonomously.

<div align="center">
  <video src="https://github.com/user-attachments/assets/c619b4ea-bb8e-4382-860e-f3757e36af20" width="600" controls></video>
</div>

## The Cua Stack

| Component | Description | Docs |
|-----------|-------------|------|
| **Desktop Sandboxes** | Isolated environments for safe agent execution—cloud, Docker, macOS VMs, or Windows Sandbox | [Set up a Sandbox](https://cua.ai/docs/cua/guide/get-started/set-up-sandbox) |
| **Computer SDK** | Unified API for screenshots, mouse, keyboard, and file operations across all platforms | [Computer SDK](https://cua.ai/docs/cua/reference/computer-sdk) |
| **Agent SDK** | Framework for 100+ vision-language models with composable grounding and planning | [Agent SDK](https://cua.ai/docs/cua/reference/agent-sdk) |

## Quick Start

```bash
pip install cua-agent[all] cua-computer
```

```python
from computer import Computer
from agent import ComputerAgent

computer = Computer(os_type="linux", provider_type="cloud")
agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929", computer=computer)

async for result in agent.run([{"role": "user", "content": "Open Firefox and search for Cua"}]):
    print(result)
```

**Get started:**
- [Clone a starter template](https://github.com/trycua/agent-template) and run in <1 min
- [Full quickstart guide](https://cua.ai/docs/cua/guide/get-started/set-up-sandbox)
- [Supported models](https://cua.ai/docs/cua/reference/agent-sdk/supported-models)

## Packages

| Package | Description | Docs |
|---------|-------------|------|
| [cua-agent](./libs/python/agent) | AI agent framework for computer-use tasks | [Guide](https://cua.ai/docs/cua/guide/fundamentals/agent-loop) |
| [cua-computer](./libs/python/computer) | SDK for controlling desktop environments | [Guide](https://cua.ai/docs/cua/guide/fundamentals/computer-interface) |
| [cua-mcp-server](./libs/python/mcp-server) | MCP server for Claude Desktop, Cursor, etc. | [Guide](https://cua.ai/docs/cua/reference/mcp-server) |
| [lume](./libs/lume) | macOS/Linux VM management on Apple Silicon | [Docs](https://cua.ai/docs/lume) |
| [lumier](./libs/lumier) | Docker-compatible interface for Lume VMs | [README](./libs/lumier/README.md) |

## Python Version

Cua requires **Python 3.12 or 3.13**. Python 3.14 is not yet supported.

## Resources

- [Documentation](https://cua.ai/docs) — Guides, examples, and API reference
- [Blog](https://www.cua.ai/blog) — Tutorials, updates, and research
- [Discord](https://discord.com/invite/mVnXXpdE85) — Community support and discussions
- [GitHub Issues](https://github.com/trycua/cua/issues) — Bug reports and feature requests

## Contributing

We welcome contributions! See our [Contributing Guidelines](CONTRIBUTING.md) for details.

## License

MIT License — see [LICENSE](LICENSE.md) for details.

Third-party components have their own licenses:
- [Kasm](libs/kasm/LICENSE) (MIT)
- [OmniParser](https://github.com/microsoft/OmniParser/blob/master/LICENSE) (CC-BY-4.0)
- Optional `cua-agent[omni]` includes ultralytics (AGPL-3.0)

## Trademarks

Apple, macOS, Ubuntu, Canonical, and Microsoft are trademarks of their respective owners. This project is not affiliated with or endorsed by these companies.

---

<div align="center">

[![Stargazers over time](https://starchart.cc/trycua/cua.svg?variant=adaptive)](https://starchart.cc/trycua/cua)

Thank you to all our [GitHub Sponsors](https://github.com/sponsors/trycua)!

<img width="300" alt="coderabbit-cli" src="https://github.com/user-attachments/assets/23a98e38-7897-4043-8ef7-eb990520dccc" />

</div>
