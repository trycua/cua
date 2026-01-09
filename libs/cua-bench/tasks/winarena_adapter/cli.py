#!/usr/bin/env python3
"""CLI for Windows Arena adapter management.

This CLI provides commands for:
- VM management: setup, start, stop, status
- Task management: list, run

Usage:
    python -m tasks.winarena_adapter.cli vm setup --iso /path/to/windows.iso
    python -m tasks.winarena_adapter.cli vm start
    python -m tasks.winarena_adapter.cli vm stop
    python -m tasks.winarena_adapter.cli vm status
    python -m tasks.winarena_adapter.cli task list
"""

import argparse
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

# Get the adapter directory
ADAPTER_DIR = Path(__file__).parent
INFRA_DIR = ADAPTER_DIR / "infra"
SCRIPTS_DIR = INFRA_DIR / "scripts"

# Windows 11 Enterprise Evaluation ISO download URL (Microsoft official)
WINDOWS_ISO_URL = "https://go.microsoft.com/fwlink/?linkid=2334167&clcid=0x409"

# Default container name
DEFAULT_CONTAINER_NAME = "winarena"


def get_docker_image_name(mode: str = "azure") -> str:
    """Get the Docker image name based on mode."""
    if mode == "dev":
        return "trycua/winarena-dev:latest"
    return "trycua/winarena:latest"


def check_docker():
    """Check if Docker is running."""
    try:
        result = subprocess.run(["docker", "info"], capture_output=True, timeout=10)
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


def pull_image(image_name: str):
    """Pull Docker image from registry."""
    print(f"Pulling image: {image_name}")
    subprocess.run(["docker", "pull", image_name], check=True)


def get_data_dir() -> Path:
    """Get the cua-bench data directory (XDG compliant)."""
    xdg_data = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return Path(xdg_data) / "cua-bench"


def get_image_path(name: str = "windows-qemu") -> Path:
    """Get the image path for a named image."""
    image_path = get_data_dir() / "images" / name
    image_path.mkdir(parents=True, exist_ok=True)
    return image_path


def get_worker_path(worker_id: int) -> Path:
    """Get the worker-specific storage path (CoW overlay)."""
    worker_path = get_data_dir() / "workers" / str(worker_id)
    worker_path.mkdir(parents=True, exist_ok=True)
    return worker_path


def get_iso_path() -> Path:
    """Get the default ISO path."""
    return get_data_dir() / "windows.iso"


def download_windows_iso(dest_path: Path) -> bool:
    """Download Windows 11 ISO from Microsoft.

    Returns True if download succeeded, False otherwise.
    """
    print("\nDownloading Windows 11 Enterprise Evaluation ISO...")
    print(f"  URL: {WINDOWS_ISO_URL}")
    print(f"  Destination: {dest_path}")
    print("\n  This is a large file (~6GB) and may take a while.\n")

    try:
        # Create parent directory if needed
        dest_path.parent.mkdir(parents=True, exist_ok=True)

        # Download with progress
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
        print("\n\n✓ Download complete!")
        return True
    except Exception as e:
        print(f"\n\n✗ Download failed: {e}")
        return False


# =============================================================================
# VM Commands
# =============================================================================


def cmd_vm_setup(args):
    """Set up the Windows VM base image.

    This command:
    1. Checks for Windows ISO (downloads if needed)
    2. Pulls the winarena Docker image if not present
    3. Starts the Windows VM with the ISO mounted
    4. Waits for Windows to fully initialize (~45-60 minutes)
    5. Gracefully shuts down to create a persistent base image
    """
    if not check_docker():
        print("Error: Docker is not running. Please start Docker and try again.")
        return 1

    image_name = get_docker_image_name()
    image_path = get_image_path()

    # Check if base image already exists
    windows_boot = image_path / "windows.boot"
    if windows_boot.exists() and not args.force:
        print(f"Base image already exists at: {image_path}")
        print("Use --force to recreate it.")
        return 0

    # Determine ISO path
    iso_path = None
    if args.iso:
        iso_path = Path(args.iso).resolve()
        if not iso_path.exists():
            print(f"Error: ISO file not found: {iso_path}")
            return 1
    else:
        # Check default location
        default_iso = get_iso_path()
        if default_iso.exists():
            iso_path = default_iso
            print(f"Using existing ISO: {iso_path}")
        else:
            # No ISO found - prompt user
            print("\n" + "=" * 60)
            print("Windows ISO Required")
            print("=" * 60)
            print("\nTo create a base image, you need a Windows 11 ISO file.")
            print("\nOptions:")
            print("  1. Download automatically (~6GB):")
            print("     python -m tasks.winarena_adapter.cli vm setup --download-iso")
            print("\n  2. Provide your own ISO:")
            print("     python -m tasks.winarena_adapter.cli vm setup --iso /path/to/windows.iso")
            print("\n  3. Download manually from Microsoft:")
            print(f"     {WINDOWS_ISO_URL}")
            print(f"     Then save to: {default_iso}")
            print()

            if args.download_iso:
                if not download_windows_iso(default_iso):
                    return 1
                iso_path = default_iso
            else:
                return 1

    print(f"\nUsing Windows ISO: {iso_path}")

    # Check if image exists, pull if not
    if not check_image_exists(image_name):
        print(f"Image {image_name} not found locally.")
        if args.skip_pull:
            print("Error: --skip-pull is set but image not found.")
            return 1
        pull_image(image_name)

    print("\nPreparing Windows VM base image...")
    print(f"Base image path: {image_path}")
    print("\nThis will:")
    print("  1. Start a Windows 11 VM in Docker with the ISO")
    print("  2. Install Windows and required software (~45-60 minutes)")
    print("  3. Gracefully shut down to save the base image")
    print(f"\nYou can monitor progress at: http://localhost:{args.browser_port}")
    print()

    # Check if container already running
    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", "name=winarena-setup"], capture_output=True, text=True
    )
    if check_result.stdout.strip():
        print("Container 'winarena-setup' is already running.")
        print("Use 'docker stop winarena-setup' to stop it first.")
        return 1

    # Determine if we should run detached
    detach_mode = getattr(args, "detach", False)

    # Get the setup folder path (contains setup.ps1, tools_config.json, etc.)
    setup_path = INFRA_DIR / "vm" / "setup"

    # Build docker command
    # Always use -t for pseudo-TTY (required by QEMU), but -i only if interactive
    docker_cmd = [
        "docker",
        "run",
        "-t",  # Always allocate pseudo-TTY for QEMU
        "--rm",
        "-p",
        f"{args.browser_port}:8006",
        "-p",
        f"{args.rdp_port}:3389",
        "--name",
        "winarena-setup",
        "--platform",
        "linux/amd64",
        "-v",
        f"{image_path}:/storage",
        "-v",
        f"{iso_path}:/custom.iso:ro",  # Mount the Windows ISO
        "-v",
        f"{setup_path}:/shared",  # Mount setup folder for live updates
        "--cap-add",
        "NET_ADMIN",
        "--stop-timeout",
        "120",
    ]

    # Add detach flag if requested
    if detach_mode:
        docker_cmd.insert(2, "-d")  # Insert after "run"
    elif sys.stdin.isatty():
        docker_cmd.insert(2, "-i")  # Add interactive flag if TTY available

    # Add KVM if available
    if os.path.exists("/dev/kvm") and not args.no_kvm:
        docker_cmd.extend(["--device=/dev/kvm"])
    else:
        docker_cmd.extend(["-e", "KVM=N"])
        print("Note: Running without KVM (slower)")

    # Add RAM and CPU settings
    docker_cmd.extend(["-e", f"RAM_SIZE={args.ram}"])
    docker_cmd.extend(["-e", f"CPU_CORES={args.cpus}"])

    # Handle --winarena-apps flag
    install_winarena_apps = getattr(args, "winarena_apps", False)
    if install_winarena_apps:
        print("Will install Windows Arena benchmark apps (Chrome, LibreOffice, VLC, etc.)")
        # Write config file to shared folder that setup.ps1 will read
        config_file = setup_path / "install_config.json"
        import json

        config_file.write_text(json.dumps({"INSTALL_WINARENA_APPS": True}, indent=2))
    else:
        # Write config with apps disabled
        config_file = setup_path / "install_config.json"
        import json

        config_file.write_text(json.dumps({"INSTALL_WINARENA_APPS": False}, indent=2))

    # Override the default ENTRYPOINT (/bin/bash -c) to run entry.sh directly with arguments
    # This ensures arguments are passed correctly to the script
    docker_cmd.extend(["--entrypoint", "./entry.sh"])

    docker_cmd.append(image_name)

    # Run with prepare-image flag (these become CMD args to the entrypoint)
    docker_cmd.extend(["--prepare-image", "true", "--start-client", "false"])

    print(
        f"Running: docker run {'(detached) ' if detach_mode else ''}{' '.join(docker_cmd[2:10])}..."
    )

    try:
        if detach_mode:
            # Start container in detached mode
            result = subprocess.run(docker_cmd, capture_output=True, text=True, check=False)
            if result.returncode != 0:
                print(f"\n✗ Failed to start container: {result.stderr}")
                return 1

            container_id = result.stdout.strip()[:12]
            print(f"\n✓ Container started in background (ID: {container_id})")
            print("\nMonitor progress:")
            print(f"  Browser: http://localhost:{args.browser_port}")
            print("  Logs:    docker logs -f winarena-setup")
            print("  Status:  docker ps -f name=winarena-setup")
            print(f"\nWhen complete, the base image will be at: {image_path}")
            print("\nTo check if base image was created:")
            print(f"  ls -la {image_path}")
            return 0
        else:
            # Run the container - this will block until Windows is fully set up
            # and gracefully shut down (prepare-image=true triggers this)
            result = subprocess.run(docker_cmd, check=False)

            # Check if base image was created
            windows_boot = image_path / "windows.boot"
            if windows_boot.exists():
                print("\n✓ Base image prepared successfully!")
                print(f"  Path: {image_path}")
                return 0
            else:
                print("\n✗ Base image was not created.")
                print(f"  Container exit code: {result.returncode}")
                print(f"  Expected file: {windows_boot}")
                print("\nTry running with --detach flag for background operation:")
                print("  python -m tasks.winarena_adapter.cli vm setup --detach")
                return 1
    except subprocess.CalledProcessError as e:
        print(f"\n✗ Failed to prepare image: {e}")
        return 1
    except KeyboardInterrupt:
        print("\n\nInterrupted. The container will be stopped.")
        return 1


def cmd_vm_start(args):
    """Start the base image for testing/development."""
    if not check_docker():
        print("Error: Docker is not running. Please start Docker and try again.")
        return 1

    image_name = get_docker_image_name()
    image_path = get_image_path()
    setup_path = INFRA_DIR / "vm" / "setup"

    # Check if base image exists
    windows_boot = image_path / "windows.boot"
    if not windows_boot.exists():
        print("Error: No base image found.")
        print("Run 'python -m tasks.winarena_adapter.cli vm setup' first.")
        return 1

    # Check if image exists
    if not check_image_exists(image_name):
        if args.skip_pull:
            print(f"Error: Image {image_name} not found.")
            return 1
        pull_image(image_name)

    container_name = args.container_name or DEFAULT_CONTAINER_NAME

    # Check if container already running
    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", f"name={container_name}"], capture_output=True, text=True
    )
    if check_result.stdout.strip():
        print(f"Container '{container_name}' is already running.")
        print(f"  Browser: http://localhost:{args.browser_port}")
        print("  Stop with: python -m tasks.winarena_adapter.cli vm stop")
        return 0

    print("Starting Windows Arena base image...")
    print(f"  Browser: http://localhost:{args.browser_port}")
    print(f"  RDP: localhost:{args.rdp_port}")

    # Build docker command
    docker_cmd = [
        "docker",
        "run",
        "-d",  # Always detached for start command
        "-t",  # Pseudo-TTY for QEMU
        "--rm",
        "-p",
        f"{args.browser_port}:8006",
        "-p",
        f"{args.rdp_port}:3389",
        "--name",
        container_name,
        "--platform",
        "linux/amd64",
        "-v",
        f"{image_path}:/storage",
        "-v",
        f"{setup_path}:/shared",  # Mount setup folder for live updates
        "--cap-add",
        "NET_ADMIN",
        "--stop-timeout",
        "120",
    ]

    # Add KVM if available
    if os.path.exists("/dev/kvm") and not args.no_kvm:
        docker_cmd.extend(["--device=/dev/kvm"])
    else:
        docker_cmd.extend(["-e", "KVM=N"])
        print("Note: Running without KVM (slower)")

    # Add RAM and CPU settings
    docker_cmd.extend(["-e", f"RAM_SIZE={args.ram}"])
    docker_cmd.extend(["-e", f"CPU_CORES={args.cpus}"])

    # Just run start_vm.sh to boot the VM (no client, no setup logic)
    docker_cmd.extend(["--entrypoint", "./start_vm.sh"])
    docker_cmd.append(image_name)

    try:
        result = subprocess.run(docker_cmd, capture_output=True, text=True, check=False)
        if result.returncode != 0:
            print(f"\n✗ Failed to start container: {result.stderr}")
            return 1

        container_id = result.stdout.strip()[:12]
        print(f"\n✓ Container started (ID: {container_id})")
        print("\nMonitor:")
        print(f"  Browser: http://localhost:{args.browser_port}")
        print(f"  Logs:    docker logs -f {container_name}")
        print("  Stop:    python -m tasks.winarena_adapter.cli vm stop")
        return 0
    except Exception as e:
        print(f"\n✗ Failed to start container: {e}")
        return 1


def cmd_vm_stop(args):
    """Stop the running Windows Arena container."""
    container_name = args.container_name or DEFAULT_CONTAINER_NAME

    # Check if container is running
    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", f"name={container_name}"], capture_output=True, text=True
    )
    if not check_result.stdout.strip():
        print(f"No running container named '{container_name}' found.")
        return 0

    print(f"Stopping container '{container_name}'...")
    try:
        subprocess.run(
            ["docker", "stop", container_name],
            check=True,
            timeout=150,  # Allow for graceful shutdown
        )
        print(f"✓ Container '{container_name}' stopped.")
        return 0
    except subprocess.TimeoutExpired:
        print("Timeout waiting for container to stop. Forcing...")
        subprocess.run(["docker", "kill", container_name], check=False)
        return 1
    except subprocess.CalledProcessError as e:
        print(f"✗ Failed to stop container: {e}")
        return 1


def cmd_vm_status(args):
    """Check the status of Windows Arena setup."""
    print("\nWindows Arena Status\n" + "=" * 40)

    # Check Docker
    docker_ok = check_docker()
    print(f"Docker:       {'✓ Running' if docker_ok else '✗ Not running'}")

    # Check image
    image_name = get_docker_image_name()
    image_ok = check_image_exists(image_name) if docker_ok else False
    print(f"Image:        {'✓ ' + image_name if image_ok else '✗ Not found'}")

    # Check Windows ISO
    iso_path = get_iso_path()
    iso_ok = iso_path.exists()
    print(f"Windows ISO:  {'✓ Found' if iso_ok else '✗ Not found'}")
    print(f"  Path:       {iso_path}")

    # Check base image
    image_path = get_image_path()
    windows_boot = image_path / "windows.boot"
    golden_ok = windows_boot.exists()
    print(f"Golden Image: {'✓ Ready' if golden_ok else '✗ Not prepared'}")
    print(f"  Path:       {image_path}")

    # Check KVM
    kvm_ok = os.path.exists("/dev/kvm")
    print(f"KVM:          {'✓ Available' if kvm_ok else '○ Not available (will run slower)'}")

    # Check running container
    container_name = DEFAULT_CONTAINER_NAME
    check_result = subprocess.run(
        ["docker", "ps", "-q", "-f", f"name={container_name}"], capture_output=True, text=True
    )
    container_running = bool(check_result.stdout.strip())
    print(f"Container:    {'✓ Running' if container_running else '○ Not running'}")

    print()

    if not golden_ok:
        if not iso_ok:
            print("Next steps:")
            print("  1. Download Windows ISO:")
            print("     python -m tasks.winarena_adapter.cli vm setup --download-iso")
            print("  Or provide your own:")
            print("     python -m tasks.winarena_adapter.cli vm setup --iso /path/to/windows.iso")
        else:
            print("Next step: Run 'python -m tasks.winarena_adapter.cli vm setup'")
    elif not docker_ok:
        print("Please start Docker to run Windows Arena tasks.")
    else:
        print("Ready to run!")
        print("  Start VM:   python -m tasks.winarena_adapter.cli vm start")
        print("  List tasks: python -m tasks.winarena_adapter.cli task list")

    return 0


# =============================================================================
# Task Commands
# =============================================================================


def cmd_task_list(args):
    """List all Windows Arena tasks."""
    from .task_loader import load_waa_tasks

    tasks = load_waa_tasks()

    # Group by domain
    domains = {}
    for task in tasks:
        domain = task.metadata.get("domain", "unknown")
        if domain not in domains:
            domains[domain] = []
        domains[domain].append(task)

    print(f"\nWindows Arena Tasks ({len(tasks)} total)\n")
    print("=" * 60)

    for domain in sorted(domains.keys()):
        domain_tasks = domains[domain]
        print(f"\n{domain.upper()} ({len(domain_tasks)} tasks)")
        print("-" * 40)

        if args.verbose:
            for i, task in enumerate(domain_tasks):
                desc = (
                    task.description[:70] + "..."
                    if len(task.description) > 70
                    else task.description
                )
                print(f"  {task.task_id[:36]}  {desc}")
        else:
            # Just show count
            pass

    print(f"\n{'=' * 60}")
    print(f"Total: {len(tasks)} tasks across {len(domains)} domains")

    return 0


def cmd_task_run(args):
    """Run the Windows Arena container with a specific task."""
    if not check_docker():
        print("Error: Docker is not running. Please start Docker and try again.")
        return 1

    image_name = get_docker_image_name()
    image_path = get_image_path()

    # Check if base image exists
    windows_boot = image_path / "windows.boot"
    if not windows_boot.exists():
        print("Error: No base image found.")
        print("Run 'python -m tasks.winarena_adapter.cli vm setup' first.")
        return 1

    # Check if image exists
    if not check_image_exists(image_name):
        if args.skip_pull:
            print(f"Error: Image {image_name} not found.")
            return 1
        pull_image(image_name)

    print("Starting Windows Arena container...")
    print(f"  Browser: http://localhost:{args.browser_port}")
    print(f"  RDP: localhost:{args.rdp_port}")

    # Get the setup folder path (contains setup.ps1, tools_config.json, etc.)
    setup_path = INFRA_DIR / "vm" / "setup"

    container_name = args.container_name or DEFAULT_CONTAINER_NAME

    # Build docker command
    docker_cmd = [
        "docker",
        "run",
        "-it" if sys.stdout.isatty() else "-i",
        "--rm",
        "-p",
        f"{args.browser_port}:8006",
        "-p",
        f"{args.rdp_port}:3389",
        "--name",
        container_name,
        "--platform",
        "linux/amd64",
        "-v",
        f"{image_path}:/storage",
        "-v",
        f"{setup_path}:/shared",  # Mount setup folder for live updates
        "--cap-add",
        "NET_ADMIN",
        "--stop-timeout",
        "120",
    ]

    # Add KVM if available
    if os.path.exists("/dev/kvm") and not args.no_kvm:
        docker_cmd.extend(["--device=/dev/kvm"])
    else:
        docker_cmd.extend(["-e", "KVM=N"])

    # Add RAM and CPU settings
    docker_cmd.extend(["-e", f"RAM_SIZE={args.ram}"])
    docker_cmd.extend(["-e", f"CPU_CORES={args.cpus}"])

    # Add API keys from environment
    if os.environ.get("OPENAI_API_KEY"):
        docker_cmd.extend(["-e", f"OPENAI_API_KEY={os.environ['OPENAI_API_KEY']}"])
    elif os.environ.get("AZURE_API_KEY") and os.environ.get("AZURE_ENDPOINT"):
        docker_cmd.extend(["-e", f"AZURE_API_KEY={os.environ['AZURE_API_KEY']}"])
        docker_cmd.extend(["-e", f"AZURE_ENDPOINT={os.environ['AZURE_ENDPOINT']}"])

    docker_cmd.append(image_name)

    # Entrypoint
    if args.interactive:
        docker_cmd.extend(["--entrypoint", "/bin/bash"])
    else:
        docker_cmd.extend(
            [
                "/bin/bash",
                "-c",
                f"./entry.sh --prepare-image false --start-client {'false' if args.no_client else 'true'}",
            ]
        )

    try:
        subprocess.run(docker_cmd, check=True)
        return 0
    except subprocess.CalledProcessError as e:
        print(f"Container exited with error: {e}")
        return 1
    except KeyboardInterrupt:
        print("\nStopping container...")
        return 0


# =============================================================================
# Main CLI
# =============================================================================


def main():
    parser = argparse.ArgumentParser(
        description="Windows Arena CLI for cua-bench",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # VM Management
  python -m tasks.winarena_adapter.cli vm status           # Check setup status
  python -m tasks.winarena_adapter.cli vm setup --detach   # Create base image
  python -m tasks.winarena_adapter.cli vm start            # Start the VM
  python -m tasks.winarena_adapter.cli vm stop             # Stop the VM

  # Task Management
  python -m tasks.winarena_adapter.cli task list           # List all tasks
  python -m tasks.winarena_adapter.cli task list --verbose # List with details
  python -m tasks.winarena_adapter.cli task run            # Run benchmark client
""",
    )

    subparsers = parser.add_subparsers(dest="command", help="Command group")

    # ==========================================================================
    # VM command group
    # ==========================================================================
    vm_parser = subparsers.add_parser("vm", help="VM management commands")
    vm_subparsers = vm_parser.add_subparsers(dest="vm_command", help="VM command")

    # vm setup
    vm_setup_parser = vm_subparsers.add_parser("setup", help="Create the Windows VM base image")
    vm_setup_parser.add_argument("--iso", help="Path to Windows 11 ISO file")
    vm_setup_parser.add_argument(
        "--download-iso", action="store_true", help="Download Windows 11 ISO from Microsoft (~6GB)"
    )
    vm_setup_parser.add_argument(
        "--detach", "-d", action="store_true", help="Run in background (detached mode)"
    )
    vm_setup_parser.add_argument(
        "--force", action="store_true", help="Force recreation of base image"
    )
    vm_setup_parser.add_argument(
        "--skip-pull", action="store_true", help="Don't pull image if missing"
    )
    vm_setup_parser.add_argument("--no-kvm", action="store_true", help="Disable KVM acceleration")
    vm_setup_parser.add_argument(
        "--winarena-apps",
        action="store_true",
        help="Install Windows Arena benchmark apps (Chrome, LibreOffice, VLC, etc.)",
    )
    vm_setup_parser.add_argument("--ram", default="8G", help="RAM size (default: 8G)")
    vm_setup_parser.add_argument("--cpus", default="8", help="CPU cores (default: 8)")
    vm_setup_parser.add_argument(
        "--browser-port", default="8006", help="noVNC port (default: 8006)"
    )
    vm_setup_parser.add_argument("--rdp-port", default="3390", help="RDP port (default: 3390)")
    vm_setup_parser.set_defaults(func=cmd_vm_setup)

    # vm start
    vm_start_parser = vm_subparsers.add_parser(
        "start", help="Start the Windows VM (no benchmark client)"
    )
    vm_start_parser.add_argument(
        "--skip-pull", action="store_true", help="Don't pull image if missing"
    )
    vm_start_parser.add_argument("--no-kvm", action="store_true", help="Disable KVM acceleration")
    vm_start_parser.add_argument("--ram", default="8G", help="RAM size (default: 8G)")
    vm_start_parser.add_argument("--cpus", default="8", help="CPU cores (default: 8)")
    vm_start_parser.add_argument(
        "--browser-port", default="8006", help="noVNC port (default: 8006)"
    )
    vm_start_parser.add_argument("--rdp-port", default="3390", help="RDP port (default: 3390)")
    vm_start_parser.add_argument(
        "--container-name", help=f"Container name (default: {DEFAULT_CONTAINER_NAME})"
    )
    vm_start_parser.set_defaults(func=cmd_vm_start)

    # vm stop
    vm_stop_parser = vm_subparsers.add_parser("stop", help="Stop the Windows VM container")
    vm_stop_parser.add_argument(
        "--container-name", help=f"Container name (default: {DEFAULT_CONTAINER_NAME})"
    )
    vm_stop_parser.set_defaults(func=cmd_vm_stop)

    # vm status
    vm_status_parser = vm_subparsers.add_parser("status", help="Check Windows Arena setup status")
    vm_status_parser.set_defaults(func=cmd_vm_status)

    # ==========================================================================
    # Task command group
    # ==========================================================================
    task_parser = subparsers.add_parser("task", help="Task management commands")
    task_subparsers = task_parser.add_subparsers(dest="task_command", help="Task command")

    # task list
    task_list_parser = task_subparsers.add_parser("list", help="List all Windows Arena tasks")
    task_list_parser.add_argument("--verbose", "-v", action="store_true", help="Show task details")
    task_list_parser.set_defaults(func=cmd_task_list)

    # task run
    task_run_parser = task_subparsers.add_parser(
        "run", help="Run the Windows Arena container with benchmark client"
    )
    task_run_parser.add_argument(
        "--interactive", "-i", action="store_true", help="Start interactive shell"
    )
    task_run_parser.add_argument(
        "--no-client", action="store_true", help="Don't start the benchmark client"
    )
    task_run_parser.add_argument(
        "--skip-pull", action="store_true", help="Don't pull image if missing"
    )
    task_run_parser.add_argument("--no-kvm", action="store_true", help="Disable KVM acceleration")
    task_run_parser.add_argument("--ram", default="8G", help="RAM size (default: 8G)")
    task_run_parser.add_argument("--cpus", default="8", help="CPU cores (default: 8)")
    task_run_parser.add_argument(
        "--browser-port", default="8006", help="noVNC port (default: 8006)"
    )
    task_run_parser.add_argument("--rdp-port", default="3390", help="RDP port (default: 3390)")
    task_run_parser.add_argument(
        "--container-name", help=f"Container name (default: {DEFAULT_CONTAINER_NAME})"
    )
    task_run_parser.set_defaults(func=cmd_task_run)

    # Parse arguments
    args = parser.parse_args()

    if not args.command:
        parser.print_help()
        return 1

    # Handle subcommand groups
    if args.command == "vm":
        if not args.vm_command:
            vm_parser.print_help()
            return 1
        return args.func(args)
    elif args.command == "task":
        if not args.task_command:
            task_parser.print_help()
            return 1
        return args.func(args)
    else:
        parser.print_help()
        return 1


if __name__ == "__main__":
    sys.exit(main())
