# Ubuntu Docker Support in Cua with Kasm

_Published Aug 26, 2025 by Francesco Bonacci_

Today we’re shipping **Ubuntu Docker support** in Cua. You get a full Linux desktop inside a Docker container, viewable right in your browser—no VM spin-up, no extra clients. It behaves the same on macOS, Windows, and Linux.

<img src="./assets/docker-ubuntu-support.png" alt="Cua + KasmVNC Ubuntu container desktop">

## Why we did this

If you build automation or RL workflows with Cua, you’ve probably run into the usual platform walls: macOS VMs (via Lume) are Apple-Silicon only; Windows Sandbox needs Pro/Enterprise; giving agents your host desktop is… exciting, but risky; and little OS quirks make “build once, run anywhere” harder than it should be.

We wanted something lightweight, isolated, and identical across machines. So we put a desktop in a container.

## Why we didn’t use QEMU/KVM

Short answer: **portability, startup time, and ops friction.**

- **Runs everywhere, no hypervisor drama.** KVM needs Linux; Hyper-V/Virtualization.Framework setups vary by host and policy. Docker is ubiquitous across macOS/Windows/Linux and allowed in most CI runners—so your GUI env actually runs where your team works.
- **Faster boot & smaller footprints.** Containers cold-start in seconds and images are GB-scale; VMs tend to be minutes and tens of GB. That matters for parallel agents, CI, and local iteration.
- **Lower ops overhead.** No nested virt, kernel modules, or privileged host tweaks that many orgs (and cloud runners) block. Pull → run → browser.
- **Same image, everywhere.** One Docker image gives you an identical desktop on every dev laptop and in CI.
- **Web-first access out of the box.** KasmVNC serves the desktop over HTTP—no extra VNC/RDP clients or SPICE config.

**When we _do_ reach for QEMU/KVM:**

- You need **true OS isolation** or to run **non-Linux** guests.
- You want **kernel-level features** or **device/GPU passthrough** (VFIO).
- You’re optimizing for **hardware realism** over startup speed and density.

For this release, the goal was a **cross-platform Linux desktop that feels instant and identical** across local dev and CI. Containers + KasmVNC hit that sweet spot.

## What we built

Under the hood it’s **KasmVNC + Ubuntu 22.04 (Xfce) in Docker**, pre-configured for computer-use automation. You get a proper GUI desktop served over HTTP (no VNC/RDP client), accessible from any modern browser. Cua’s Computer server boots automatically so your agents can connect immediately.

### How it works (at a glance)

```
Your System
└─ Docker Container
   └─ Xfce Desktop + KasmVNC → open in your browser
```

---

## Quick start

1. **Install Docker** — Docker Desktop (macOS/Windows) or Docker Engine (Linux).

2. **Pull or build the image**

```bash
# Pull (recommended)
docker pull --platform=linux/amd64 trycua/cua-ubuntu:latest

# Or build locally
cd libs/kasm
docker build -t cua-ubuntu:latest .
```

3. **Run with Cua’s Computer SDK**

```python
from computer import Computer

computer = Computer(
    os_type="linux",
    provider_type="docker",
    image="trycua/cua-ubuntu:latest",
    name="my-automation-container"
)

await computer.run()
```

### Make an agent that drives this desktop

```python
from agent import ComputerAgent

# assumes `computer` is the instance created above
agent = ComputerAgent("openrouter/z-ai/glm-4.5v", tools=[computer])

async for _ in agent.run("Click on the search bar and type 'hello world'"):
    pass
```

> Use any VLM with tool use; just make sure your OpenRouter creds are set.

By default you land on **Ubuntu 22.04 + Xfce** with a browser and desktop basics, the **Computer server** is running, the **web viewer** is available at `http://localhost:8006`, and common automation tools are preinstalled.

---

## What’s inside (in plain English)

A tidy Linux desktop with web access through **KasmVNC**, Python 3.11 and dev tools, plus utilities you’ll actually use for automation—`wmctrl` for windows, `xclip` for clipboard, `ffmpeg` for media, screenshot helpers, and so on. It starts as a **non-root `kasm-user`**, lives in an **isolated filesystem** (unless you mount volumes), and ships with **SSL off for local dev** so you terminate TLS upstream when you deploy.

---

## How it compares

| Feature          | KasmVNC Docker        | Lume (macOS VM)       | Windows Sandbox        |
| ---------------- | --------------------- | --------------------- | ---------------------- |
| Platform support | macOS, Windows, Linux | macOS (Apple Silicon) | Windows Pro/Enterprise |
| Resource usage   | Low (container)       | Medium (full VM)      | Medium (full VM)       |
| Setup time       | \~30s                 | 2–5 min               | 1–2 min                |
| GUI desktop      | Linux                 | macOS                 | Windows                |
| Web access       | Browser (no client)   | Typically VNC client  | Typically RDP client   |
| Consistency      | Same everywhere       | Hardware-dependent    | OS-dependent           |

**Use KasmVNC Docker when…** you want the **same GUI env across devs/CI/platforms**, you’re doing **RL or end-to-end GUI tests**, or you need **many isolated desktops on one machine**.
**Use alternatives when…** you need native **macOS** (→ Lume) or native **Windows** (→ Windows Sandbox).

---

## Using the Agent Framework (parallel example)

A compact pattern for running multiple desktops and agents side-by-side:

```python
import asyncio
from computer import Computer
from agent import ComputerAgent

# Create multiple computer instances (each gets its own desktop)
computers = []
for i in range(3):
    c = Computer(
        os_type="linux",
        provider_type="docker",
        image="trycua/cua-ubuntu:latest",
        name=f"parallel-desktop-{i}"
    )
    computers.append(c)
    await c.run()

# Pair each desktop with a task
tasks = [
    "open github and search for 'trycua/cua'",
    "open a text editor and write 'hello world'",
    "open the browser and go to google.com",
]

agents = [
    ComputerAgent(model="openrouter/z-ai/glm-4.5v", tools=[c])
    for c in computers
]

async def run_agent(agent, task):
    async for _ in agent.run(task):
        pass

await asyncio.gather(*[run_agent(a, t) for a, t in zip(agents, tasks)])
```

---

## What’s next

We’re polishing a **CLI to push/scale these containers on Cua Cloud**, exploring **GPU acceleration** for in-container inference, and publishing **prebuilt images** for Playwright, Selenium, and friends.

---

## Try it

```python
from computer import Computer
computer = Computer(os_type="linux", provider_type="docker", image="trycua/cua-ubuntu:latest")
await computer.run()
```

---

## Links

- **Docker Provider Docs:** [https://docs.trycua.com/computers/docker](https://docs.trycua.com/computers/docker)
- **KasmVNC:** [https://github.com/kasmtech/KasmVNC](https://github.com/kasmtech/KasmVNC)
- **Container Source:** [https://github.com/trycua/cua/tree/main/libs/kasm](https://github.com/trycua/cua/tree/main/libs/kasm)
- **Computer SDK:** [https://docs.trycua.com/docs/computer-sdk/computers](https://docs.trycua.com/docs/computer-sdk/computers)
- **Discord:** [https://discord.gg/cua-ai](https://discord.gg/cua-ai)

Questions or weird edge cases? Ping us on Discord—we’re curious to see what you build.
