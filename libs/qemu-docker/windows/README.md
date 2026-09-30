# Cua Windows Container

Windows 11 virtual desktop container with QEMU/KVM for Computer-Using Agents.

**[Documentation](https://cua.ai/docs/cua/reference/desktop-sandbox/qemu-container/windows)** - Setup and configuration.

## First boot

The image does not include or download Windows installation media. For first
boot, mount a Windows ISO read-only at `/storage/custom.iso`:

```bash
docker run --device /dev/kvm --device /dev/net/tun --cap-add NET_ADMIN \
  -p 8006:8006 -p 5000:5000 \
  -v /path/to/windows.iso:/storage/custom.iso:ro \
  -v cua-win-storage:/storage \
  trycua/cua-qemu-windows:latest
```

For a multi-edition ISO that needs a specific Windows image selected during
setup, provide an answer file at `/storage/custom.xml` with an explicit
`InstallFrom` selection.
