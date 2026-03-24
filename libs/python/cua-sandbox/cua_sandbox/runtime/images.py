"""Docker image tags, port mappings, and runtime constants."""

# ── Docker image tags ────────────────────────────────────────────────────────

UBUNTU_XFCE = "trycua/cua-xfce:latest"
QEMU_LINUX = "trycua/cua-qemu-linux:latest"
QEMU_WINDOWS = "trycua/cua-qemu-windows:latest"
QEMU_ANDROID = "trycua/cua-qemu-android:latest"
MACOS_SEQUOIA = "trycua/macos-sequoia:latest"

# ── Internal ports (inside the container) ────────────────────────────────────

XFCE_API_PORT = 8000
XFCE_VNC_PORT = 6901

QEMU_API_PORT = 5000
QEMU_VNC_PORT = 8006

ANDROID_API_PORT = 8000
ANDROID_VNC_PORT = 6080

LUME_API_PORT = 8443
LUME_PROVIDER_PORT = 7777

# ── Default host-side ports ──────────────────────────────────────────────────

DEFAULT_API_PORT = 8000
DEFAULT_VNC_PORT = 6901


def resolve_image(os_type: str, registry: str | None = None) -> str:
    """Map an os_type to a default Docker image tag."""
    if registry:
        return registry
    return {
        "linux": UBUNTU_XFCE,
        "windows": QEMU_WINDOWS,
        "macos": MACOS_SEQUOIA,
        "android": QEMU_ANDROID,
    }.get(os_type, UBUNTU_XFCE)


def internal_ports(docker_image: str) -> tuple[int, int]:
    """Return (api_port, vnc_port) for the given Docker image."""
    img = docker_image.lower()
    if "qemu-android" in img:
        return ANDROID_API_PORT, ANDROID_VNC_PORT
    if "qemu" in img:
        return QEMU_API_PORT, QEMU_VNC_PORT
    return XFCE_API_PORT, XFCE_VNC_PORT
