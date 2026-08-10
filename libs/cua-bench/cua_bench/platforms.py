"""Built-in platform definitions shared by CUA-Bench runners and CLI commands."""

from typing import Any, Dict, Optional


PLATFORMS: Dict[str, Dict[str, Any]] = {
    "linux-docker": {
        "image": "trycua/cua-xfce:latest",
        "description": "Linux GUI container (no KVM required)",
        "internal_vnc_port": 6901,
        "internal_api_port": 8000,
        "requires_kvm": False,
        "image_marker": None,
        "os_type": "linux",
        "boot_timeout": 60,
        "use_overlays": False,
    },
    "linux-qemu": {
        "image": "trycua/cua-qemu-linux:latest",
        "description": "Linux VM with QEMU/KVM (OSWorld)",
        "internal_vnc_port": 8006,
        "internal_api_port": 5000,
        "requires_kvm": True,
        "image_marker": "linux.boot",
        "os_type": "linux",
        "boot_timeout": 120,
        "use_overlays": True,
    },
    "windows-qemu": {
        "image": "trycua/cua-qemu-windows:latest",
        "description": "Windows VM with QEMU/KVM (Windows Arena)",
        "internal_vnc_port": 8006,
        "internal_api_port": 5000,
        "requires_kvm": True,
        "image_marker": "windows.boot",
        "os_type": "windows",
        "boot_timeout": 180,
        "use_overlays": True,
    },
    "android-qemu": {
        "image": "trycua/cua-qemu-android:latest",
        "description": "Android VM with QEMU/KVM",
        "internal_vnc_port": 8006,
        "internal_api_port": 5000,
        "requires_kvm": True,
        "image_marker": "android.boot",
        "os_type": "android",
        "boot_timeout": 120,
        "use_overlays": True,
    },
    "macos-lume": {
        "image": None,
        "description": "macOS VM with Apple Virtualization (Lume, Apple Silicon only)",
        "internal_vnc_port": None,
        "internal_api_port": 5000,
        "requires_kvm": False,
        "image_marker": None,
        "os_type": "macos",
        "boot_timeout": 120,
        "use_overlays": False,
        "requires_apple_silicon": True,
    },
}

CONTAINER_PLATFORMS = (
    "linux-docker",
    "linux-qemu",
    "windows-qemu",
    "android-qemu",
)


def get_platform_config(platform_name: str) -> Optional[Dict[str, Any]]:
    """Get a built-in platform configuration by name."""
    return PLATFORMS.get(platform_name)


def resolve_container_image(platform_name: str, image_name: Optional[str] = None) -> str:
    """Resolve the container image used to host a built-in platform.

    For Docker-native platforms, an explicit image URI replaces the built-in
    default. For QEMU platforms, ``image_name`` identifies the stored guest
    disk while the platform's container image remains the QEMU host.
    """
    if platform_name not in CONTAINER_PLATFORMS:
        raise ValueError(
            f"Unknown container platform: {platform_name}. "
            f"Valid platforms: {list(CONTAINER_PLATFORMS)}"
        )

    config = PLATFORMS[platform_name]
    default_image = config["image"]
    if not isinstance(default_image, str):
        raise ValueError(f"Platform {platform_name!r} has no container image")

    if not config["use_overlays"] and image_name and image_name != platform_name:
        return image_name
    return default_image
