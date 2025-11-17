# Cloud Windows Sandboxes GA + macOS Preview

If you've been building with our `cua` libraries, you might've hit a limitation with local computer-use sandboxes: to run agents on Windows or macOS, you need to be on that OS—Windows Sandbox for Windows, Apple Virtualization for macOS. The only cross-platform option is Linux on Docker, which limits you to virtualizing Linux environments ([see all local options here](https://cua.ai/docs/computer-sdk/computers)).

Today the story changes - we're announcing general availability of **Cloud Windows Sandboxes** and opening early preview access for **Cloud macOS Sandboxes**.

## Cloud Windows Sandboxes: Now GA

![Cloud Windows Sandboxes](./assets/cloud-windows-ga.png)

Cloud Windows Sandboxes are now generally available. You get a full Windows 11 desktop in your browser with Edge and Python pre-installed, working seamlessly with all our [Computer-Use libraries](https://github.com/trycua/cua) for RPA, UI automation, code execution, and agent development.

**What's new with this release:**
- Hot-start under 1 second
- Direct noVNC over HTTPS under our sandbox.cua.ai domain
- 3 sandbox sizes available:

| Size | CPU | RAM | Storage |
|------|-----|-----|---------|
| Small | 2 cores | 8 GB | 128 GB SSD |
| Medium | 4 cores | 16 GB | 128 GB SSD |
| Large | 8 cores | 32 GB | 256 GB SSD |

<div align="center">
  <video src="./assets/demo_wsb.mp4" width="600" controls></video>
</div>

**Pricing:** Windows Sandboxes start at 8 credits/hour (Small), 15 credits/hour (Medium), or 31 credits/hour (Large).

## Cloud macOS Sandboxes: Now in Preview

Running macOS locally comes with challenges: 30GB golden images, a maximum of 2 sandboxes per host, and unpredictable compatibility issues. With Cloud macOS Sandboxes, we provision bare-metal macOS hosts (M1, M2, M4) on-demand—giving you full desktop access without the overhead of managing local sandboxes.

![macOS Preview Waitlist](./assets/macOS-waitlist.png)

**Preview access:** Invite-only. [Join the waitlist](https://cua.ai/macos-waitlist) if you're building agents for macOS workflows.

## Getting Started Today

Sign up at [cua.ai/signin](https://cua.ai/signin) and grab your API key from the dashboard. Then connect to a sandbox:

```python
from computer import Computer

computer = Computer(
    os_type="windows",      # or "macos"
    provider_type="cloud",
    name="my-sandbox",
    api_key="your-api-key"
)

await computer.run()
```

Manage existing sandboxes:

```python
from computer.providers.cloud.provider import CloudProvider

provider = CloudProvider(api_key="your-api-key")
async with provider:
    sandboxes = await provider.list_vms()
    await provider.run_vm("my-sandbox")
    await provider.stop_vm("my-sandbox")
```

Run an agent on Windows to automate a workflow:

```python
from agent import ComputerAgent

agent = ComputerAgent(
    model="anthropic/claude-sonnet-4-5-20250929",
    tools=[computer],
    max_trajectory_budget=5.0
)

response = await agent.run(
    "Open Excel, create a sales report with this month's data, and save it to the desktop"
)
```

## FAQs

<details>
<summary><strong>Why not just use local Windows Sandbox?</strong></summary>

Local Windows Sandbox resets on every restart. No persistence, no hot-start, and you need Windows Pro. Our sandboxes persist state, hot-start in under a second, and work from any OS.

</details>

<details>
<summary><strong>What happens to my work when I stop a sandbox?</strong></summary>

Everything persists. Files, installed software, browser profiles—it's all there when you restart. Only pay for runtime, not storage.

</details>

<details>
<summary><strong>How's the latency for UI automation?</strong></summary>

We run in 4 regions so you can pick what's closest. The noVNC connection is optimized for automation, not video streaming. Your agent sees crisp screenshots, not compressed video.

</details>

<details>
<summary><strong>Are there software restrictions?</strong></summary>

No. Full admin access on both platforms. Install whatever you need—Visual Studio, Photoshop, custom enterprise software. It's your sandbox.

</details>

## Need help?

If you hit issues getting either platform working, reach out in [Discord](https://discord.gg/cua-ai). We respond fast and fix based on what people actually use.

---

Get started at [cua.ai](https://cua.ai) or [join the macOS waitlist](https://cua.ai/macos-waitlist).
