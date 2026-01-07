"""System status dashboard.

Shows an overview of images and runs.

Usage:
    cb status                           # Show system overview
"""

import asyncio
import os
import platform as sys_platform
import subprocess

from .platform import PLATFORMS, check_docker, check_kvm, check_lume, check_image_exists
from .image import load_image_registry, get_image_path


def list_running_shells() -> list:
    """List all running cua-shell containers."""
    try:
        result = subprocess.run(
            ["docker", "ps", "--format", "{{.Names}}", "-f", "name=cua-shell-"],
            capture_output=True,
            text=True,
            timeout=10
        )
        if result.returncode == 0:
            containers = [c.strip() for c in result.stdout.strip().split('\n') if c.strip()]
            return [c.replace("cua-shell-", '') for c in containers]
        return []
    except Exception:
        return []


RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"


def execute(args) -> int:
    """Show system status dashboard."""
    return asyncio.run(_execute_async(args))


async def _execute_async(args) -> int:
    """Execute status command asynchronously."""
    print("\ncua-bench Status")
    print("=" * 70)

    # System capabilities
    docker_ok = check_docker()
    kvm_ok = check_kvm()
    lume_ok = check_lume() if sys_platform.system() == "Darwin" else False
    is_macos = sys_platform.system() == "Darwin"
    is_linux = sys_platform.system() == "Linux"

    print(f"\n{BOLD}System{RESET}")
    print("-" * 70)
    print(f"  Docker:  {GREEN}✓ Running{RESET}" if docker_ok else f"  Docker:  {RED}✗ Not running{RESET}")

    if is_linux:
        print(f"  KVM:     {GREEN}✓ Available{RESET}" if kvm_ok else f"  KVM:     {YELLOW}○ Not available{RESET}")

    if is_macos:
        print(f"  Lume:    {GREEN}✓ Installed{RESET}" if lume_ok else f"  Lume:    {GREY}○ Not installed{RESET}")

    # Images
    image_registry = load_image_registry()
    print(f"\n{BOLD}Images{RESET} ({len(image_registry)} registered)")
    print("-" * 70)

    if image_registry:
        ready_count = 0
        for name, info in image_registry.items():
            platform = info.get("platform", "unknown")
            config = PLATFORMS.get(platform, {})
            marker = config.get("image_marker")
            path = get_image_path(name) if info.get("path") else None

            if marker and path:
                marker_path = path / marker
                is_ready = marker_path.exists()
            elif platform == "linux-docker":
                docker_img = info.get("docker_image", "")
                is_ready = docker_img and check_image_exists(docker_img)
            else:
                is_ready = True

            if is_ready:
                ready_count += 1
                status_icon = f"{GREEN}✓{RESET}"
            else:
                status_icon = f"{RED}✗{RESET}"

            print(f"  {status_icon} {name:<20} ({platform})")

        print(f"\n  {ready_count}/{len(image_registry)} images ready")
    else:
        print(f"  {GREY}No images registered.{RESET}")
        print(f"\n  Create one with:")
        print(f"    cb image create linux-docker")
        print(f"    cb image create windows-qemu --download-iso")

    # Interactive shells
    running_shells = list_running_shells()
    print(f"\n{BOLD}Interactive Shells{RESET} ({len(running_shells)} running)")
    print("-" * 70)

    if running_shells:
        for name in running_shells:
            print(f"  {GREEN}●{RESET} {name:<20}")
    else:
        print(f"  {GREY}No shells running.{RESET}")
        print(f"\n  Start one with:")
        print(f"    cb image shell <image>")

    # Runs (sessions)
    try:
        from cua_bench.sessions import list_sessions
        from cua_bench.sessions.providers.docker import DockerProvider

        sessions = list_sessions()
        docker_provider = DockerProvider()

        # Group by run_id
        from collections import defaultdict
        runs = defaultdict(list)
        for session in sessions:
            run_id = session.get("run_id", "-")
            if run_id != "-":
                runs[run_id].append(session)

        print(f"\n{BOLD}Runs{RESET} ({len(runs)} active)")
        print("-" * 70)

        if runs:
            for run_id, run_sessions in list(runs.items())[:5]:  # Show top 5 runs
                # Count statuses
                running = 0
                completed = 0
                failed = 0

                for session in run_sessions:
                    session_id = session.get("session_id")
                    if session_id:
                        try:
                            status_info = await docker_provider.get_session_status(session_id)
                            status = status_info.get("status", "unknown")
                            if status == "running":
                                running += 1
                            elif status == "completed":
                                completed += 1
                            elif status in ("failed", "error"):
                                failed += 1
                        except Exception:
                            pass

                total = len(run_sessions)
                agent = run_sessions[0].get("agent", "-") if run_sessions else "-"

                if running > 0:
                    status_icon = f"{GREEN}●{RESET}"
                elif completed == total:
                    status_icon = f"{CYAN}✓{RESET}"
                elif failed > 0:
                    status_icon = f"{RED}✗{RESET}"
                else:
                    status_icon = f"{GREY}○{RESET}"

                print(f"  {status_icon} {run_id:<20} agent={agent} ({completed}/{total} done)")

            if len(runs) > 5:
                print(f"\n  ... and {len(runs) - 5} more runs")
        else:
            print(f"  {GREY}No runs in progress.{RESET}")
            print(f"\n  Start a run with:")
            print(f"    cb run <task>")

    except Exception as e:
        print(f"\n{BOLD}Runs{RESET}")
        print("-" * 70)
        print(f"  {GREY}Could not load runs: {e}{RESET}")

    # Quick commands
    print("\n" + "=" * 70)
    print(f"\n{BOLD}Quick Commands{RESET}")
    print("-" * 70)
    print("  cb platform list              # Show available platforms")
    print("  cb image list                 # Show registered images")
    print("  cb image shell <name>         # Interactive shell into image")
    print("  cb run list                   # Show active runs")
    print()

    return 0


def register_parser(subparsers):
    """Register the status command with the main CLI parser."""
    subparsers.add_parser(
        'status',
        help='Show system status dashboard'
    )
