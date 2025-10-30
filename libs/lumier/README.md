<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="../../img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="../../img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

[![Swift 6](https://img.shields.io/badge/Swift_6-F54A2A?logo=swift&logoColor=white&labelColor=F54A2A)](#)
[![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)

</h1>
</div>

macOS and Linux virtual machines in a Docker container.

<div align="center">
  <video src="https://github.com/user-attachments/assets/2ecca01c-cb6f-4c35-a5a7-69bc58bd94e2" width="800" controls></video>
</div>

## What is Lumier?

**Lumier** is an interface for running macOS virtual machines with minimal setup. It uses Docker as a packaging system to deliver a pre-configured environment that connects to the `lume` virtualization service running on your host machine. With Lumier, you get:

- A ready-to-use macOS or Linux virtual machine in minutes
- Browser-based VNC access to your VM
- Easy file sharing between your host and VM
- Simple configuration through environment variables

## Requirements

Before using Lumier, make sure you have:

1. **Docker for Apple Silicon** - download it [here](https://desktop.docker.com/mac/main/arm64/Docker.dmg) and follow the installation instructions.

2. **Lume** - This is the virtualization CLI that powers Lumier. Install it with this command:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"
```

## Getting Started

```bash
# Run the container with temporary storage (using pre-built image from Docker Hub)
docker run -it --rm \
    --name macos-vm \
    -p 8006:8006 \
    -e VM_NAME=macos-vm \
    -e VERSION=ghcr.io/trycua/macos-sequoia-cua:latest \
    -e CPU_CORES=4 \
    -e RAM_SIZE=8192 \
    trycua/lumier:latest
```

After running the command above, you can access your macOS VM through a web browser (e.g., http://localhost:8006).

> **Note:** With the basic setup above, your VM will be reset when you stop the container (ephemeral mode). This means any changes you make inside the macOS VM will be lost. See [the documentation](https://trycua.com/docs/libraries/lumier/docker) for how to save your VM state.

## Docs

- [Installation](https://trycua.com/docs/libraries/lumier/installation)
- [Docker](https://trycua.com/docs/libraries/lumier/docker)
- [Docker Compose](https://trycua.com/docs/libraries/lumier/docker-compose)
- [Building Lumier](https://trycua.com/docs/libraries/lumier/building-lumier)

## Credits

This project was inspired by [dockur/windows](https://github.com/dockur/windows) and [dockur/macos](https://github.com/dockur/macos), which pioneered the approach of running Windows and macOS VMs in Docker containers.

Main differences with dockur/macos:

- Lumier is specifically designed for macOS virtualization
- Lumier supports Apple Silicon (M1/M2/M3/M4) while dockur/macos only supports Intel
- Lumier uses the Apple Virtualization Framework (Vz) through the `lume` CLI to create true virtual machines, while dockur relies on KVM.
- Image specification is different, with Lumier and Lume relying on Apple Vz spec (disk.img and nvram.bin)
