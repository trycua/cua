# Cua Windows Container

> [!WARNING]
> **Deprecated:** the `trycua/cua-qemu-windows` image is no longer maintained by Cua and will not receive rebuilds, updates or security fixes. Its build and publish workflows are switched off. The existing tags stay on Docker Hub so current pulls keep working, but do not start new work on it. Use the images in [`libs/images`](../images/README.md) (for example `ghcr.io/trycua/linux:24.04`) or a sandbox from the [`cua` SDK and CLI](https://cua.ai/docs/cua-sdk/quickstart). See [Migrate from deprecated packages](https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended#deprecated-packages).


Windows 11 virtual desktop container with QEMU/KVM for Computer-Using Agents.

**[Documentation](https://cua.ai/docs/cua/reference/desktop-sandbox/qemu-container/windows)** - Setup and configuration.

The VM runs `cua-spacesd` (gRPC + gRPC-Web on port 3211) from the
`Cua-Spacesd` logon task. Its token lives in `C:\ProgramData\cua\env-token`
inside the VM; place a file named `env-token` next to the setup scripts
(`/oem`) at image-build time to choose it. The Windows release artifact name is a
placeholder until the cua-spacesd release workflow publishes it.
