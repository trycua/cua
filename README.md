<div align="center">
  <a href="https://cua.ai" target="_blank" rel="noopener noreferrer">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="Cua logo" width="150" srcset="img/logo_white.svg">
      <source media="(prefers-color-scheme: light)" alt="Cua logo" width="150" srcset="img/logo_black.svg">
      <img alt="Cua logo" width="150" src="img/logo_black.svg">
    </picture>
  </a>

  <p align="center"><strong>Give AI agents computers they can use.</strong><br>Cua provides open-source desktop automation, isolated cloud desktops, local macOS VMs, and benchmarks for evaluating computer-use agents.</p>

  <p align="center"><strong><a href="https://run.cua.ai/?utm_source=github&utm_medium=referral&utm_campaign=fleet_activation&content_id=repo_readme" target="_blank" rel="noopener noreferrer">Try Cua Fleets now at run.cua.ai</a></strong></p>

  <p align="center">
    <a href="https://cua.ai" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/badge/cua.ai-0ea5e9" alt="cua.ai"></a>
    <a href="https://discord.gg/mVnXXpdE85" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/badge/Discord-Join%20Server-10b981?logo=discord&logoColor=white" alt="Discord"></a>
    <a href="https://x.com/trycua" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/twitter/follow/trycua?style=social" alt="Twitter"></a>
    <a href="https://cua.ai/docs" target="_blank" rel="noopener noreferrer"><img src="https://img.shields.io/badge/Docs-0ea5e9.svg" alt="Documentation"></a>
    <br>
<a href="https://trendshift.io/repositories/13685" target="_blank"><img src="https://trendshift.io/api/badge/repositories/13685" alt="trycua%2Fcua | Trendshift" style="width: 250px; height: 55px;" width="250" height="55"/></a>
  </p>

</div>

## Choose your path

<div align="center">
  <table>
    <tr>
      <td colspan="3" align="center">
        <a href="#cua-fleets">
          <img src="img/card-cua-fleets.gif" alt="Cua Fleets: isolated cloud desktops for your agents" width="790">
        </a>
      </td>
    </tr>
    <tr>
      <td align="center">
        <a href="#cua-driver">
          <img src="img/card-cua-driver.gif" alt="Cua Driver: inspect and operate apps on macOS, Windows, and Linux" width="246">
        </a>
      </td>
      <td align="center">
        <a href="#lume">
          <img src="img/card-cua-lume.gif" alt="Lume: local macOS and Linux VMs on Apple Silicon" width="246">
        </a>
      </td>
      <td align="center">
        <a href="#cua-bench">
          <img src="img/card-cua-bench.gif" alt="Cua Bench: create tasks, evaluate agents, and export trajectories" width="246">
        </a>
      </td>
    </tr>
  </table>
</div>

- **Cua Fleets:** [Provision a Linux desktop, run a command, and save a screenshot](https://cua.ai/docs/tutorials/your-first-cloud-fleet).
- **Cua Driver:** [Operate Calculator and verify its result](https://cua.ai/docs/tutorials/drive-your-first-app).
- **Lume:** [Create a Tahoe VM and connect over SSH](https://cua.ai/docs/tutorials/create-your-first-lume-vm).
- **Cua Bench:** [Create and verify a simulated task](https://cua.ai/docs/tutorials/your-first-cua-bench-task).

You bring the agent and model. Cua provides the computer and automation tools. [Computer-Use 2.0](https://cua.ai/docs/concepts/what-is-computer-use) describes an agent moving between code, APIs, and graphical interfaces within the same task.

## See Cua Driver in action

Two Cua Driver sessions select cells in LibreOffice Calc and objects in Inkscape on an Omarchy desktop while a terminal stays in the foreground. Watch the 50-second demo, then explore [Omarchy on Fleet](https://cua.ai/docs/how-to-guides/sandbox/run-omarchy-on-cloud-fleet).

https://github.com/user-attachments/assets/b4e5517c-d2db-4758-b4cf-07131b0753b2

---

## Cua Fleets

Provision isolated cloud desktops at [run.cua.ai](https://run.cua.ai/?utm_source=github&utm_medium=referral&utm_campaign=fleet_activation&content_id=repo_readme). A Fleet maintains sandbox capacity; your code claims a desktop from a pool and uses the Sandbox SDK to run commands, capture screenshots, and interact with apps inside it.

**Your first result:** provision a Linux desktop, run `uname -a`, save a screenshot, and delete the cloud resources. The tutorial covers Fleet credentials, dependencies, and cleanup. Pools can retain paid capacity after a claim ends, so follow its cleanup steps.

Local sandboxes and Fleets share the Sandbox SDK, but credentials, images, operations, and runtime requirements differ. Use the [runtime support reference](https://cua.ai/docs/reference/sandbox-sdk/runtime-support) to choose an environment. For your own hardware, see [Manage local sandbox lifecycle](https://cua.ai/docs/how-to-guides/sandbox/manage-local-lifecycle).

**[Your first Cloud Fleet](https://cua.ai/docs/tutorials/your-first-cloud-fleet)** | **[Fleet overview](https://cua.ai/docs/cloud-fleets)** | **[Sandbox SDK reference](https://cua.ai/docs/reference/sandbox-sdk)**

---

## Cua Driver

Give your agent tools to inspect and operate native desktop apps and browsers on macOS, Windows, and Linux. Connect through the CLI, MCP, or typed SDKs. Background delivery lets agents work without moving your pointer or taking focus when the app and platform support it; see [platform support](https://cua.ai/docs/reference/cua-driver/platform-support) for the boundaries.

**macOS / Linux**

```sh
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
```

**Windows (PowerShell)**

```powershell
irm https://cua.ai/driver/install.ps1 | iex
```

**Your first result:** connect your agent, ask it to compute 6 × 7 in Calculator, and have it verify that the app displays 42. The tutorial covers platform setup, permissions, and agent connection.

**[Drive your first app](https://cua.ai/docs/tutorials/drive-your-first-app)** | **[Installation](https://cua.ai/docs/how-to-guides/driver/install)** | **[CLI Reference](https://cua.ai/docs/reference/cua-driver/cli-reference)**

Using Claude Code, Codex, Cursor, OpenClaw, or another agent? [Find your integration](https://cua.ai/docs/how-to-guides/driver/connect-your-agent). Source documentation and architecture notes live in [`libs/cua-driver/README.md`](libs/cua-driver/README.md).

---

## Lume

Create and manage local macOS and Linux VMs on Apple Silicon using Apple's Virtualization.Framework.

```bash
/bin/bash -c "$(curl -fsSL https://cua.ai/lume/install.sh)"
```

**Your first result:** create a vanilla macOS Tahoe VM from an Apple restore image, start it, and connect over SSH. The tutorial uses the Lume CLI directly and explains the unattended setup defaults.

**[Create your first Lume VM](https://cua.ai/docs/tutorials/create-your-first-lume-vm)** | **[Installation](https://cua.ai/docs/how-to-guides/lume/install-lume)** | **[CLI reference](https://cua.ai/docs/reference/lume/cli-reference)**

---

## Cua Bench

Build computer-use tasks, evaluate agents, and export trajectories for training. Start with a simulated task that requires no VM, Docker, or model API key.

With Python 3.12 or 3.13 and [uv](https://docs.astral.sh/uv/) installed:

```bash
uv tool install 'cua-bench[browser]'
uv tool run --from 'cua-bench[browser]' playwright install chromium
```

**Your first result:** create a small task, run its reference solution, and verify that its evaluator reports a reward of `1.0`. Then try the same task yourself.

**[Build your first task](https://cua.ai/docs/tutorials/your-first-cua-bench-task)** | **[What is Cua-Bench?](https://cua.ai/docs/concepts/what-is-cua-bench)** | **[CLI reference](https://cua.ai/docs/reference/cua-bench/cli-reference)** | **[Partner with us](https://cuabench.ai/)**

---

## Packages

| Package                                                  | Description                                                       |
| -------------------------------------------------------- | ----------------------------------------------------------------- |
| [cua-driver](libs/cua-driver/README.md)                  | Desktop automation tools for macOS, Windows, and Linux            |
| [cua-agent](libs/python/agent)                           | AI agent framework for computer-use tasks                         |
| [cua-sandbox](https://cua.ai/docs/reference/sandbox-sdk) | SDK for creating and controlling local and cloud sandboxes        |
| [cua-fleet](libs/python/cua-fleet/README.md)             | Python SDK for Fleet infrastructure and sandbox pools             |
| [cua-computer-server](libs/python/computer-server)       | Guest service for UI interactions and code execution in sandboxes |
| [cua-bench](libs/cua-bench/README.md)                    | Benchmarks and RL environments for computer-use                   |
| [lume](https://cua.ai/docs/reference/lume/cli-reference) | macOS/Linux VM management on Apple Silicon                        |
| [lumier](libs/lumier/README.md)                          | Docker-compatible interface for Lume VMs                          |

## Resources

- [Documentation](https://cua.ai/docs) — Guides, examples, and API reference
- [Blog](https://cua.ai/blog) — Tutorials, updates, and research
- [Discord](https://discord.com/invite/mVnXXpdE85) — Community support and discussions
- [GitHub Issues](https://github.com/trycua/cua/issues) — Bug reports and feature requests
- [Security](SECURITY.md) — Private vulnerability reporting

## Citation

If Cua supports your research, please cite the software:

```bibtex
@software{cua2025,
  author  = {{Cua AI, Inc.}},
  title   = {Cua},
  year    = {2025},
  url     = {https://github.com/trycua/cua},
  license = {MIT}
}
```

For reproducibility, include the Cua release or commit used in your experiments. Citation metadata is also available in [`CITATION.cff`](CITATION.cff).

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

Thank you to all our [GitHub Sponsors](https://github.com/sponsors/trycua)!

<img width="300" alt="coderabbit-cli" src="https://github.com/user-attachments/assets/23a98e38-7897-4043-8ef7-eb990520dccc" />

</div>
