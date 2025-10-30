# From Lume to Containerization: Our Journey Meets Apple's Vision

_Published on June 10, 2025 by Francesco Bonacci_

Yesterday, Apple announced their new [Containerization framework](https://github.com/apple/containerization) at WWDC. Since then, our Discord and X users have been asking what this means for Cua virtualization capabilities on Apple Silicon. We've been working in this space for months - from [Lume](https://github.com/trycua/cua/tree/main/libs/lume) to [Lumier](https://github.com/trycua/cua/tree/main/libs/lumier) to [Cua Cloud Sandbox](./introducing-cua-cloud-containers). Here's our take on Apple's announcement.

## Our Story

When we started Cua, we wanted to solve a simple problem: make it easy to run VMs on Apple Silicon, with a focus on testing and deploying computer-use agents without dealing with complicated setups.

We decided to build on Apple's Virtualization framework because it was fast and well-designed. This became Lume, which we launched on [Hacker News](https://news.ycombinator.com/item?id=42908061).

Four months later, we're happy with our choice. Users are running VMs with great performance and low memory usage. Now Apple's new [Containerization](https://github.com/apple/containerization) framework builds on the same foundation - showing we were on the right track.

## What Apple Announced

Apple's Containerization framework changes how containers work on macOS. Here's what's different:

### How It Works

Instead of running all containers in one shared VM (like Docker or Colima), Apple runs each container in its own tiny VM:

```bash
How Docker Works:
┌─────────────────────────────────┐
│         Your Mac                │
├─────────────────────────────────┤
│      One Big Linux VM           │
├─────────────────────────────────┤
│ Container 1 │ Container 2 │ ... │
└─────────────────────────────────┘

How Apple's Framework Works:
┌─────────────────────────────────┐
│         Your Mac                │
├─────────────────────────────────┤
│ Mini VM 1 │ Mini VM 2 │ Mini VM 3│
│Container 1│Container 2│Container 3│
└─────────────────────────────────┘
```

Why is this better?

- **Better security**: Each container is completely separate
- **Better performance**: Each container gets its own resources
- **Real isolation**: If one container has problems, others aren't affected

> **Note**: You'll need macOS Tahoe 26 Preview or later to use all features. The new [VZVMNetNetworkDeviceAttachment](https://developer.apple.com/documentation/virtualization/vzvmnetnetworkdeviceattachment) API required to fully implement the above architecture is only available there.

### The Technical Details

Here's what makes it work:

- **vminitd**: A tiny program that starts up each container VM super fast
- **Fast boot**: These mini VMs start in less than a second
- **Simple storage**: Containers are stored as ready-to-use disk images

Instead of using big, slow startup systems, Apple created something minimal. Each container VM boots with just what it needs - nothing more.

The `vminitd` part is really clever. It's the first thing that runs in each mini VM and lets the container talk to the outside world. It handles everything the container needs to work properly.

### What About GPU Passthrough?

Some developers found hints in macOS Tahoe that GPU support might be coming, through a symbol called `_VZPCIDeviceConfiguration` in the new version of the Virtualization framework. This could mean we'll be able to use GPUs inside containers and VMs soon. Imagine running local models using Ollama or LM Studio! We're not far from having fully local and isolated computer-use agents.

## What We've Built on top of Apple's Virtualization Framework

While Apple's new framework focuses on containers, we've been building VM management tools on top of the same Apple Virtualization framework. Here's what we've released:

### Lume: Simple VM Management

[Lume](https://github.com/trycua/cua/tree/main/libs/lume) is our command-line tool for creating and managing VMs on Apple Silicon. We built it because setting up VMs on macOS was too complicated.

What Lume does:

- **Direct control**: Works directly with Apple's Virtualization framework
- **Ready-to-use images**: Start a macOS or Linux VM with one command
- **API server**: Control VMs from other programs (runs on port 7777)
- **Smart storage**: Uses disk space efficiently
- **Easy install**: One command to get started
- **Share images**: Push your VM images to registries like Docker images

```bash
# Install Lume
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"

# Start a macOS VM
lume run macos-sequoia-vanilla:latest
```

### Lumier: Docker-Style VM Management

[Lumier](https://github.com/trycua/lumier) works differently. It lets you use Docker commands to manage VMs. But here's the key: **Docker is just for packaging, not for isolation**.

What makes Lumier useful:

- **Familiar commands**: If you know Docker, you know Lumier
- **Web access**: Connect to your VM through a browser
- **Save your work**: VMs remember their state
- **Share files**: Easy to move files between your Mac and the VM
- **Automation**: Script your VM setup

```bash
# Run a macOS VM with Lumier
docker run -it --rm \
    --name macos-vm \
    -p 8006:8006 \
    -e VM_NAME=macos-vm \
    -e VERSION=ghcr.io/trycua/macos-sequoia-cua:latest \
    trycua/lumier:latest
```

## Comparing the Options

Let's see how these three approaches stack up:

### How They're Built

```bash
Apple Containerization:
Your App → Container → Mini VM → Mac Hardware

Lume:
Your App → Full VM → Mac Hardware

Lumier:
Docker → Lume → Full VM → Mac Hardware
```

### When to Use What

**Apple's Containerization**

- ✅ Perfect for: Running containers with maximum security
- ✅ Starts in under a second
- ✅ Uses less memory and CPU
- ❌ Needs macOS Tahoe 26 Preview
- ❌ Only for containers, not full VMs

**Lume**

- ✅ Perfect for: Development and testing
- ✅ Full control over macOS/Linux VMs
- ✅ Works on current macOS versions
- ✅ Direct access to everything
- ❌ Uses more resources than containers

**Lumier**

- ✅ Perfect for: Teams already using Docker
- ✅ Easy to share and deploy
- ✅ Access through your browser
- ✅ Great for automated workflows
- ❌ Adds an extra layer of complexity

### Using Them Together

Here's the cool part - you can combine these tools:

1. **Create a VM**: Use Lume to set up a macOS VM
2. **Run containers**: Use Apple's framework inside that VM (works on M3+ Macs with nested virtualization)

You get the best of both worlds: full VM control plus secure containers.

## What's Next for Cua?

Apple's announcement confirms we're on the right path. Here's what we're looking forward to:

1. **Faster VMs**: Learning from Apple's super-fast container startup, and whether some learnings can be applied to macOS VMs
2. **GPU support**: Getting ready for GPU passthrough when `_VZPCIDeviceConfiguration` is made available, realistically in a stable release of macOS Tahoe 26

## Learn More

- [Apple Containerization Framework](https://github.com/apple/containerization)
- [Lume - Direct VM Management](https://github.com/trycua/cua/tree/main/libs/lume)
- [Lumier - Docker Interface for VMs](https://github.com/trycua/cua/tree/main/libs/lumier)
- [Cua Cloud Sandbox](https://trycua.com)
- [Join our Discord](https://discord.gg/cua-ai)

---

_Questions about virtualization on Apple Silicon? Come chat with us on Discord!_
