# Day 2 of 5 Days of OSS Releases: QEMU Sandboxes

_Published on January 30, 2026 by the Cua Team_

Today we're releasing Windows 11 and Ubuntu VMs running in Docker via QEMU/KVM, available for self-hosting and MIT-licensed.

## Hardware-virtualized Desktops

Real operating systems booting from disk images. Windows 11 with full GUI, Ubuntu 22.04 with desktop environment. GPU passthrough supported for running local models on the sandbox or GUI apps that need graphics acceleration.

![qemu_1](https://github.com/user-attachments/assets/8282733d-c7e5-4f31-9974-32b414364868)

```bash
docker run -p 8006:8006 -p 5000:5000 ghcr.io/trycua/cua-qemu-windows:latest
```

## Fully Unattended Setup

Windows uses an unattended answer file, Linux uses cloud-init. Go from ISO to configured VM with user account, network, and computer server installed — no manual steps.

![qemu_2](https://github.com/user-attachments/assets/0c529e5e-a288-467e-a4e7-b065088af5b0)


## Computer Server Pre-installed

cua-computer-server runs on port 5000 at boot. Screenshots, mouse, keyboard, all over HTTP. Same API as Lume sandboxes — switch between macOS, Windows, and Linux without changing your agent code. Built-in autoupdater keeps the server current without rebuilding images.


![qemu_3](https://github.com/user-attachments/assets/edcb04c7-2f6d-4006-a593-0399f6eef761)

## noVNC on Port 8006

Browser-based desktop access. Open localhost:8006 to see the desktop, watch your agent run, debug visually. No VNC client install needed.

![qemu_4](https://github.com/user-attachments/assets/fa762996-1188-4119-9a79-0846f9e69bfb)


## Memory Snapshots

Freeze and restore full VM state via QEMU's snapshot support. Save state mid-task, restore to exact same point later — running processes, memory contents, everything.

![qemu_5](https://github.com/user-attachments/assets/8fd12831-c572-43e1-9455-349bb2fa879b)


## Runtime Config

Set RAM, CPU cores, disk size via environment variables. No need to rebuild images for different resource requirements.

![qemu_6](https://github.com/user-attachments/assets/89632439-b8e1-4b7b-80bb-c6453e281625)

```bash
docker run -e RAM_SIZE=16G -e CPU_CORES=8 -e DISK_SIZE=100G ghcr.io/trycua/cua-qemu-windows:latest
```

## Benchmark Compatible

Works with OSWorld and Windows Agent Arena. Pre-configured images include the applications used in standard agent evaluation suites.

![qemu_7](https://github.com/user-attachments/assets/a4374703-8516-4edc-88be-cbdb72919b88)

---

**Requires Docker and KVM support.**

- [GitHub Repository](https://github.com/trycua/cua)
- [Desktop Sandbox Documentation](https://cua.ai/docs/cua/guide/get-started/what-is-desktop-sandbox)
