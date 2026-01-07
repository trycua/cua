"""Image management commands.

Images are stored base images used for running sandboxes. They are created
from platforms and stored locally. For QEMU-based platforms, images contain
QCOW2 disk files. For Docker-based platforms, images reference Docker images.

Usage:
    cb image list                       # List all images
    cb image create <platform>          # Create image from platform
    cb image info <name>                # Show image details
    cb image delete <name>              # Remove an image
    cb image clone <src> <dest>         # Clone an image
    cb image shell <name>               # Interactive shell (uses overlay by default)
    cb image shell <name> --writable    # Modify golden image directly (dangerous!)
"""

import json
import os
import shutil
import subprocess
import urllib.request
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional

from .platform import PLATFORMS, check_docker, check_kvm, check_lume, check_image_exists
from ...runner.docker_utils import create_overlay_copy, allocate_ports


# =============================================================================
# XDG Base Directory Support
# =============================================================================

def get_xdg_data_home() -> Path:
    """Get XDG_DATA_HOME, defaulting to ~/.local/share per spec."""
    xdg_data = os.environ.get("XDG_DATA_HOME")
    if xdg_data:
        return Path(xdg_data)
    return Path.home() / ".local" / "share"


def get_xdg_state_home() -> Path:
    """Get XDG_STATE_HOME, defaulting to ~/.local/state per spec."""
    xdg_state = os.environ.get("XDG_STATE_HOME")
    if xdg_state:
        return Path(xdg_state)
    return Path.home() / ".local" / "state"


def get_data_dir() -> Path:
    """Get the cua-bench data directory (for images)."""
    return get_xdg_data_home() / "cua-bench"


def get_state_dir() -> Path:
    """Get the cua-bench state directory (for registries)."""
    return get_xdg_state_home() / "cua-bench"


# =============================================================================
# Image Registry
# =============================================================================

def get_image_registry_path() -> Path:
    """Get the image registry file path."""
    return get_state_dir() / "images.json"


def load_image_registry() -> Dict[str, Any]:
    """Load the image registry."""
    registry_path = get_image_registry_path()
    if registry_path.exists():
        try:
            data = json.loads(registry_path.read_text())
            return data.get("images", data)
        except Exception:
            return {}
    return {}


def save_image_registry(registry: Dict[str, Any]):
    """Save the image registry."""
    registry_path = get_image_registry_path()
    registry_path.parent.mkdir(parents=True, exist_ok=True)
    data = {"version": 1, "images": registry}
    registry_path.write_text(json.dumps(data, indent=2))


def register_image(
    name: str,
    platform: str,
    path: Path,
    description: str = "",
    docker_image: Optional[str] = None,
    config: Optional[Dict] = None,
    parent: Optional[str] = None,
    tags: Optional[List[str]] = None,
    apps_installed: Optional[List[str]] = None,
):
    """Register an image in the registry."""
    registry = load_image_registry()

    entry = {
        "platform": platform,
        "path": str(path),
        "description": description,
        "created_at": datetime.now().isoformat(),
    }

    if docker_image:
        entry["docker_image"] = docker_image
    if config:
        entry["config"] = config
    if parent:
        entry["parent"] = parent
    if tags:
        entry["tags"] = tags
    if apps_installed:
        entry["apps_installed"] = apps_installed

    platform_config = PLATFORMS.get(platform, {})
    if platform_config.get("image_marker"):
        entry["marker_file"] = platform_config["image_marker"]

    registry[name] = entry
    save_image_registry(registry)


def unregister_image(name: str):
    """Remove an image from the registry."""
    registry = load_image_registry()
    if name in registry:
        del registry[name]
        save_image_registry(registry)


def get_image_info(name: str) -> Optional[Dict]:
    """Get image info by name."""
    registry = load_image_registry()
    return registry.get(name)


def list_images() -> Dict[str, Any]:
    """List all registered images."""
    return load_image_registry()


def auto_discover_images():
    """Scan images directory and register any unregistered images.

    This handles the case where images were created with --detach mode
    and never got registered (because the process returned early).
    """
    from datetime import datetime

    images_dir = get_images_base_path()
    if not images_dir.exists():
        return

    registry = load_image_registry()
    modified = False

    # Check each subdirectory in the images folder
    for image_dir in images_dir.iterdir():
        if not image_dir.is_dir():
            continue

        name = image_dir.name

        # Skip if already registered
        if name in registry:
            continue

        # Check for platform-specific marker files
        for platform_name, config in PLATFORMS.items():
            marker = config.get("image_marker")
            if not marker:
                continue

            marker_path = image_dir / marker
            if marker_path.exists():
                # Found an unregistered image with a valid marker
                print(f"Auto-registering discovered image: {name} ({platform_name})")

                entry = {
                    "platform": platform_name,
                    "path": str(image_dir),
                    "description": f"Auto-discovered {platform_name} image",
                    "created_at": datetime.now().isoformat(),
                    "docker_image": config.get("image"),
                    "config": {"memory": "8G", "cpus": "8"},
                    "tags": ["auto-discovered"],
                    "marker_file": marker,
                }

                registry[name] = entry
                modified = True
                break

    if modified:
        save_image_registry(registry)


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


def pull_image(image_name: str):
    """Pull Docker image from registry."""
    print(f"Pulling image: {image_name}")
    subprocess.run(["docker", "pull", image_name], check=True)


def download_windows_iso(dest_path: Path) -> bool:
    """Download Windows 11 ISO from Microsoft."""
    print(f"\nDownloading Windows 11 Enterprise Evaluation ISO...")
    print(f"  URL: {WINDOWS_ISO_URL}")
    print(f"  Destination: {dest_path}")
    print("\n  This is a large file (~6GB) and may take a while.\n")

    try:
        dest_path.parent.mkdir(parents=True, exist_ok=True)

        def report_progress(block_num, block_size, total_size):
            downloaded = block_num * block_size
            if total_size > 0:
                percent = min(100, downloaded * 100 / total_size)
                downloaded_mb = downloaded / (1024 * 1024)
                total_mb = total_size / (1024 * 1024)
                print(f"\r  Progress: {percent:.1f}% ({downloaded_mb:.1f}/{total_mb:.1f} MB)", end="", flush=True)
            else:
                downloaded_mb = downloaded / (1024 * 1024)
                print(f"\r  Downloaded: {downloaded_mb:.1f} MB", end="", flush=True)

        urllib.request.urlretrieve(WINDOWS_ISO_URL, dest_path, reporthook=report_progress)
        print("\n\n✓ Download complete!")
        return True
    except Exception as e:
        print(f"\n\n✗ Download failed: {e}")
        return False


def format_size(size_bytes: int) -> str:
    """Format size in human-readable format."""
    for unit in ['B', 'KB', 'MB', 'GB', 'TB']:
        if size_bytes < 1024.0:
            return f"{size_bytes:.1f} {unit}"
        size_bytes /= 1024.0
    return f"{size_bytes:.1f} PB"


# =============================================================================
# Commands
# =============================================================================

def cmd_list(args) -> int:
    """List all images."""
    # Auto-discover any unregistered images (e.g., from --detach mode)
    auto_discover_images()

    registry = load_image_registry()
    output_format = getattr(args, 'format', 'table')
    filter_platform = getattr(args, 'platform', None)

    if output_format == 'json':
        if filter_platform:
            filtered = {k: v for k, v in registry.items() if v.get('platform') == filter_platform}
            print(json.dumps(filtered, indent=2))
        else:
            print(json.dumps(registry, indent=2))
        return 0

    print("\nImages")
    print("=" * 85)

    if not registry:
        print("\nNo images found.")
        print("\nCreate one with:")
        print("  cb image create linux-docker")
        print("  cb image create windows-qemu --download-iso")
        return 0

    print(f"\n{'NAME':<20} {'PLATFORM':<15} {'SIZE':<10} {'CREATED':<12} {'STATUS':<10}")
    print("-" * 85)

    for name, info in sorted(registry.items()):
        platform = info.get("platform", "unknown")

        # Apply filter
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

        # Calculate size
        if path.exists() and str(path) != "/dev/null":
            try:
                total_size = sum(f.stat().st_size for f in path.rglob('*') if f.is_file())
                size = format_size(total_size)
            except Exception:
                size = "-"
        else:
            size = "-"

        # Check status
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

        # Check if cloned
        if info.get("parent"):
            status = f"ready (from {info['parent']})" if status == "ready" else status

        status_color = "\033[92m" if "ready" in status else "\033[91m"
        reset = "\033[0m"

        print(f"{name:<20} {platform:<15} {size:<10} {created:<12} {status_color}{status}{reset}")

    print("\n" + "=" * 85)
    print("\nCommands:")
    print("  cb image info <name>                # Show detailed info")
    print("  cb image clone <source> <target>    # Clone an image")
    print("  cb image create <platform>          # Create new image")
    print()
    return 0


def cmd_info(args) -> int:
    """Show detailed information about an image."""
    name = args.name
    info = get_image_info(name)

    if not info:
        print(f"Error: Image '{name}' not found.")
        print("\nAvailable images:")
        for n in load_image_registry().keys():
            print(f"  - {n}")
        return 1

    platform = info.get("platform", "unknown")
    path = Path(info.get("path", ""))

    print(f"\nImage: {name}")
    print("=" * 60)

    print(f"\nPlatform:    {platform}")
    print(f"Path:        {path}")

    # Size
    if path.exists() and str(path) != "/dev/null":
        try:
            total_size = sum(f.stat().st_size for f in path.rglob('*') if f.is_file())
            print(f"Size:        {format_size(total_size)}")
        except Exception:
            pass

    # Created
    created_at = info.get("created_at", "")
    if created_at:
        try:
            dt = datetime.fromisoformat(created_at)
            print(f"Created:     {dt.strftime('%Y-%m-%d %H:%M:%S')}")
        except Exception:
            print(f"Created:     {created_at}")

    # Parent
    if info.get("parent"):
        print(f"Parent:      {info['parent']}")

    # Docker image
    if info.get("docker_image"):
        print(f"Docker:      {info['docker_image']}")

    # Status
    config = PLATFORMS.get(platform, {})
    marker = config.get("image_marker")
    if marker:
        marker_path = path / marker if path.exists() else None
        marker_ok = marker_path and marker_path.exists()
        print(f"Marker:      {marker} {'✓' if marker_ok else '✗'}")

    # Description
    if info.get("description"):
        print(f"Description: {info['description']}")

    # Apps installed
    if info.get("apps_installed"):
        apps = ", ".join(info["apps_installed"])
        print(f"Apps:        {apps}")

    # Tags
    if info.get("tags"):
        tags = ", ".join(info["tags"])
        print(f"Tags:        {tags}")

    # Config
    if info.get("config"):
        print(f"\nConfig:")
        for key, value in info["config"].items():
            print(f"  {key.capitalize():<10} {value}")

    print()
    return 0


def cmd_create(args) -> int:
    """Create an image from a platform."""
    platform_name = args.platform
    config = PLATFORMS.get(platform_name)

    if not config:
        print(f"Error: Unknown platform '{platform_name}'")
        print(f"\nAvailable platforms: {', '.join(PLATFORMS.keys())}")
        return 1

    # Dispatch to appropriate creation function
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
        print(f"Error: Image creation not implemented for platform: {platform_name}")
        return 1


def _create_linux_docker(args, config) -> int:
    """Create linux-docker image (pull container)."""
    if not check_docker():
        print("Error: Docker is not running. Please start Docker and try again.")
        return 1

    image_name = getattr(args, 'docker_image', None) or config["image"]
    name = getattr(args, 'name', None) or "linux-docker"

    # Always pull latest image unless --skip-pull is set
    if getattr(args, 'skip_pull', False):
        if not check_image_exists(image_name):
            print("Error: --skip-pull is set but image not found locally.")
            return 1
        print(f"Skipping pull (using local image: {image_name})")
    else:
        print(f"Pulling latest Linux Docker image: {image_name}")
        try:
            pull_image(image_name)
        except Exception as e:
            print(f"\n✗ Failed to pull image: {e}")
            return 1

    # Register the image
    register_image(
        name=name,
        platform="linux-docker",
        path=Path("/dev/null"),
        description="Linux GUI container",
        docker_image=image_name,
        config={"memory": "4G", "cpus": "2"},
        tags=["default", "webtop"],
    )

    print(f"\n✓ Linux Docker image ready!")
    print("\nNext steps:")
    print(f"  cb image shell {name}")
    return 0


def _create_linux_qemu(args, config) -> int:
    """Create linux-qemu image."""
    if not check_docker():
        print("Error: Docker is not running. Please start Docker and try again.")
        return 1

    name = getattr(args, 'name', None) or "linux-qemu"
    image_path = get_image_path(name)
    distro = getattr(args, 'distro', 'ubuntu')

    marker_path = image_path / config["image_marker"]
    if marker_path.exists() and not getattr(args, 'force', False):
        print(f"✓ Linux QEMU image '{name}' already exists at: {image_path}")
        print("\nUse --force to recreate it.")
        print("\nNext steps:")
        print(f"  cb image shell {name}")
        return 0

    if getattr(args, 'iso', None):
        iso_path = Path(args.iso).resolve()
        if not iso_path.exists():
            print(f"Error: ISO file not found: {iso_path}")
            return 1
    else:
        print(f"\n{'=' * 60}")
        print("Linux QEMU Setup")
        print('=' * 60)
        print(f"\nDistro: {distro}")
        print("\n[Coming soon] For now, please:")
        print("  1. Download an Ubuntu/Fedora ISO manually")
        print("  2. Run: cb image create linux-qemu --iso /path/to/linux.iso")
        return 1

    return 1


def _create_windows_qemu(args, config) -> int:
    """Create windows-qemu image."""
    import sys

    if not check_docker():
        print("Error: Docker is not running. Please start Docker and try again.")
        return 1

    docker_image = getattr(args, 'docker_image', None) or config["image"]
    name = getattr(args, 'name', None) or "windows-qemu"
    image_path = get_image_path(name)

    marker_path = image_path / config["image_marker"]
    if marker_path.exists() and not getattr(args, 'force', False):
        print(f"✓ Windows QEMU image '{name}' already exists at: {image_path}")
        print("\nUse --force to recreate it.")
        print("\nNext steps:")
        print(f"  cb image shell {name}")
        return 0

    iso_path = None
    if getattr(args, 'iso', None):
        iso_path = Path(args.iso).resolve()
        if not iso_path.exists():
            print(f"Error: ISO file not found: {iso_path}")
            return 1
    else:
        default_iso = get_iso_path()
        if default_iso.exists():
            iso_path = default_iso
            print(f"Using existing ISO: {iso_path}")
        elif getattr(args, 'download_iso', False):
            if not download_windows_iso(default_iso):
                return 1
            iso_path = default_iso
        else:
            print("\n" + "=" * 60)
            print("Windows ISO Required")
            print("=" * 60)
            print("\nOptions:")
            print("  1. Download automatically (~6GB):")
            print("     cb image create windows-qemu --download-iso")
            print("\n  2. Provide your own ISO:")
            print("     cb image create windows-qemu --iso /path/to/windows.iso")
            return 1

    # Always pull latest image unless --skip-pull is set
    if getattr(args, 'skip_pull', False):
        if not check_image_exists(docker_image):
            print("Error: --skip-pull is set but image not found locally.")
            return 1
        print(f"Skipping pull (using local image: {docker_image})")
    else:
        pull_image(docker_image)

    print(f"\n{'=' * 60}")
    print(f"Creating Windows QEMU Image: {name}")
    print('=' * 60)
    print(f"\nImage path: {image_path}")

    # Get user-specified ports or auto-allocate
    user_vnc = getattr(args, 'vnc_port', None)
    user_api = getattr(args, 'api_port', None)

    if user_vnc and user_api:
        vnc_port = int(user_vnc)
        api_port = int(user_api)
    else:
        vnc_port, api_port = allocate_ports(
            vnc_default=int(user_vnc) if user_vnc else 8006,
            api_default=int(user_api) if user_api else 5000
        )
        if not user_vnc or not user_api:
            print(f"Auto-allocated ports: VNC={vnc_port}, API={api_port}")

    print(f"\nMonitor progress at: http://localhost:{vnc_port}")

    setup_container = "cua-setup-windows"
    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", f"name={setup_container}"],
        capture_output=True, text=True
    )
    if check_result.stdout.strip():
        print(f"Setup container '{setup_container}' is already running.")
        print(f"Use 'docker stop {setup_container}' to stop it first.")
        return 1

    detach_mode = getattr(args, 'detach', False)

    docker_cmd = [
        "docker", "run", "-t", "--rm",
        "-p", f"{vnc_port}:8006",
        "-p", f"{api_port}:5000",
        "--name", setup_container,
        "--platform", "linux/amd64",
        "-v", f"{image_path}:/storage",
        "-v", f"{iso_path}:/custom.iso:ro",
        "--cap-add", "NET_ADMIN",
        "--stop-timeout", "120",
    ]

    if detach_mode:
        docker_cmd.insert(2, "-d")
    elif sys.stdin.isatty():
        docker_cmd.insert(2, "-i")

    if check_kvm() and not getattr(args, 'no_kvm', False):
        docker_cmd.extend(["--device=/dev/kvm"])
    else:
        docker_cmd.extend(["-e", "KVM=N"])
        print("Note: Running without KVM (slower)")

    memory = getattr(args, 'memory', '8G')
    disk = getattr(args, 'disk', '64G')
    cpus = getattr(args, 'cpus', '8')
    docker_cmd.extend(["-e", f"RAM_SIZE={memory}"])
    docker_cmd.extend(["-e", f"CPU_CORES={cpus}"])

    winarena_apps = getattr(args, 'winarena_apps', False)
    if winarena_apps:
        print("Will install WinArena benchmark apps (Chrome, LibreOffice, VLC, etc.)")
        docker_cmd.extend(["-e", "INSTALL_WINARENA_APPS=true"])

        # Create install_config.json and mount it into /oem/
        # This file is read by setup.ps1 inside Windows VM
        import json
        config_file = image_path / "install_config.json"
        config_data = {"INSTALL_WINARENA_APPS": True}
        config_file.write_text(json.dumps(config_data))
        docker_cmd.extend(["-v", f"{config_file}:/oem/install_config.json:ro"])
        print(f"  Config mounted: {config_file} -> /oem/install_config.json")
    else:
        docker_cmd.extend(["-e", "INSTALL_WINARENA_APPS=false"])

    docker_cmd.append(docker_image)

    try:
        if detach_mode:
            result = subprocess.run(docker_cmd, capture_output=True, text=True, check=False)
            if result.returncode != 0:
                print(f"\n✗ Failed to start container: {result.stderr}")
                return 1

            container_id = result.stdout.strip()[:12]
            print(f"\n✓ Setup started in background (ID: {container_id})")
            print(f"\nMonitor progress:")
            print(f"  Browser: http://localhost:{vnc_port}")
            print(f"  Logs:    docker logs -f {setup_container}")
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

                print("\n" + "=" * 60)
                print(f"✓ Windows QEMU image '{name}' created!")
                print("=" * 60)
                print(f"\n  Path: {image_path}")
                print("\nNext steps:")
                print(f"  cb image shell {name}")
                return 0
            else:
                print(f"\n✗ Image was not created.")
                return 1
    except KeyboardInterrupt:
        print("\n\nInterrupted.")
        return 1


def _create_android_qemu(args, config) -> int:
    """Create android-qemu image."""
    name = getattr(args, 'name', None) or "android-qemu"
    android_version = getattr(args, 'version', '14')

    print(f"\n{'=' * 60}")
    print("Android QEMU Setup")
    print('=' * 60)
    print(f"\nAndroid version: {android_version}")
    print(f"Image name: {name}")
    print("\n[Coming soon] This will be available in a future release.")
    return 1


def _create_macos_lume(args, config) -> int:
    """Create macos-lume image."""
    import platform as sys_platform

    if sys_platform.system() != "Darwin":
        print("Error: macOS image creation can only run on macOS hosts (Apple Silicon required).")
        return 1

    if not check_lume():
        print("Error: Lume is not installed.")
        print("\nInstall Lume:")
        print('  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/lume/scripts/install.sh)"')
        return 1

    name = getattr(args, 'name', None) or "macos-lume"
    macos_version = getattr(args, 'version', 'sonoma')

    print("✓ Lume is installed")
    print(f"\nTo create a macOS VM ({macos_version}):")
    print(f"  lume create --name {name} --os {macos_version}")
    print("\nTo start:")
    print(f"  cb image shell {name}")
    return 0


def cmd_delete(args) -> int:
    """Delete an image."""
    name = args.name
    force = getattr(args, 'force', False)

    info = get_image_info(name)
    if not info:
        print(f"Error: Image '{name}' not found.")
        return 1

    platform = info.get("platform", "unknown")
    path = Path(info.get("path", ""))

    print(f"\nRemoving image: {name}")
    print(f"  Platform: {platform}")
    print(f"  Path: {path}")

    # Confirm if not forced
    if not force:
        if platform != "linux-docker" and path.exists() and str(path) != "/dev/null":
            try:
                total_size = sum(f.stat().st_size for f in path.rglob('*') if f.is_file())
                print(f"  Size: {format_size(total_size)}")
            except Exception:
                pass

        print("\nThis will:")
        print("  - Remove the registry entry")
        if platform != "linux-docker" and path.exists() and str(path) != "/dev/null":
            print("  - Delete all files in the image directory")

        response = input("\nContinue? [y/N] ").strip().lower()
        if response != 'y':
            print("Cancelled.")
            return 0

    # Remove from registry
    unregister_image(name)
    print("✓ Removed from registry")

    # Remove files (not for container-based images)
    if platform != "linux-docker" and path.exists() and str(path) != "/dev/null":
        try:
            shutil.rmtree(path)
            print(f"✓ Deleted files: {path}")
        except Exception as e:
            print(f"✗ Failed to delete files: {e}")
            return 1

    print(f"\n✓ Image '{name}' removed")
    return 0


def cmd_clone(args) -> int:
    """Clone an image."""
    source = args.source
    target = args.target

    source_info = get_image_info(source)
    if not source_info:
        print(f"Error: Source image '{source}' not found.")
        return 1

    # Check if target already exists
    if get_image_info(target):
        if not getattr(args, 'force', False):
            print(f"Error: Target image '{target}' already exists.")
            print("Use --force to overwrite.")
            return 1

    source_platform = source_info.get("platform", "unknown")
    source_path = Path(source_info.get("path", ""))

    # For container-based images, just copy the registry entry
    if source_platform == "linux-docker":
        print(f"Cloning '{source}' to '{target}'...")
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
        print(f"\n✓ Created '{target}' (reference to same container image)")
        return 0

    # For disk-based images, actually copy the files
    if not source_path.exists():
        print(f"Error: Source path does not exist: {source_path}")
        return 1

    target_path = get_image_path(target)

    print(f"Cloning '{source}' to '{target}'...")
    print(f"  Source: {source_path}")
    print(f"  Target: {target_path}")

    # Calculate size for progress
    try:
        total_size = sum(f.stat().st_size for f in source_path.rglob('*') if f.is_file())
        print(f"  Size:   {format_size(total_size)}")
    except Exception:
        pass

    print("\nCopying files (this may take a while for large images)...")

    try:
        # Remove target if exists (with --force)
        if target_path.exists():
            shutil.rmtree(target_path)

        # Copy the directory
        shutil.copytree(source_path, target_path)

        # Register the new image
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

        print(f"\n✓ Successfully cloned '{source}' to '{target}'")
        print(f"\nNext steps:")
        print(f"  cb image shell {target}              # Start shell (protected)")
        print(f"  cb image shell {target} --writable   # Start shell (modify image)")
        print(f"  cb image info {target}               # View details")
        return 0

    except Exception as e:
        print(f"\n✗ Failed to clone: {e}")
        return 1


def cmd_shell(args) -> int:
    """Start an interactive shell into an image.

    By default, uses an overlay to protect the golden image.
    With --writable, modifies the golden image directly (dangerous!).
    """
    name = args.name
    writable = getattr(args, 'writable', False)

    info = get_image_info(name)
    if not info:
        print(f"Error: Image '{name}' not found.")
        print("\nAvailable images:")
        for n in load_image_registry().keys():
            print(f"  - {n}")
        return 1

    platform = info.get("platform", "unknown")
    image_path = Path(info.get("path", ""))
    docker_image = info.get("docker_image")

    # Get platform config
    config = PLATFORMS.get(platform, {})
    if not config:
        print(f"Error: Unknown platform '{platform}'")
        return 1

    if not check_docker():
        print("Error: Docker is not running. Please start Docker and try again.")
        return 1

    # For linux-docker, just start the container
    if platform == "linux-docker":
        return _shell_linux_docker(args, info, name)

    # For QEMU platforms, need to handle overlay
    return _shell_qemu(args, info, name, writable)


def _shell_linux_docker(args, info, name) -> int:
    """Start interactive shell for linux-docker image."""
    import sys

    docker_image = info.get("docker_image")
    if not docker_image:
        print(f"Error: No Docker image configured for '{name}'")
        return 1

    # Get user-specified ports or auto-allocate
    user_vnc = getattr(args, 'vnc_port', None)
    user_api = getattr(args, 'api_port', None)

    if user_vnc and user_api:
        vnc_port = int(user_vnc)
        api_port = int(user_api)
    else:
        vnc_port, api_port = allocate_ports(
            vnc_default=int(user_vnc) if user_vnc else 6901,
            api_default=int(user_api) if user_api else 8000
        )
        if not user_vnc or not user_api:
            print(f"Auto-allocated ports: VNC={vnc_port}, API={api_port}")

    detach_mode = getattr(args, 'detach', False)

    container_name = f"cua-shell-{name}"

    # Check if already running
    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", f"name={container_name}"],
        capture_output=True, text=True
    )
    if check_result.stdout.strip():
        print(f"Shell for '{name}' is already running.")
        print(f"\n  VNC: http://localhost:{vnc_port}")
        print(f"  Stop: docker stop {container_name}")
        return 0

    print(f"Starting interactive shell for '{name}'...")

    docker_cmd = [
        "docker", "run", "-t", "--rm",
        "-p", f"{vnc_port}:6901",
        "-p", f"{api_port}:8000",
        "--name", container_name,
        docker_image,
    ]

    # Handle TTY and detach modes
    if detach_mode:
        docker_cmd.insert(2, "-d")
    elif sys.stdin.isatty():
        docker_cmd.insert(2, "-i")

    print(f"\n  VNC: http://localhost:{vnc_port}")

    if detach_mode:
        print(f"  Stop: docker stop {container_name}")
    else:
        print(f"\nPress Ctrl+C to stop.\n")

    try:
        if detach_mode:
            result = subprocess.run(docker_cmd, capture_output=True, text=True, check=False)
            if result.returncode != 0:
                print(f"\n✗ Failed to start container: {result.stderr}")
                return 1
            container_id = result.stdout.strip()[:12]
            print(f"\n✓ Shell started in background (ID: {container_id})")
            return 0
        else:
            subprocess.run(docker_cmd, check=False)
            return 0
    except KeyboardInterrupt:
        print("\nStopped.")
        return 0


def _shell_qemu(args, info, name, writable: bool) -> int:
    """Start interactive shell for QEMU-based image."""
    import sys

    platform = info.get("platform")
    image_path = Path(info.get("path", ""))
    docker_image = info.get("docker_image") or PLATFORMS.get(platform, {}).get("image")

    if not image_path.exists():
        print(f"Error: Image path does not exist: {image_path}")
        return 1

    config = PLATFORMS.get(platform, {})
    marker = config.get("image_marker")
    if marker:
        marker_path = image_path / marker
        if not marker_path.exists():
            print(f"Error: Image '{name}' is not ready (marker missing: {marker})")
            print(f"\nRecreate with: cb image create {platform}")
            return 1

    # Get user-specified ports or auto-allocate
    user_vnc = getattr(args, 'vnc_port', None)
    user_api = getattr(args, 'api_port', None)

    if user_vnc and user_api:
        vnc_port = int(user_vnc)
        api_port = int(user_api)
    else:
        vnc_port, api_port = allocate_ports(
            vnc_default=int(user_vnc) if user_vnc else 8006,
            api_default=int(user_api) if user_api else 5000
        )
        if not user_vnc or not user_api:
            print(f"Auto-allocated ports: VNC={vnc_port}, API={api_port}")

    memory = getattr(args, 'memory', '8G')
    cpus = getattr(args, 'cpus', '8')
    detach_mode = getattr(args, 'detach', False)

    container_name = f"cua-shell-{name}"

    # Check if already running
    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", f"name={container_name}"],
        capture_output=True, text=True
    )
    if check_result.stdout.strip():
        print(f"Shell for '{name}' is already running.")
        print(f"\n  VNC:  http://localhost:{vnc_port}")
        print(f"  API:  http://localhost:{api_port}")
        print(f"  Stop: docker stop {container_name}")
        return 0

    # Prepare volumes
    volumes = []
    overlay_path = None

    if writable:
        # Direct access to golden image - DANGEROUS!
        print("\n" + "=" * 60)
        print("⚠️  WARNING: WRITABLE MODE")
        print("=" * 60)
        print("\nYou are about to modify the golden image directly.")
        print("Any changes will be PERMANENT and affect all future runs.")
        print(f"\nImage: {name}")
        print(f"Path:  {image_path}")

        if sys.stdin.isatty():
            response = input("\nAre you sure? Type 'yes' to continue: ").strip().lower()
            if response != 'yes':
                print("Cancelled.")
                return 0
        else:
            print("\nError: Writable mode requires interactive terminal for confirmation.")
            print("Use --detach if you want to run without a TTY.")
            return 1

        volumes.append(f"{image_path}:/storage")
        print(f"\n🔓 Writable mode: Changes will persist to golden image")
    else:
        # Use overlay to protect golden image
        # NOTE: This is a temporary workaround that copies the entire image.
        # Proper COW overlay support is WIP: https://github.com/trycua/cua/issues/699
        overlay_path = get_data_dir() / "overlays" / f"shell-{name}"

        print(f"\n🔒 Protected mode: Copying golden image to overlay...")
        try:
            create_overlay_copy(image_path, overlay_path, verbose=True)
        except Exception as e:
            print(f"\n✗ Failed to create overlay: {e}")
            return 1

        volumes.append(f"{overlay_path}:/storage")
        print(f"\n✓ Overlay ready (changes will be discarded on exit)")

    print(f"\nStarting {platform} shell for '{name}'...")

    docker_cmd = [
        "docker", "run", "-t", "--rm",
        "-p", f"{vnc_port}:8006",
        "-p", f"{api_port}:5000",
        "--name", container_name,
        "--platform", "linux/amd64",
        "--cap-add", "NET_ADMIN",
        "--stop-timeout", "120",
    ]

    # Handle TTY and detach modes
    if detach_mode:
        docker_cmd.insert(2, "-d")
    elif sys.stdin.isatty():
        docker_cmd.insert(2, "-i")

    for vol in volumes:
        docker_cmd.extend(["-v", vol])

    if os.path.exists("/dev/kvm") and not getattr(args, 'no_kvm', False):
        docker_cmd.extend(["--device=/dev/kvm"])
    else:
        docker_cmd.extend(["-e", "KVM=N"])
        print("Note: Running without KVM (slower)")

    docker_cmd.extend(["-e", f"RAM_SIZE={memory}"])
    docker_cmd.extend(["-e", f"CPU_CORES={cpus}"])
    docker_cmd.append(docker_image)

    print(f"\n  VNC:  http://localhost:{vnc_port}")
    print(f"  API:  http://localhost:{api_port}")

    if detach_mode:
        print(f"  Stop: docker stop {container_name}")
    else:
        print(f"\nPress Ctrl+C to stop.\n")

    try:
        if detach_mode:
            result = subprocess.run(docker_cmd, capture_output=True, text=True, check=False)
            if result.returncode != 0:
                print(f"\n✗ Failed to start container: {result.stderr}")
                return 1
            container_id = result.stdout.strip()[:12]
            print(f"\n✓ Shell started in background (ID: {container_id})")
            return 0
        else:
            subprocess.run(docker_cmd, check=False)
            return 0
    except KeyboardInterrupt:
        print("\nStopped.")
        return 0
    finally:
        # Clean up overlay if we created one (only for non-detach mode)
        if not detach_mode and overlay_path and overlay_path.exists() and not writable:
            print(f"\nCleaning up overlay: {overlay_path}")
            try:
                shutil.rmtree(overlay_path)
            except Exception as e:
                print(f"Warning: Failed to clean up overlay: {e}")


# =============================================================================
# CLI Registration
# =============================================================================

def register_parser(subparsers):
    """Register the image command with the main CLI parser."""
    image_parser = subparsers.add_parser(
        'image',
        help='Manage stored base images'
    )
    image_subparsers = image_parser.add_subparsers(dest='image_command', help='Image command')

    # image list
    list_parser = image_subparsers.add_parser('list', help='List all images')
    list_parser.add_argument('--platform', help='Filter by platform')
    list_parser.add_argument('--format', choices=['table', 'json'], default='table', help='Output format')

    # image info
    info_parser = image_subparsers.add_parser('info', help='Show image details')
    info_parser.add_argument('name', help='Image name')

    # image create
    create_parser = image_subparsers.add_parser('create', help='Create image from platform')
    create_parser.add_argument('platform', help='Platform name (e.g., linux-docker, windows-qemu)')
    create_parser.add_argument('--name', help='Image name (default: same as platform)')
    create_parser.add_argument('--iso', help='Path to ISO file (for QEMU platforms)')
    create_parser.add_argument('--download-iso', action='store_true', dest='download_iso', help='Download Windows 11 ISO (~6GB)')
    create_parser.add_argument('--docker-image', dest='docker_image', help='Override Docker image')
    create_parser.add_argument('--distro', default='ubuntu', choices=['ubuntu', 'fedora'], help='Linux distribution')
    create_parser.add_argument('--version', default='14', help='OS version (e.g., 14 for Android, sonoma for macOS)')
    create_parser.add_argument('--disk', default='64G', help='Disk size (default: 64G)')
    create_parser.add_argument('--memory', default='8G', help='Memory (default: 8G)')
    create_parser.add_argument('--cpus', default='8', help='CPU cores (default: 8)')
    create_parser.add_argument('--winarena-apps', action='store_true', dest='winarena_apps',
                                help='Install WinArena benchmark apps (Chrome, LibreOffice, VLC, etc.)')
    create_parser.add_argument('--detach', '-d', action='store_true', help='Run in background')
    create_parser.add_argument('--force', action='store_true', help='Force recreation')
    create_parser.add_argument('--skip-pull', action='store_true', dest='skip_pull', help="Don't pull Docker image")
    create_parser.add_argument('--no-kvm', action='store_true', dest='no_kvm', help='Disable KVM acceleration')
    create_parser.add_argument('--vnc-port', dest='vnc_port', help='VNC port (default: auto-allocate from 8006)')
    create_parser.add_argument('--api-port', dest='api_port', help='API port (default: auto-allocate from 5000)')

    # image delete
    delete_parser = image_subparsers.add_parser('delete', help='Delete an image')
    delete_parser.add_argument('name', help='Image name')
    delete_parser.add_argument('--force', action='store_true', help='Skip confirmation')

    # image clone
    clone_parser = image_subparsers.add_parser('clone', help='Clone an image')
    clone_parser.add_argument('source', help='Source image name')
    clone_parser.add_argument('target', help='Target image name')
    clone_parser.add_argument('--force', action='store_true', help='Overwrite if target exists')

    # image shell
    shell_parser = image_subparsers.add_parser('shell', help='Interactive shell into image (uses overlay by default)')
    shell_parser.add_argument('name', help='Image name')
    shell_parser.add_argument('--writable', action='store_true', help='Modify golden image directly (dangerous!)')
    shell_parser.add_argument('--detach', '-d', action='store_true', help='Run in background')
    shell_parser.add_argument('--vnc-port', dest='vnc_port', help='VNC port (default: auto-allocate from 8006)')
    shell_parser.add_argument('--api-port', dest='api_port', help='API port (default: auto-allocate from 5000)')
    shell_parser.add_argument('--memory', default='8G', help='Memory (default: 8G)')
    shell_parser.add_argument('--cpus', default='8', help='CPU cores (default: 8)')
    shell_parser.add_argument('--no-kvm', action='store_true', dest='no_kvm', help='Disable KVM acceleration')

    image_parser.set_defaults(image_command='list')


def execute(args) -> int:
    """Execute the image command."""
    cmd = getattr(args, 'image_command', 'list')

    if cmd == 'list':
        return cmd_list(args)
    elif cmd == 'info':
        return cmd_info(args)
    elif cmd == 'create':
        return cmd_create(args)
    elif cmd == 'delete':
        return cmd_delete(args)
    elif cmd == 'clone':
        return cmd_clone(args)
    elif cmd == 'shell':
        return cmd_shell(args)
    else:
        return cmd_list(args)
