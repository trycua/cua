"""Platform information commands.

Platforms are read-only built-in configurations for different environment types.
Each platform defines the Docker image, ports, and requirements for running
a specific type of sandbox (linux-docker, windows-qemu, etc.).

Usage:
    cua platform list                    # Show available platforms
    cua platform info <type>             # Show platform details
"""

import argparse
import json
import os
import platform as sys_platform
import subprocess
from typing import Any, Dict, Optional

from cua_cli.utils.output import print_error, print_info

# =============================================================================
# Platform Configurations
# =============================================================================

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


# =============================================================================
# Helper Functions
# =============================================================================


def check_docker() -> bool:
    """Check if Docker is running."""
    try:
        result = subprocess.run(["docker", "info"], capture_output=True, timeout=10)
        return result.returncode == 0
    except Exception:
        return False


def check_kvm() -> bool:
    """Check if KVM is available.

    On Linux: checks /dev/kvm directly
    On Windows/macOS with Docker: checks if KVM is available inside Docker's VM
    """
    if sys_platform.system() == "Linux":
        return os.path.exists("/dev/kvm")

    try:
        result = subprocess.run(
            ["docker", "run", "--rm", "--device=/dev/kvm", "alpine", "test", "-e", "/dev/kvm"],
            capture_output=True,
            timeout=30,
        )
        return result.returncode == 0
    except Exception:
        return False


def check_lume() -> bool:
    """Check if Lume is installed."""
    try:
        result = subprocess.run(["lume", "--version"], capture_output=True, timeout=10)
        return result.returncode == 0
    except Exception:
        return False


def check_image_exists(image_name: str) -> bool:
    """Check if a Docker image exists locally."""
    try:
        result = subprocess.run(
            ["docker", "images", "-q", image_name], capture_output=True, text=True, timeout=10
        )
        return bool(result.stdout.strip())
    except Exception:
        return False


def get_platform_config(platform_name: str) -> Optional[Dict[str, Any]]:
    """Get platform configuration by name."""
    return PLATFORMS.get(platform_name)


# =============================================================================
# Commands
# =============================================================================


def cmd_list(args: argparse.Namespace) -> int:
    """List all available platforms."""
    output_format = getattr(args, "format", "table")

    if output_format == "json":
        print(json.dumps(PLATFORMS, indent=2))
        return 0

    print("\nPlatforms")
    print("=" * 80)

    docker_ok = check_docker()
    kvm_ok = check_kvm()
    lume_ok = check_lume() if sys_platform.system() == "Darwin" else False
    is_macos = sys_platform.system() == "Darwin"
    is_linux = sys_platform.system() == "Linux"

    print("\nSystem:")
    print(f"  Docker:  {'✓ Running' if docker_ok else '✗ Not running'}")
    if is_linux:
        print(f"  KVM:     {'✓ Available' if kvm_ok else '○ Not available (QEMU will be slower)'}")
    if is_macos:
        print(f"  Lume:    {'✓ Installed' if lume_ok else '○ Not installed'}")

    print("\n" + "-" * 80)
    print(f"\n{'PLATFORM':<18} {'DESCRIPTION':<45} {'STATUS':<12}")
    print("-" * 80)

    for name, config in PLATFORMS.items():
        description = config.get("description", "")[:44]

        if name == "macos-lume":
            if not is_macos:
                status = "macOS only"
                status_color = "\033[90m"
            elif not lume_ok:
                status = "needs Lume"
                status_color = "\033[33m"
            else:
                status = "ready"
                status_color = "\033[92m"
        elif config.get("requires_kvm") and not kvm_ok:
            if is_linux:
                status = "no KVM"
                status_color = "\033[33m"
            else:
                status = "Linux only"
                status_color = "\033[90m"
        elif not docker_ok:
            status = "no Docker"
            status_color = "\033[91m"
        else:
            status = "ready"
            status_color = "\033[92m"

        reset = "\033[0m"
        print(f"{name:<18} {description:<45} {status_color}{status:<12}{reset}")

    print("\n" + "=" * 80)
    print("\nCommands:")
    print("  cua platform info <type>      # Show platform details")
    print("  cua image create <platform>   # Create image from platform")
    print()

    return 0


def cmd_info(args: argparse.Namespace) -> int:
    """Show detailed information about a platform."""
    name = args.platform
    config = get_platform_config(name)

    if not config:
        print_error(f"Unknown platform '{name}'")
        print_info(f"Available platforms: {', '.join(PLATFORMS.keys())}")
        return 1

    print(f"\nPlatform: {name}")
    print("=" * 60)

    print(f"\nDescription: {config.get('description', '-')}")
    print(f"OS Type:     {config.get('os_type', '-')}")

    if config.get("image"):
        print(f"Docker Image: {config['image']}")
        if check_docker():
            exists = check_image_exists(config["image"])
            print(f"Image Pulled: {'✓ Yes' if exists else '✗ No'}")

    print("\nPorts:")
    if config.get("internal_api_port"):
        print(f"  API Port (internal): {config['internal_api_port']}")
    if config.get("internal_vnc_port"):
        print(f"  VNC Port (internal): {config['internal_vnc_port']}")

    print("\nRequirements:")
    if config.get("requires_kvm"):
        kvm_ok = check_kvm()
        print(f"  KVM:    Required {'(✓ available)' if kvm_ok else '(✗ not available)'}")
    else:
        print("  KVM:    Not required")

    if config.get("requires_apple_silicon"):
        is_macos = sys_platform.system() == "Darwin"
        print(
            f"  Apple Silicon: Required {'(✓ running on macOS)' if is_macos else '(✗ not on macOS)'}"
        )

    if config.get("image_marker"):
        print(f"\nImage Marker: {config['image_marker']}")
        print("  (Marker file created in image directory when image is ready)")

    print("\nConfiguration:")
    print(f"  Boot Timeout: {config.get('boot_timeout', 60)}s")
    print(f"  Use Overlays: {'Yes' if config.get('use_overlays') else 'No'}")

    print("\n" + "=" * 60)
    print("\nTo create an image from this platform:")
    print(f"  cua image create {name}")
    print()

    return 0


# =============================================================================
# CLI Registration
# =============================================================================


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register the platform command with the main CLI parser."""
    platform_parser = subparsers.add_parser(
        "platform", help="Show available platform configurations"
    )
    platform_subparsers = platform_parser.add_subparsers(
        dest="platform_command", help="Platform command"
    )

    # platform list
    list_parser = platform_subparsers.add_parser("list", help="List all available platforms")
    list_parser.add_argument(
        "--format", choices=["table", "json"], default="table", help="Output format"
    )

    # platform info
    info_parser = platform_subparsers.add_parser("info", help="Show platform details")
    info_parser.add_argument("platform", help="Platform name (e.g., linux-docker, windows-qemu)")

    platform_parser.set_defaults(platform_command="list")


def execute(args: argparse.Namespace) -> int:
    """Execute the platform command."""
    cmd = getattr(args, "platform_command", "list")

    if cmd == "list":
        return cmd_list(args)
    elif cmd == "info":
        return cmd_info(args)
    else:
        return cmd_list(args)
