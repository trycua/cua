# Cua Windows Container

Windows 11 virtual desktop container with QEMU/KVM for Computer-Using Agents.

**[Documentation](https://cua.ai/docs/cua/reference/desktop-sandbox/qemu-container/windows)** - Setup and configuration.

The VM runs `cua-spacesd` (gRPC + gRPC-Web on port 3211) from the
`Cua-Spacesd` logon task. Its token lives in `C:\ProgramData\cua\env-token`
inside the VM; place a file named `env-token` next to the setup scripts
(`/oem`) at image-build time to choose it. The Windows release artifact name is a
placeholder until the cua-spacesd release workflow publishes it.
