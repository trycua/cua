# Cua Linux Container

Ubuntu 22.04 virtual desktop container with QEMU/KVM for Computer-Using Agents.

**[Documentation](https://cua.ai/docs/cua/reference/desktop-sandbox/qemu-container/linux)** - Setup and configuration.

The VM runs `cua-spacesd` (gRPC + gRPC-Web on port 3211) as the
`cua-spacesd` systemd service. Its token lives in `/etc/cua/env-token` inside
the VM; put a file named `env-token` next to the setup scripts (`/oem`) at
image-build time to choose it.
