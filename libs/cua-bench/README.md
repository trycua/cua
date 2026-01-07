# cua-bench

A toolkit for verifiable cross-platform computer-use RL environments and benchmarks. Supports real Windows, Linux, macOS, and Android VM environments, as well as lightweight HTML-based webtop environments.

## Installation

```bash
# Install cua-bench CLI
pip install -e .

# Or with uv
uv pip install -e .

# Install playwright (for webtop environments)
playwright install chromium
```

## Quick Start

### Base Images

Base images are reproducible environments for running benchmarks. The naming convention indicates the underlying technology:

| Type | Command | Description | Requirements |
|------|---------|-------------|--------------|
| `linux-docker` | `cb image create linux-docker` | Linux GUI container (XFCE) | Docker |
| `linux-qemu` | `cb image create linux-qemu` | Linux VM with QEMU/KVM | Docker + KVM |
| `windows-qemu` | `cb image create windows-qemu --download-iso` | Windows 11 VM | Docker + KVM |
| `android-qemu` | `cb image create android-qemu` | Android VM | Docker + KVM |
| `macos-lume` | `cb image create macos-lume` | macOS VM via Lume | Apple Silicon |

### Create an Image

```bash
# Quick setup (Linux container, no KVM required)
cb image create linux-docker

# Windows VM setup (~1-2 hours, downloads Windows ISO)
cb image create windows-qemu --download-iso

# Check available images
cb image list
```

### Start a Sandbox

```bash
# Start sandbox from image
cb sandbox start linux-docker
cb sandbox start windows-qemu

# Start with custom name
cb sandbox start windows-qemu --name my-test

# List running sandboxes
cb sandbox list

# Connect to sandbox (opens VNC in browser)
cb sandbox vnc linux-docker

# Stop sandbox
cb sandbox stop linux-docker
```

### Manage Images

```bash
# List all images
cb image list

# Clone for customization
cb image clone windows-qemu windows-custom

# Show image details
cb image info windows-qemu

# Delete an image
cb image delete windows-custom
```

## Running Benchmarks

### Run Tasks

```bash
# Run a task with agent evaluation
cb run tasks/my_task --agent cua-agent --model anthropic/claude-sonnet-4-20250514

# Run with oracle solution
cb run tasks/my_task --oracle

# Run on specific image
cb run tasks/winarena_adapter --image windows-qemu

# Run on cloud (coming soon)
cb run tasks/my_task --on cloud
```

### Supported Benchmarks

| Benchmark | Image | Tasks | Description |
|-----------|--------------|-------|-------------|
| Windows Arena | `windows-qemu` | 173 | Windows 11 desktop automation across 12 apps |
| OSWorld | `linux-qemu` | 369 | Linux desktop automation |
| OS-Harm | `linux-qemu` | 150+ | Security/safety evaluation |
| WebArena | `linux-docker` | 812 | Web browsing tasks |
| AndroidWorld | `android-qemu` | 200+ | Android mobile automation |
| macOSWorld | `macos-lume` | 100+ | macOS desktop automation |

### Windows Arena

Run Windows desktop automation benchmarks with 173 tasks across 12 application domains.

**Requirements:** x86_64 Linux with KVM support (nested virtualization).

```bash
# Create base image (~1-2 hours)
cb image create windows-qemu --download-iso

# Start sandbox
cb sandbox start windows-qemu

# List available tasks
cb run tasks/winarena_adapter --list

# Run a specific task variant
cb run task tasks/winarena_adapter --variant-id 0

# Stop when done
cb env stop windows-qemu
```

## Creating Tasks

### Generate a Task

```bash
# AI-assisted task generation
cb generate-task "2048 game with icons" --output tasks/2048_icons_env

# Create empty task scaffold
cb create-task tasks/my_env
```

### Interact with Tasks

```bash
# Interactive mode (opens browser)
cb interact tasks/my_env

# With specific task variant
cb interact tasks/my_env --variant-id 0

# Run oracle and save screenshot
cb interact tasks/my_env --oracle --screenshot output.png
```

## Session Management

```bash
# List all sessions
cb sessions

# View session logs
cb sessions logs <session_id>

# Open log directory
cb sessions open <session_id>

# Stop a session
cb sessions stop <session_id>

# Clean up inactive sessions
cb sessions --clean
```

## Dataset Export

Export snapshots to training datasets for UI grounding:

```bash
# Process with aguvis-stage-1 format (default)
cb process ./outputs --save-dir ./datasets

# Process with gui-r1 format
cb process ./outputs --mode gui-r1

# Push to Hugging Face Hub
cb process ./outputs --push-to-hub --repo-id username/repo
```

**Available Processors:**

- **`aguvis-stage-1`** - Action augmentation dataset (AgUVis/smol2operator style)
- **`gui-r1`** - Low-level click instructions (GUI-R1 format)

## Programmatic Interface

```python
import cua_bench as cb

# Create an environment
env = cb.make("tasks/click_env")

# Setup and get initial screenshot
screenshot, task_cfg = await env.reset(task_id=0)

# Execute a step
screenshot = await env.step('page.click("#submit")')

# Run the solution
screenshot = await env.solve()

# Evaluate the result
rewards = await env.evaluate()

# Clean up
await env.close()
```

### Python API for Running Environments

```python
from cua_bench.cli.commands.env import get_computer

# Connect to a running environment
computer = await get_computer("windows-qemu")

# Take screenshot
screenshot = await computer.interface.screenshot()

# Execute actions
await computer.interface.left_click(100, 200)
await computer.interface.type_text("Hello world")
await computer.interface.key("Enter")
```

## CLI Reference

| Command | Description |
|---------|-------------|
| `cb image create <platform>` | Create base image |
| `cb image list` | List available images |
| `cb image clone <src> <dst>` | Clone an image |
| `cb image delete <name>` | Delete an image |
| `cb sandbox start <image>` | Start sandbox from image |
| `cb sandbox stop <id>` | Stop sandbox |
| `cb sandbox list` | List running sandboxes |
| `cb sandbox vnc <id>` | Open VNC viewer |
| `cb run <task>` | Run task with evaluation |
| `cb interact <task>` | Interactive task mode |
| `cb process <dir>` | Export training data |

For full CLI documentation, see [docs/CLI_REFERENCE.md](docs/CLI_REFERENCE.md).

## Documentation

- [CLI Reference](docs/CLI_REFERENCE.md) - Complete CLI specification
- [Local Testing Guide](docs/LOCAL_TESTING.md) - Testing on remote VMs
- [Windows Arena](tasks/winarena_adapter/README.md) - Windows Arena adapter docs
