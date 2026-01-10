"""Prune command - Clean up cua-bench data, images, and Docker resources.

Usage:
    cb prune                  # Interactive mode - shows what would be deleted
    cb prune --all            # Remove everything (images, overlays, runs, docker)
    cb prune --images         # Remove only stored images
    cb prune --overlays       # Remove only task overlays
    cb prune --runs           # Remove only run logs and registry
    cb prune --docker         # Remove only docker containers/images
    cb prune --dry-run        # Show what would be deleted without deleting
"""

import json
import shutil
import subprocess
from pathlib import Path
from typing import Dict, List, Tuple

from .image import (
    format_size,
    get_data_dir,
    get_image_registry_path,
    get_images_base_path,
    get_state_dir,
    load_image_registry,
)


def get_runs_dir() -> Path:
    """Get the runs directory path."""
    return get_data_dir() / "runs"


def get_runs_file() -> Path:
    """Get the runs.json file path."""
    return get_state_dir() / "runs.json"


RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"


def get_overlays_path() -> Path:
    """Get the overlays directory path."""
    return get_data_dir() / "overlays"


def get_docker_resources() -> Tuple[List[Dict], List[Dict]]:
    """Get cua-bench related Docker containers and images."""
    containers = []
    images = []

    # Get containers with cua- prefix
    try:
        result = subprocess.run(
            ["docker", "ps", "-a", "--filter", "name=cua-", "--format", "{{json .}}"],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode == 0:
            for line in result.stdout.strip().split("\n"):
                if line:
                    try:
                        containers.append(json.loads(line))
                    except json.JSONDecodeError:
                        pass
    except Exception:
        pass

    # Get cua-bench related images
    try:
        result = subprocess.run(
            ["docker", "images", "--format", "{{json .}}"],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode == 0:
            for line in result.stdout.strip().split("\n"):
                if line:
                    try:
                        img = json.loads(line)
                        repo = img.get("Repository", "")
                        # Match cua-bench images (windows-qemu, webtop, etc.)
                        if any(
                            x in repo
                            for x in ["trycua/", "windows-qemu", "webtop", "linuxserver/webtop"]
                        ):
                            images.append(img)
                    except json.JSONDecodeError:
                        pass
    except Exception:
        pass

    return containers, images


def calculate_dir_size(path: Path) -> int:
    """Calculate total size of a directory."""
    if not path.exists():
        return 0
    try:
        return sum(f.stat().st_size for f in path.rglob("*") if f.is_file())
    except Exception:
        return 0


def cmd_prune(args) -> int:
    """Execute the prune command."""
    dry_run = getattr(args, "dry_run", False)
    force = getattr(args, "force", False)
    prune_all = getattr(args, "all", False)
    prune_images = getattr(args, "images", False)
    prune_overlays = getattr(args, "overlays", False)
    prune_docker = getattr(args, "docker", False)
    prune_runs = getattr(args, "runs", False)

    # If no specific flags, show interactive mode
    if not any([prune_all, prune_images, prune_overlays, prune_docker, prune_runs]):
        return _interactive_prune(args)

    # If --all, set all flags
    if prune_all:
        prune_images = True
        prune_overlays = True
        prune_docker = True
        prune_runs = True

    total_freed = 0
    items_removed = 0

    print(f"\n{BOLD}CUA-Bench Prune{RESET}")
    print("=" * 60)

    if dry_run:
        print(f"\n{YELLOW}DRY RUN - No changes will be made{RESET}\n")

    # Prune overlays
    if prune_overlays:
        freed, count = _prune_overlays(dry_run, force)
        total_freed += freed
        items_removed += count

    # Prune images
    if prune_images:
        freed, count = _prune_images(dry_run, force)
        total_freed += freed
        items_removed += count

    # Prune docker resources
    if prune_docker:
        freed, count = _prune_docker(dry_run, force)
        total_freed += freed
        items_removed += count

    # Prune runs
    if prune_runs:
        freed, count = _prune_runs(dry_run, force)
        total_freed += freed
        items_removed += count

    # Summary
    print("\n" + "=" * 60)
    if dry_run:
        print(f"{YELLOW}Would free: {format_size(total_freed)}{RESET}")
    else:
        print(f"{GREEN}Freed: {format_size(total_freed)}{RESET}")
        print(f"Items removed: {items_removed}")

    return 0


def _interactive_prune(args) -> int:
    """Interactive prune mode - show what's available to clean."""
    print(f"\n{BOLD}CUA-Bench Storage Overview{RESET}")
    print("=" * 60)

    # Check overlays
    overlays_path = get_overlays_path()
    overlays_size = calculate_dir_size(overlays_path)
    overlays_count = len(list(overlays_path.iterdir())) if overlays_path.exists() else 0

    # Check images
    images_path = get_images_base_path()
    images_size = calculate_dir_size(images_path)
    registry = load_image_registry()
    images_count = len(registry)

    # Check runs
    runs_dir = get_runs_dir()
    runs_file = get_runs_file()
    runs_size = calculate_dir_size(runs_dir)
    runs_count = 0
    if runs_file.exists():
        try:
            with open(runs_file, "r") as f:
                runs_data = json.load(f)
                runs_count = len(runs_data)
        except Exception:
            pass

    # Check state
    state_path = get_state_dir()
    state_size = calculate_dir_size(state_path)

    # Check docker
    containers, docker_images = get_docker_resources()

    # Windows ISO
    iso_path = get_data_dir() / "windows.iso"
    iso_size = iso_path.stat().st_size if iso_path.exists() else 0

    print(f"\n{CYAN}Local Storage:{RESET}")
    print(f"  Overlays:            {format_size(overlays_size):>10}  ({overlays_count} items)")
    print(f"  Images:              {format_size(images_size):>10}  ({images_count} registered)")
    print(f"  Runs:                {format_size(runs_size):>10}  ({runs_count} runs)")
    print(f"  State/Registry:      {format_size(state_size):>10}")
    if iso_size > 0:
        print(f"  Windows ISO:         {format_size(iso_size):>10}")

    print(f"\n{CYAN}Docker Resources:{RESET}")
    print(f"  Containers:          {len(containers):>10} cua-* containers")
    print(f"  Images:              {len(docker_images):>10} cua-bench images")

    total_local = overlays_size + images_size + runs_size + state_size + iso_size
    print(f"\n{BOLD}Total Local Storage: {format_size(total_local)}{RESET}")

    # Show available commands
    print(f"\n{CYAN}Cleanup Commands:{RESET}")
    print(
        f"  cb prune --overlays      {GREY}# Remove task overlays ({format_size(overlays_size)}){RESET}"
    )
    print(
        f"  cb prune --images        {GREY}# Remove all images ({format_size(images_size)}){RESET}"
    )
    print(
        f"  cb prune --runs          {GREY}# Remove run logs and registry ({format_size(runs_size)}){RESET}"
    )
    print(f"  cb prune --docker        {GREY}# Stop containers, remove images{RESET}")
    print(f"  cb prune --all           {GREY}# Remove everything{RESET}")
    print(f"  cb prune --all --force   {GREY}# Remove everything without confirmation{RESET}")
    print(f"\n  {GREY}Add --dry-run to preview without deleting{RESET}")

    return 0


def _prune_overlays(dry_run: bool, force: bool) -> Tuple[int, int]:
    """Remove task overlays."""
    overlays_path = get_overlays_path()

    if not overlays_path.exists():
        print(f"\n{GREY}No overlays directory found{RESET}")
        return 0, 0

    size = calculate_dir_size(overlays_path)
    count = len(list(overlays_path.iterdir()))

    if count == 0:
        print(f"\n{GREY}No overlays to remove{RESET}")
        return 0, 0

    print(f"\n{CYAN}Overlays:{RESET}")
    print(f"  Path:  {overlays_path}")
    print(f"  Size:  {format_size(size)}")
    print(f"  Count: {count}")

    if dry_run:
        print(f"  {YELLOW}Would remove {count} overlays{RESET}")
        return size, count

    if not force:
        response = input(f"\n  Remove {count} overlays? [y/N] ").strip().lower()
        if response != "y":
            print("  Skipped.")
            return 0, 0

    try:
        shutil.rmtree(overlays_path)
        overlays_path.mkdir(parents=True, exist_ok=True)
        print(f"  {GREEN}Removed {count} overlays{RESET}")
        return size, count
    except Exception as e:
        print(f"  {RED}Failed: {e}{RESET}")
        return 0, 0


def _prune_images(dry_run: bool, force: bool) -> Tuple[int, int]:
    """Remove all stored images."""
    images_path = get_images_base_path()
    registry_path = get_image_registry_path()
    registry = load_image_registry()

    if not images_path.exists() and not registry:
        print(f"\n{GREY}No images found{RESET}")
        return 0, 0

    size = calculate_dir_size(images_path)
    count = len(registry)

    print(f"\n{CYAN}Images:{RESET}")
    print(f"  Path:     {images_path}")
    print(f"  Size:     {format_size(size)}")
    print(f"  Registry: {count} images")

    if registry:
        print("\n  Registered images:")
        for name, info in registry.items():
            platform = info.get("platform", "unknown")
            print(f"    - {name} ({platform})")

    if dry_run:
        print(f"\n  {YELLOW}Would remove {count} images and {format_size(size)} of data{RESET}")
        return size, count

    if not force:
        print(f"\n  {RED}WARNING: This will delete all golden images!{RESET}")
        print("  You will need to recreate them with 'cb image create'")
        response = input("\n  Remove all images? Type 'yes' to confirm: ").strip().lower()
        if response != "yes":
            print("  Skipped.")
            return 0, 0

    removed_size = 0
    removed_count = 0

    # Remove image files
    if images_path.exists():
        try:
            removed_size = size
            shutil.rmtree(images_path)
            images_path.mkdir(parents=True, exist_ok=True)
            print(f"  {GREEN}Removed image files{RESET}")
        except Exception as e:
            print(f"  {RED}Failed to remove image files: {e}{RESET}")

    # Clear registry
    if registry_path.exists():
        try:
            registry_path.unlink()
            removed_count = count
            print(f"  {GREEN}Cleared image registry{RESET}")
        except Exception as e:
            print(f"  {RED}Failed to clear registry: {e}{RESET}")

    # Also remove Windows ISO if present
    iso_path = get_data_dir() / "windows.iso"
    if iso_path.exists():
        try:
            iso_size = iso_path.stat().st_size
            iso_path.unlink()
            removed_size += iso_size
            print(f"  {GREEN}Removed Windows ISO ({format_size(iso_size)}){RESET}")
        except Exception as e:
            print(f"  {RED}Failed to remove ISO: {e}{RESET}")

    return removed_size, removed_count


def _prune_docker(dry_run: bool, force: bool) -> Tuple[int, int]:
    """Remove Docker containers and images."""
    containers, images = get_docker_resources()

    if not containers and not images:
        print(f"\n{GREY}No Docker resources found{RESET}")
        return 0, 0

    print(f"\n{CYAN}Docker Resources:{RESET}")

    # List containers
    if containers:
        print(f"\n  Containers ({len(containers)}):")
        for c in containers:
            name = c.get("Names", "unknown")
            status = c.get("Status", "unknown")
            print(f"    - {name} ({status})")

    # List images
    if images:
        print(f"\n  Images ({len(images)}):")
        for img in images:
            repo = img.get("Repository", "unknown")
            tag = img.get("Tag", "latest")
            size = img.get("Size", "unknown")
            print(f"    - {repo}:{tag} ({size})")

    if dry_run:
        print(
            f"\n  {YELLOW}Would remove {len(containers)} containers and {len(images)} images{RESET}"
        )
        return 0, len(containers) + len(images)

    if not force:
        response = input("\n  Remove Docker resources? [y/N] ").strip().lower()
        if response != "y":
            print("  Skipped.")
            return 0, 0

    removed = 0

    # Stop and remove containers
    for c in containers:
        name = c.get("Names", "")
        if name:
            try:
                subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
                print(f"  {GREEN}Removed container: {name}{RESET}")
                removed += 1
            except Exception as e:
                print(f"  {RED}Failed to remove {name}: {e}{RESET}")

    # Remove images
    for img in images:
        image_id = img.get("ID", "")
        repo = img.get("Repository", "")
        if image_id:
            try:
                subprocess.run(["docker", "rmi", "-f", image_id], capture_output=True, check=False)
                print(f"  {GREEN}Removed image: {repo}{RESET}")
                removed += 1
            except Exception as e:
                print(f"  {RED}Failed to remove {repo}: {e}{RESET}")

    return 0, removed


def _prune_runs(dry_run: bool, force: bool) -> Tuple[int, int]:
    """Remove run logs and runs.json registry."""
    runs_dir = get_runs_dir()
    runs_file = get_runs_file()

    # Calculate what we have
    runs_size = calculate_dir_size(runs_dir)
    runs_count = 0

    if runs_file.exists():
        try:
            with open(runs_file, "r") as f:
                runs_data = json.load(f)
                runs_count = len(runs_data)
        except Exception:
            pass

    if not runs_dir.exists() and not runs_file.exists():
        print(f"\n{GREY}No runs found{RESET}")
        return 0, 0

    print(f"\n{CYAN}Runs:{RESET}")
    if runs_dir.exists():
        print(f"  Logs:     {runs_dir}")
        print(f"  Size:     {format_size(runs_size)}")
    if runs_file.exists():
        print(f"  Registry: {runs_file}")
        print(f"  Count:    {runs_count} runs")

    if dry_run:
        print(
            f"\n  {YELLOW}Would remove {runs_count} runs and {format_size(runs_size)} of logs{RESET}"
        )
        return runs_size, runs_count

    if not force:
        response = input("\n  Remove all run logs and registry? [y/N] ").strip().lower()
        if response != "y":
            print("  Skipped.")
            return 0, 0

    removed_size = 0
    removed_count = 0

    # Remove run logs directory
    if runs_dir.exists():
        try:
            removed_size = runs_size
            shutil.rmtree(runs_dir)
            runs_dir.mkdir(parents=True, exist_ok=True)
            print(f"  {GREEN}Removed run logs{RESET}")
        except Exception as e:
            print(f"  {RED}Failed to remove run logs: {e}{RESET}")

    # Remove runs registry
    if runs_file.exists():
        try:
            runs_file.unlink()
            removed_count = runs_count
            print(f"  {GREEN}Cleared runs registry{RESET}")
        except Exception as e:
            print(f"  {RED}Failed to clear registry: {e}{RESET}")

    return removed_size, removed_count


def register_parser(subparsers):
    """Register the prune command with the main CLI parser."""
    prune_parser = subparsers.add_parser(
        "prune", help="Clean up cua-bench data, images, and Docker resources"
    )
    prune_parser.add_argument(
        "--all", "-a", action="store_true", help="Remove everything (images, overlays, docker)"
    )
    prune_parser.add_argument("--images", action="store_true", help="Remove stored images")
    prune_parser.add_argument("--overlays", action="store_true", help="Remove task overlays")
    prune_parser.add_argument("--runs", action="store_true", help="Remove run logs and registry")
    prune_parser.add_argument(
        "--docker", action="store_true", help="Remove Docker containers and images"
    )
    prune_parser.add_argument(
        "--dry-run",
        action="store_true",
        dest="dry_run",
        help="Show what would be deleted without deleting",
    )
    prune_parser.add_argument(
        "--force", "-f", action="store_true", help="Skip confirmation prompts"
    )


def execute(args) -> int:
    """Execute the prune command."""
    return cmd_prune(args)
