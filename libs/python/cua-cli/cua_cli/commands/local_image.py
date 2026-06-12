"""Local image management commands for CUA CLI.

Handles creating, cloning, and managing local VM/container images.
These are stored base images used for running sandboxes. For QEMU-based
platforms, images contain QCOW2 disk files. For Docker-based platforms,
images reference Docker container images.

Usage:
    cua image create <platform>          # Create image from platform
    cua image info <name>                # Show image details
    cua image clone <src> <dest>         # Clone an image
    cua image shell <name>               # Interactive shell (uses overlay by default)
    cua image shell <name> --writable    # Modify golden image directly (dangerous!)
"""

import argparse
import json
import shutil
import subprocess
import urllib.request
from datetime import datetime
from pathlib import Path
from typing import Optional

from cua_cli.commands.platform import (
    PLATFORMS,
    check_docker,
    check_image_exists,
    check_kvm,
    check_lume,
)
from cua_cli.utils.docker import allocate_ports, create_overlay_copy
from cua_cli.utils.output import (
    console,
    print_error,
    print_info,
    print_success,
    print_warning,
)
from cua_cli.utils.paths import get_data_dir
from cua_cli.utils.registry import (
    auto_discover_images,
    get_image_info,
    load_image_registry,
    register_image,
    unregister_image,
)

# =============================================================================
# Storage Paths
# =============================================================================


def get_images_base_path() -> Path:
    """Get the base path for all images."""
    return get_data_dir() / "images"


def get_image_path(name: str) -> Path:
    """Get the image path for a named image."""
    image_path = get_images_base_path() / name
    image_path.mkdir(parents=True, exist_ok=True)
    return image_path


def get_iso_path() -> Path:
    """Get the default Windows ISO path."""
    return get_data_dir() / "windows.iso"


# =============================================================================
# Helper Functions
# =============================================================================

WINDOWS_ISO_URL = "https://go.microsoft.com/fwlink/?linkid=2334167&clcid=0x409"


def pull_docker_image(image_name: str) -> None:
    """Pull Docker image from registry."""
    print_info(f"Pulling image: {image_name}")
    subprocess.run(["docker", "pull", image_name], check=True)


def download_windows_iso(dest_path: Path) -> bool:
    """Download Windows 11 ISO from Microsoft."""
    print_info("Downloading Windows 11 Enterprise Evaluation ISO...")
    print_info(f"  URL: {WINDOWS_ISO_URL}")
    print_info(f"  Destination: {dest_path}")
    print_warning("This is a large file (~6GB) and may take a while.")

    try:
        dest_path.parent.mkdir(parents=True, exist_ok=True)

        def report_progress(block_num, block_size, total_size):
            downloaded = block_num * block_size
            if total_size > 0:
                percent = min(100, downloaded * 100 / total_size)
                downloaded_mb = downloaded / (1024 * 1024)
                total_mb = total_size / (1024 * 1024)
                print(
                    f"\r  Progress: {percent:.1f}% ({downloaded_mb:.1f}/{total_mb:.1f} MB)",
                    end="",
                    flush=True,
                )
            else:
                downloaded_mb = downloaded / (1024 * 1024)
                print(f"\r  Downloaded: {downloaded_mb:.1f} MB", end="", flush=True)

        urllib.request.urlretrieve(WINDOWS_ISO_URL, dest_path, reporthook=report_progress)
        print()
        print_success("Download complete!")
        return True
    except Exception as e:
        print()
        print_error(f"Download failed: {e}")
        return False


def format_size(size_bytes: int) -> str:
    """Format size in human-readable format."""
    for unit in ["B", "KB", "MB", "GB", "TB"]:
        if size_bytes < 1024.0:
            return f"{size_bytes:.1f} {unit}"
        size_bytes /= 1024.0
    return f"{size_bytes:.1f} PB"


# =============================================================================
# Commands
# =============================================================================


def cmd_local_list(args: argparse.Namespace) -> int:
    """List all local images."""
    auto_discover_images()

    registry = load_image_registry()
    output_format = "json" if getattr(args, "json", False) else getattr(args, "format", "table")
    filter_platform = getattr(args, "platform", None)

    if output_format == "json":
        if filter_platform:
            filtered = {k: v for k, v in registry.items() if v.get("platform") == filter_platform}
            print(json.dumps(filtered, indent=2))
        else:
            print(json.dumps(registry, indent=2))
        return 0

    print("\nLocal Images")
    print("=" * 85)

    if not registry:
        print_info("No local images found.")
        print_info("Create one with:")
        print_info("  cua image create linux-docker")
        print_info("  cua image create windows-qemu --download-iso")
        return 0

    print(f"\n{'NAME':<20} {'PLATFORM':<15} {'SIZE':<10} {'CREATED':<12} {'STATUS':<10}")
    print("-" * 85)

    for name, info in sorted(registry.items()):
        platform = info.get("platform", "unknown")

        if filter_platform and platform != filter_platform:
            continue

        path = Path(info.get("path", ""))
        created_at = info.get("created_at", "")
        if created_at:
            try:
                dt = datetime.fromisoformat(created_at)
                created = dt.strftime("%Y-%m-%d")
            except Exception:
                created = created_at[:10] if len(created_at) >= 10 else created_at
        else:
            created = "-"

        if path.exists() and str(path) != "/dev/null":
            try:
                total_size = sum(f.stat().st_size for f in path.rglob("*") if f.is_file())
                size = format_size(total_size)
            except Exception:
                size = "-"
        else:
            size = "-"

        config = PLATFORMS.get(platform, {})
        marker = config.get("image_marker")
        if marker and path.exists():
            marker_path = path / marker
            status = "ready" if marker_path.exists() else "missing"
        elif platform == "linux-docker":
            docker_img = info.get("docker_image", "")
            status = "ready" if docker_img and check_image_exists(docker_img) else "missing"
        else:
            status = "ready" if path.exists() or str(path) == "/dev/null" else "missing"

        if info.get("parent"):
            status = f"ready (from {info['parent']})" if status == "ready" else status

        style = "green" if "ready" in status else "red"

        console.print(
            f"{name:<20} {platform:<15} {size:<10} {created:<12} [{style}]{status}[/{style}]"
        )

    print("\n" + "=" * 85)
    print("\nCommands:")
    print("  cua image info <name>                # Show detailed info")
    print("  cua image clone <source> <target>    # Clone an image")
    print("  cua image create <platform>          # Create new image")
    print()
    return 0


def cmd_info(args: argparse.Namespace) -> int:
    """Show detailed information about an image."""
    name = args.name
    info = get_image_info(name)

    if not info:
        print_error(f"Image '{name}' not found.")
        print_info("Available images:")
        for n in load_image_registry().keys():
            print_info(f"  - {n}")
        return 1

    platform = info.get("platform", "unknown")
    path = Path(info.get("path", ""))

    print(f"\nImage: {name}")
    print("=" * 60)

    print(f"\nPlatform:    {platform}")
    print(f"Path:        {path}")

    if path.exists() and str(path) != "/dev/null":
        try:
            total_size = sum(f.stat().st_size for f in path.rglob("*") if f.is_file())
            print(f"Size:        {format_size(total_size)}")
        except Exception:
            pass

    created_at = info.get("created_at", "")
    if created_at:
        try:
            dt = datetime.fromisoformat(created_at)
            print(f"Created:     {dt.strftime('%Y-%m-%d %H:%M:%S')}")
        except Exception:
            print(f"Created:     {created_at}")

    if info.get("parent"):
        print(f"Parent:      {info['parent']}")

    if info.get("docker_image"):
        print(f"Docker:      {info['docker_image']}")

    config = PLATFORMS.get(platform, {})
    marker = config.get("image_marker")
    if marker:
        marker_path = path / marker if path.exists() else None
        marker_ok = marker_path and marker_path.exists()
        print(f"Marker:      {marker} {'✓' if marker_ok else '✗'}")

    if info.get("description"):
        print(f"Description: {info['description']}")

    if info.get("apps_installed"):
        apps = ", ".join(info["apps_installed"])
        print(f"Apps:        {apps}")

    if info.get("tags"):
        tags = ", ".join(info["tags"])
        print(f"Tags:        {tags}")

    if info.get("config"):
        print("\nConfig:")
        for key, value in info["config"].items():
            print(f"  {key.capitalize():<10} {value}")

    print()
    return 0


def cmd_create(args: argparse.Namespace) -> int:
    """Create an image from a platform."""
    platform_name = args.platform
    config = PLATFORMS.get(platform_name)

    if not config:
        print_error(f"Unknown platform '{platform_name}'")
        print_info(f"Available platforms: {', '.join(PLATFORMS.keys())}")
        return 1

    if platform_name == "linux-docker":
        return _create_linux_docker(args, config)
    elif platform_name == "linux-qemu":
        return _create_linux_qemu(args, config)
    elif platform_name == "windows-qemu":
        return _create_windows_qemu(args, config)
    elif platform_name == "android-qemu":
        return _create_android_qemu(args, config)
    elif platform_name == "macos-lume":
        return _create_macos_lume(args, config)
    else:
        print_error(f"Image creation not implemented for platform: {platform_name}")
        return 1


def _create_linux_docker(args: argparse.Namespace, config: dict) -> int:
    """Create linux-docker image (pull container)."""
    if not check_docker():
        print_error("Docker is not running. Please start Docker and try again.")
        return 1

    image_name = getattr(args, "docker_image", None) or config["image"]
    name = getattr(args, "name", None) or "linux-docker"

    if getattr(args, "skip_pull", False):
        if not check_image_exists(image_name):
            print_error("--skip-pull is set but image not found locally.")
            return 1
        print_info(f"Skipping pull (using local image: {image_name})")
    else:
        print_info(f"Pulling latest Linux Docker image: {image_name}")
        try:
            pull_docker_image(image_name)
        except Exception as e:
            print_error(f"Failed to pull image: {e}")
            return 1

    register_image(
        name=name,
        platform="linux-docker",
        path=Path("/dev/null"),
        description="Linux GUI container",
        docker_image=image_name,
        config={"memory": "4G", "cpus": "2"},
        tags=["default", "webtop"],
    )

    print_success(f"Linux Docker image '{name}' ready!")
    print_info(f"Next: cua image shell {name}")
    return 0


def _create_linux_qemu(args: argparse.Namespace, config: dict) -> int:
    """Create linux-qemu image."""
    if not check_docker():
        print_error("Docker is not running. Please start Docker and try again.")
        return 1

    name = getattr(args, "name", None) or "linux-qemu"
    image_path = get_image_path(name)

    marker_path = image_path / config["image_marker"]
    if marker_path.exists() and not getattr(args, "force", False):
        print_info(f"Linux QEMU image '{name}' already exists at: {image_path}")
        print_info("Use --force to recreate it.")
        print_info(f"Next: cua image shell {name}")
        return 0

    if getattr(args, "iso", None):
        iso_path = Path(args.iso).resolve()
        if not iso_path.exists():
            print_error(f"ISO file not found: {iso_path}")
            return 1
    else:
        print_info("Linux QEMU Setup")
        print_info("[Coming soon] For now, please:")
        print_info("  1. Download an Ubuntu/Fedora ISO manually")
        print_info("  2. Run: cua image create linux-qemu --iso /path/to/linux.iso")
        return 1

    return 1


def _create_windows_qemu(args: argparse.Namespace, config: dict) -> int:
    """Create windows-qemu image."""
    import sys

    if not check_docker():
        print_error("Docker is not running. Please start Docker and try again.")
        return 1

    docker_image = getattr(args, "docker_image", None) or config["image"]
    name = getattr(args, "name", None) or "windows-qemu"
    image_path = get_image_path(name)

    marker_path = image_path / config["image_marker"]
    if marker_path.exists() and not getattr(args, "force", False):
        print_info(f"Windows QEMU image '{name}' already exists at: {image_path}")
        print_info("Use --force to recreate it.")
        print_info(f"Next: cua image shell {name}")
        return 0

    iso_path: Optional[Path] = None
    if getattr(args, "iso", None):
        iso_path = Path(args.iso).resolve()
        if not iso_path.exists():
            print_error(f"ISO file not found: {iso_path}")
            return 1
    else:
        default_iso = get_iso_path()
        if default_iso.exists():
            iso_path = default_iso
            print_info(f"Using existing ISO: {iso_path}")
        elif getattr(args, "download_iso", False):
            if not download_windows_iso(default_iso):
                return 1
            iso_path = default_iso
        else:
            print_info("Windows ISO Required")
            print_info("Options:")
            print_info("  1. Download automatically (~6GB):")
            print_info("     cua image create windows-qemu --download-iso")
            print_info("  2. Provide your own ISO:")
            print_info("     cua image create windows-qemu --iso /path/to/windows.iso")
            return 1

    if getattr(args, "skip_pull", False):
        if not check_image_exists(docker_image):
            print_error("--skip-pull is set but image not found locally.")
            return 1
        print_info(f"Skipping pull (using local image: {docker_image})")
    else:
        pull_docker_image(docker_image)

    print_info(f"Creating Windows QEMU Image: {name}")
    print_info(f"Image path: {image_path}")

    user_vnc = getattr(args, "vnc_port", None)
    user_api = getattr(args, "api_port", None)

    if user_vnc and user_api:
        vnc_port = int(user_vnc)
        api_port = int(user_api)
    else:
        vnc_port, api_port = allocate_ports(
            vnc_default=int(user_vnc) if user_vnc else 8006,
            api_default=int(user_api) if user_api else 5000,
        )
        if not user_vnc or not user_api:
            print_info(f"Auto-allocated ports: VNC={vnc_port}, API={api_port}")

    print_info(f"Monitor progress at: http://localhost:{vnc_port}")

    setup_container = "cua-setup-windows"
    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", f"name={setup_container}"], capture_output=True, text=True
    )
    if check_result.stdout.strip():
        print_error(f"Setup container '{setup_container}' is already running.")
        print_info(f"Use 'docker stop {setup_container}' to stop it first.")
        return 1

    detach_mode = getattr(args, "detach", False)

    docker_cmd = [
        "docker",
        "run",
        "-t",
        "--rm",
        "-p",
        f"{vnc_port}:8006",
        "-p",
        f"{api_port}:5000",
        "--name",
        setup_container,
        "--platform",
        "linux/amd64",
        "-v",
        f"{image_path}:/storage",
        "-v",
        f"{iso_path}:/custom.iso:ro",
        "--cap-add",
        "NET_ADMIN",
        "--stop-timeout",
        "120",
    ]

    if detach_mode:
        docker_cmd.insert(2, "-d")
    elif sys.stdin.isatty():
        docker_cmd.insert(2, "-i")

    if check_kvm() and not getattr(args, "no_kvm", False):
        docker_cmd.extend(["--device=/dev/kvm"])
    else:
        docker_cmd.extend(["-e", "KVM=N"])
        print_warning("Running without KVM (slower)")

    memory = getattr(args, "memory", "8G")
    disk = getattr(args, "disk", "64G")
    cpus = getattr(args, "cpus", "8")
    docker_cmd.extend(["-e", f"RAM_SIZE={memory}"])
    docker_cmd.extend(["-e", f"CPU_CORES={cpus}"])

    winarena_apps = getattr(args, "winarena_apps", False)
    if winarena_apps:
        print_info("Will install WinArena benchmark apps (Chrome, LibreOffice, VLC, etc.)")
        docker_cmd.extend(["-e", "INSTALL_WINARENA_APPS=true"])

        config_file = image_path / "install_config.json"
        config_data = {"INSTALL_WINARENA_APPS": True}
        config_file.write_text(json.dumps(config_data))
        docker_cmd.extend(["-v", f"{config_file}:/oem/install_config.json:ro"])
        print_info(f"  Config mounted: {config_file} -> /oem/install_config.json")
    else:
        docker_cmd.extend(["-e", "INSTALL_WINARENA_APPS=false"])

    docker_cmd.append(docker_image)

    try:
        if detach_mode:
            result = subprocess.run(docker_cmd, capture_output=True, text=True, check=False)
            if result.returncode != 0:
                print_error(f"Failed to start container: {result.stderr}")
                return 1

            container_id = result.stdout.strip()[:12]
            print_success(f"Setup started in background (ID: {container_id})")
            print_info(f"  Browser: http://localhost:{vnc_port}")
            print_info(f"  Logs:    docker logs -f {setup_container}")
            return 0
        else:
            result = subprocess.run(docker_cmd, check=False)

            if marker_path.exists():
                description = "Windows 11 VM"
                apps_installed = None
                if winarena_apps:
                    description += " with WinArena apps"
                    apps_installed = ["chrome", "libreoffice", "vlc", "vscode", "7zip"]

                register_image(
                    name=name,
                    platform="windows-qemu",
                    path=image_path,
                    description=description,
                    docker_image=docker_image,
                    config={"memory": memory, "cpus": cpus, "disk": disk},
                    tags=["winarena"] if winarena_apps else ["default"],
                    apps_installed=apps_installed,
                )

                print_success(f"Windows QEMU image '{name}' created!")
                print_info(f"  Path: {image_path}")
                print_info(f"Next: cua image shell {name}")
                return 0
            else:
                print_error("Image was not created.")
                return 1
    except KeyboardInterrupt:
        print_warning("Interrupted.")
        return 1


def _create_android_qemu(args: argparse.Namespace, config: dict) -> int:
    """Create android-qemu image."""
    name = getattr(args, "name", None) or "android-qemu"
    android_version = getattr(args, "version", "14")

    print_info("Android QEMU Setup")
    print_info(f"  Android version: {android_version}")
    print_info(f"  Image name: {name}")
    print_info("[Coming soon] This will be available in a future release.")
    return 1


def _create_macos_lume(args: argparse.Namespace, config: dict) -> int:
    """Create macos-lume image."""
    import platform as sys_platform

    if sys_platform.system() != "Darwin":
        print_error("macOS image creation can only run on macOS hosts (Apple Silicon required).")
        return 1

    if not check_lume():
        print_error("Lume is not installed.")
        print_info("Install Lume:")
        print_info(
            '  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"'
        )
        return 1

    name = getattr(args, "name", None) or "macos-lume"
    macos_version = getattr(args, "version", "sonoma")

    print_success("Lume is installed")
    print_info(f"To create a macOS VM ({macos_version}):")
    print_info(f"  lume create --name {name} --os {macos_version}")
    print_info("To start:")
    print_info(f"  cua image shell {name}")
    return 0


def cmd_local_delete(args: argparse.Namespace) -> int:
    """Delete a local image."""
    name = args.name
    force = getattr(args, "force", False)

    info = get_image_info(name)
    if not info:
        print_error(f"Image '{name}' not found.")
        return 1

    platform = info.get("platform", "unknown")
    path = Path(info.get("path", ""))

    print_info(f"Removing image: {name}")
    print_info(f"  Platform: {platform}")
    print_info(f"  Path: {path}")

    if not force:
        if platform != "linux-docker" and path.exists() and str(path) != "/dev/null":
            try:
                total_size = sum(f.stat().st_size for f in path.rglob("*") if f.is_file())
                print_info(f"  Size: {format_size(total_size)}")
            except Exception:
                pass

        print_warning("This will:")
        print_warning("  - Remove the registry entry")
        if platform != "linux-docker" and path.exists() and str(path) != "/dev/null":
            print_warning("  - Delete all files in the image directory")

        response = input("\nContinue? [y/N] ").strip().lower()
        if response != "y":
            print_info("Cancelled.")
            return 0

    unregister_image(name)
    print_success("Removed from registry")

    if platform != "linux-docker" and path.exists() and str(path) != "/dev/null":
        try:
            shutil.rmtree(path)
            print_success(f"Deleted files: {path}")
        except Exception as e:
            print_error(f"Failed to delete files: {e}")
            return 1

    print_success(f"Image '{name}' removed")
    return 0


def cmd_clone(args: argparse.Namespace) -> int:
    """Clone an image."""
    source = args.source
    target = args.target

    source_info = get_image_info(source)
    if not source_info:
        print_error(f"Source image '{source}' not found.")
        return 1

    if get_image_info(target):
        if not getattr(args, "force", False):
            print_error(f"Target image '{target}' already exists.")
            print_info("Use --force to overwrite.")
            return 1

    source_platform = source_info.get("platform", "unknown")
    source_path = Path(source_info.get("path", ""))

    if source_platform == "linux-docker":
        print_info(f"Cloning '{source}' to '{target}'...")
        register_image(
            name=target,
            platform=source_platform,
            path=source_path,
            description=f"Clone of {source}",
            docker_image=source_info.get("docker_image"),
            config=source_info.get("config"),
            parent=source,
            tags=(source_info.get("tags", []) or []) + ["cloned"],
        )
        print_success(f"Created '{target}' (reference to same container image)")
        return 0

    if not source_path.exists():
        print_error(f"Source path does not exist: {source_path}")
        return 1

    target_path = get_image_path(target)

    print_info(f"Cloning '{source}' to '{target}'...")
    print_info(f"  Source: {source_path}")
    print_info(f"  Target: {target_path}")

    try:
        total_size = sum(f.stat().st_size for f in source_path.rglob("*") if f.is_file())
        print_info(f"  Size:   {format_size(total_size)}")
    except Exception:
        pass

    print_info("Copying files (this may take a while for large images)...")

    try:
        if target_path.exists():
            shutil.rmtree(target_path)

        shutil.copytree(source_path, target_path)

        register_image(
            name=target,
            platform=source_platform,
            path=target_path,
            description=f"Clone of {source}",
            docker_image=source_info.get("docker_image"),
            config=source_info.get("config"),
            parent=source,
            tags=(source_info.get("tags", []) or []) + ["cloned"],
            apps_installed=source_info.get("apps_installed"),
        )

        print_success(f"Successfully cloned '{source}' to '{target}'")
        print_info("Next:")
        print_info(f"  cua image shell {target}              # Start shell (protected)")
        print_info(f"  cua image shell {target} --writable   # Start shell (modify image)")
        print_info(f"  cua image info {target}               # View details")
        return 0

    except Exception as e:
        print_error(f"Failed to clone: {e}")
        return 1


def cmd_shell(args: argparse.Namespace) -> int:
    """Start an interactive shell into an image.

    By default, uses an overlay to protect the golden image.
    With --writable, modifies the golden image directly (dangerous!).
    """
    name = args.name
    writable = getattr(args, "writable", False)

    info = get_image_info(name)
    if not info:
        print_error(f"Image '{name}' not found.")
        print_info("Available images:")
        for n in load_image_registry().keys():
            print_info(f"  - {n}")
        return 1

    platform = info.get("platform", "unknown")

    config = PLATFORMS.get(platform, {})
    if not config:
        print_error(f"Unknown platform '{platform}'")
        return 1

    if not check_docker():
        print_error("Docker is not running. Please start Docker and try again.")
        return 1

    if platform == "linux-docker":
        return _shell_linux_docker(args, info, name)

    return _shell_qemu(args, info, name, writable)


def _shell_linux_docker(args: argparse.Namespace, info: dict, name: str) -> int:
    """Start interactive shell for linux-docker image."""
    import sys

    docker_image = info.get("docker_image")
    if not docker_image:
        print_error(f"No Docker image configured for '{name}'")
        return 1

    user_vnc = getattr(args, "vnc_port", None)
    user_api = getattr(args, "api_port", None)

    if user_vnc and user_api:
        vnc_port = int(user_vnc)
        api_port = int(user_api)
    else:
        vnc_port, api_port = allocate_ports(
            vnc_default=int(user_vnc) if user_vnc else 6901,
            api_default=int(user_api) if user_api else 8000,
        )
        if not user_vnc or not user_api:
            print_info(f"Auto-allocated ports: VNC={vnc_port}, API={api_port}")

    detach_mode = getattr(args, "detach", False)

    container_name = f"cua-shell-{name}"

    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", f"name={container_name}"], capture_output=True, text=True
    )
    if check_result.stdout.strip():
        print_info(f"Shell for '{name}' is already running.")
        print_info(f"  VNC: http://localhost:{vnc_port}")
        print_info(f"  Stop: docker stop {container_name}")
        return 0

    print_info(f"Starting interactive shell for '{name}'...")

    docker_cmd = [
        "docker",
        "run",
        "-t",
        "--rm",
        "-p",
        f"{vnc_port}:6901",
        "-p",
        f"{api_port}:8000",
        "--name",
        container_name,
        docker_image,
    ]

    if detach_mode:
        docker_cmd.insert(2, "-d")
    elif sys.stdin.isatty():
        docker_cmd.insert(2, "-i")

    print_info(f"  VNC: http://localhost:{vnc_port}")

    if detach_mode:
        print_info(f"  Stop: docker stop {container_name}")
    else:
        print_info("Press Ctrl+C to stop.")

    try:
        if detach_mode:
            result = subprocess.run(docker_cmd, capture_output=True, text=True, check=False)
            if result.returncode != 0:
                print_error(f"Failed to start container: {result.stderr}")
                return 1
            container_id = result.stdout.strip()[:12]
            print_success(f"Shell started in background (ID: {container_id})")
            return 0
        else:
            subprocess.run(docker_cmd, check=False)
            return 0
    except KeyboardInterrupt:
        print_info("Stopped.")
        return 0


def _shell_qemu(args: argparse.Namespace, info: dict, name: str, writable: bool) -> int:
    """Start interactive shell for QEMU-based image."""
    import sys

    platform = info.get("platform")
    image_path = Path(info.get("path", ""))
    docker_image = info.get("docker_image") or PLATFORMS.get(platform, {}).get("image")

    if not image_path.exists():
        print_error(f"Image path does not exist: {image_path}")
        return 1

    config = PLATFORMS.get(platform, {})
    marker = config.get("image_marker")
    if marker:
        marker_path = image_path / marker
        if not marker_path.exists():
            print_error(f"Image '{name}' is not ready (marker missing: {marker})")
            print_info(f"Recreate with: cua image create {platform}")
            return 1

    user_vnc = getattr(args, "vnc_port", None)
    user_api = getattr(args, "api_port", None)

    if user_vnc and user_api:
        vnc_port = int(user_vnc)
        api_port = int(user_api)
    else:
        vnc_port, api_port = allocate_ports(
            vnc_default=int(user_vnc) if user_vnc else 8006,
            api_default=int(user_api) if user_api else 5000,
        )
        if not user_vnc or not user_api:
            print_info(f"Auto-allocated ports: VNC={vnc_port}, API={api_port}")

    memory = getattr(args, "memory", "8G")
    cpus = getattr(args, "cpus", "8")
    detach_mode = getattr(args, "detach", False)

    container_name = f"cua-shell-{name}"

    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", f"name={container_name}"], capture_output=True, text=True
    )
    if check_result.stdout.strip():
        print_info(f"Shell for '{name}' is already running.")
        print_info(f"  VNC:  http://localhost:{vnc_port}")
        print_info(f"  API:  http://localhost:{api_port}")
        print_info(f"  Stop: docker stop {container_name}")
        return 0

    volumes = []
    overlay_path = None

    if writable:
        print_warning("WARNING: WRITABLE MODE")
        print_warning("You are about to modify the golden image directly.")
        print_warning("Any changes will be PERMANENT and affect all future runs.")
        print_info(f"  Image: {name}")
        print_info(f"  Path:  {image_path}")

        if sys.stdin.isatty():
            response = input("\nAre you sure? Type 'yes' to continue: ").strip().lower()
            if response != "yes":
                print_info("Cancelled.")
                return 0
        else:
            print_error("Writable mode requires interactive terminal for confirmation.")
            print_info("Use --detach if you want to run without a TTY.")
            return 1

        volumes.append(f"{image_path}:/storage")
        print_info("Writable mode: Changes will persist to golden image")
    else:
        overlay_path = get_data_dir() / "overlays" / f"shell-{name}"

        print_info("Protected mode: Copying golden image to overlay...")
        try:
            create_overlay_copy(image_path, overlay_path, verbose=True)
        except Exception as e:
            print_error(f"Failed to create overlay: {e}")
            return 1

        volumes.append(f"{overlay_path}:/storage")
        print_success("Overlay ready (changes will be discarded on exit)")

    print_info(f"Starting {platform} shell for '{name}'...")

    docker_cmd = [
        "docker",
        "run",
        "-t",
        "--rm",
        "-p",
        f"{vnc_port}:8006",
        "-p",
        f"{api_port}:5000",
        "--name",
        container_name,
        "--platform",
        "linux/amd64",
        "--cap-add",
        "NET_ADMIN",
        "--stop-timeout",
        "120",
    ]

    if detach_mode:
        docker_cmd.insert(2, "-d")
    elif sys.stdin.isatty():
        docker_cmd.insert(2, "-i")

    for vol in volumes:
        docker_cmd.extend(["-v", vol])

    if check_kvm() and not getattr(args, "no_kvm", False):
        docker_cmd.extend(["--device=/dev/kvm"])
    else:
        docker_cmd.extend(["-e", "KVM=N"])
        print_warning("Running without KVM (slower)")

    docker_cmd.extend(["-e", f"RAM_SIZE={memory}"])
    docker_cmd.extend(["-e", f"CPU_CORES={cpus}"])
    docker_cmd.append(docker_image)

    print_info(f"  VNC:  http://localhost:{vnc_port}")
    print_info(f"  API:  http://localhost:{api_port}")

    if detach_mode:
        print_info(f"  Stop: docker stop {container_name}")
    else:
        print_info("Press Ctrl+C to stop.")

    try:
        if detach_mode:
            result = subprocess.run(docker_cmd, capture_output=True, text=True, check=False)
            if result.returncode != 0:
                print_error(f"Failed to start container: {result.stderr}")
                return 1
            container_id = result.stdout.strip()[:12]
            print_success(f"Shell started in background (ID: {container_id})")
            return 0
        else:
            subprocess.run(docker_cmd, check=False)
            return 0
    except KeyboardInterrupt:
        print_info("Stopped.")
        return 0
    finally:
        if not detach_mode and overlay_path and overlay_path.exists() and not writable:
            print_info(f"Cleaning up overlay: {overlay_path}")
            try:
                shutil.rmtree(overlay_path)
            except Exception as e:
                print_warning(f"Failed to clean up overlay: {e}")
