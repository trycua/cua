<div align="center">
  <a href="https://cua.ai" target="_blank" rel="noopener noreferrer">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="Cua logo" width="150" srcset="img/logo_white.svg">
      <source media="(prefers-color-scheme: light)" alt="Cua logo" width="150" srcset="img/logo_black.svg">
      <img alt="Cua logo" width="150" src="img/logo_black.svg">
    </picture>
  </a>

  <p align="center">Build, benchmark, and deploy agents that use computers</p>

  <p align="center">
    <a href="https://cua.ai" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/badge/cua.ai-0ea5e9" alt="cua.ai"></a>
    <a href="https://discord.gg/mVnXXpdE85" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/badge/Discord-Join%20Server-10b981?logo=discord&logoColor=white" alt="Discord"></a>
    <a href="https://x.com/trycua" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/twitter/follow/trycua?style=social" alt="Twitter"></a>
    <a href="https://cua.ai/docs" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/badge/Docs-0ea5e9.svg" alt="Documentation"></a>
    <br>
<a href="https://trendshift.io/repositories/13685" target="_blank"><img src="https://trendshift.io/api/badge/repositories/13685" alt="trycua%2Fcua | Trendshift" style="width: 250px; height: 55px;" width="250" height="55"/></a>
  </p>

</div>

## Choose Your Path

<div align="center">
  <table>
    <tr>
      <td colspan="3" align="center">
        <a href="#cua-drivers---background-computer-use-on-macos-and-windows-with-linux-pre-release">
          <img src="img/card-cua-driver.gif" alt="Cua Drivers — background computer-use for any agent" width="790">
        </a>
      </td>
    </tr>
    <tr>
      <td align="center">
        <a href="#cua---agent-ready-sandboxes-for-any-os">
          <img src="img/card-cua-sandbox.gif" alt="Cua &amp; Cua Sandbox" width="246">
        </a>
      </td>
      <td align="center">
        <a href="#cua-bench---benchmarks--rl-environments">
          <img src="img/card-cua-bench.gif" alt="Cua Bench" width="246">
        </a>
      </td>
      <td align="center">
        <a href="#lume---macos-virtualization">
          <img src="img/card-cua-lume.gif" alt="Lume" width="246">
        </a>
      </td>
    </tr>
  </table>
  <p>
    <strong>Building your own agent?</strong> Start with <a href="#cua---agent-ready-sandboxes-for-any-os">Cua</a> ·
    <strong>Giving a coding agent a computer?</strong> <a href="#cua-drivers---background-computer-use-on-macos-and-windows-with-linux-pre-release">Cua Drivers</a> ·
    <strong>Evaluating or training models?</strong> <a href="#cua-bench---benchmarks--rl-environments">Cua Bench</a> ·
    <strong>Need macOS VMs?</strong> <a href="#lume---macos-virtualization">Lume</a>
  </p>
</div>

---

## Cua Drivers - Background computer-use on macOS and Windows, with Linux pre-release

Drive native desktop apps **in the background**. Agents click, type, and verify without stealing the cursor or focus. Use the same CLI and MCP server on macOS and Windows from Claude Code, Cursor, Codex, OpenClaw, and custom clients. Linux support is available as a pre-release backend while platform testing is still in progress.

**macOS / Linux**

```sh
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh)"
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.ps1 | iex
```

Then wire it into Claude Code as an MCP server and your agent can drive the desktop in the background:

```bash
claude mcp add --transport stdio cua-driver -- cua-driver mcp
```

Full tool reference, architecture notes, and the optional agent skill pack live here: [`libs/cua-driver/README.md`](libs/cua-driver/README.md).

---

## Cua - Agent-Ready Sandboxes for Any OS

Build agents that see screens, click buttons, and complete tasks autonomously. One API for any VM or container image — cloud or local.

```sh
pip install cua
```


```python
# Requires Python 3.11 or later
from cua import Sandbox, Image

# Same API regardless of OS or runtime
async with Sandbox.ephemeral(Image.linux()) as sb:   # or .macos() .windows() .android()
    result = await sb.shell.run("echo hello")
    screenshot = await sb.screenshot()
    await sb.mouse.click(100, 200)
    await sb.keyboard.type("Hello from Cua!")
    await sb.mobile.gesture((100, 500), (100, 200))  # multi-touch gestures
```

|                    | Linux container | Linux VM | macOS | Windows | Android | BYOI (.qcow2, .iso) |
| ------------------ | --------------- | -------- | ----- | ------- | ------- | ------------------- |
| **Cloud (cua.ai)** | ✅              | ✅       | ✅    | ✅      | ✅      | 🔜 soon             |
| **Local (QEMU)**   | ✅              | ✅       | ✅    | ✅      | ✅      | ✅                  |

**[Get Started](https://cua.ai/docs/cua/guide/get-started/set-up-sandbox)** | **[Examples](https://cua.ai/docs/cua/examples)** | **[API Reference](https://cua.ai/docs/cua/reference/agent-sdk)**

---

## Cua-Bench - Benchmarks & RL Environments

Evaluate computer-use agents on OSWorld, ScreenSpot, Windows Arena, and custom tasks. Export trajectories for training.


```bash
# Clone, install, and create base image
git clone https://github.com/trycua/cua && cd cua/cua-bench
uv tool install -e . && cb image create linux-docker

# Run benchmark with agent
cb run dataset datasets/cua-bench-basic --agent cua-agent --max-parallel 4
```

**[Get Started](https://cua.ai/docs/cuabench/guide/getting-started/first-steps)** | **[Partner With Us](https://cuabench.ai/)** | **[Registry](https://cuabench.ai/registry)** | **[CLI Reference](https://cua.ai/docs/cuabench/reference/cli-reference)**

---

## Lume - macOS Virtualization

Create and manage macOS/Linux VMs with near-native performance on Apple Silicon using Apple's Virtualization.Framework.


```bash
# Install Lume
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"

# Pull & start a macOS VM
lume run macos-sequoia-vanilla:latest
```

**[Get Started](https://cua.ai/docs/lume)** | **[FAQ](https://cua.ai/docs/lume/guide/getting-started/faq)** | **[CLI Reference](https://cua.ai/docs/lume/reference/cli-reference)**

---

## Packages

| Package                                                                     | Description                                                |
| --------------------------------------------------------------------------- | ---------------------------------------------------------- |
| [cua-driver](libs/cua-driver/README.md)                                     | Background computer-use agent for macOS, Windows, and Linux |
| [cua-agent](https://cua.ai/docs/cua/reference/agent-sdk)                    | AI agent framework for computer-use tasks                  |
| [cua-sandbox](https://cua.ai/docs/cua/reference/sandbox-sdk)                | SDK for creating and controlling sandboxes                 |
| [cua-computer-server](https://cua.ai/docs/cua/reference/sandbox-sdk)        | Driver for UI interactions and code execution in sandboxes |
| [cua-bench](https://cua.ai/docs/cuabench)                                   | Benchmarks and RL environments for computer-use            |
| [lume](https://cua.ai/docs/lume)                                            | macOS/Linux VM management on Apple Silicon                 |
| [lumier](https://cua.ai/docs/lume/guide/advanced/lumier)                    | Docker-compatible interface for Lume VMs                   |

## Resources

- [Documentation](https://cua.ai/docs) — Guides, examples, and API reference
- [Blog](https://cua.ai/blog) — Tutorials, updates, and research
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
