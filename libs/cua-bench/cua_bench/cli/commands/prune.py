"""``cb prune``: clean up cua-bench data.

Usage:
    cb prune                  # Show what could be removed
    cb prune --runs           # Remove run logs and the runs registry
    cb prune --images         # Remove the 0.2.x local image store (deprecated)
    cb prune --overlays       # Remove 0.2.x task overlays (deprecated)
    cb prune --all            # All of the above
    cb prune --dry-run        # Show what would be deleted without deleting
    cb prune --runs --keep 20 # Keep the 20 newest runs, delete the rest
    cb prune --runs --older-than 30  # Delete runs older than 30 days

Sandboxes come from the cua SDK since cua-bench 0.3 (local ones are released
when a run ends; `cb env gc` collects idle cloud pools), so ``--docker`` only
prints how to find containers an older cua-bench left behind.
"""

from ._help import examples
import json
import os
import shutil
from pathlib import Path
from typing import Tuple

#: The label cua-bench 0.2.x put on the Docker containers it created.
LEGACY_OWNER_LABEL = "org.trycua.bench.owner=cua-bench"

RESET = "\033[0m"
BOLD = "\033[1m"
CYAN = "\033[36m"
GREEN = "\033[92m"
YELLOW = "\033[33m"
RED = "\033[91m"
GREY = "\033[90m"


def get_data_dir() -> Path:
    xdg = os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share"))
    return Path(xdg) / "cua-bench"


def get_state_dir() -> Path:
    xdg = os.environ.get("XDG_STATE_HOME", os.path.expanduser("~/.local/state"))
    return Path(xdg) / "cua-bench"


def get_runs_dir() -> Path:
    """Get the runs directory path."""
    return get_data_dir() / "runs"


def get_runs_file() -> Path:
    """Get the runs.json file path."""
    return get_state_dir() / "runs.json"


def get_overlays_path() -> Path:
    """Get the overlays directory path (0.2.x)."""
    return get_data_dir() / "overlays"


def get_images_base_path() -> Path:
    """The 0.2.x local image store."""
    return get_data_dir() / "images"


def get_image_registry_path() -> Path:
    return get_state_dir() / "images.json"


def format_size(size_bytes: float) -> str:
    """Format size in human-readable format."""
    for unit in ["B", "KB", "MB", "GB", "TB"]:
        if size_bytes < 1024.0:
            return f"{size_bytes:.1f} {unit}"
        size_bytes /= 1024.0
    return f"{size_bytes:.1f} PB"


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
    prune_images = getattr(args, "images", False) or prune_all
    prune_overlays = getattr(args, "overlays", False) or prune_all
    prune_docker = getattr(args, "docker", False) or prune_all
    prune_runs = getattr(args, "runs", False) or prune_all
    keep = getattr(args, "keep", None)
    older_than = getattr(args, "older_than", None)
    if prune_runs and (keep is not None or older_than is not None):
        return _prune_runs_retained(keep, older_than, dry_run)

    if not any([prune_images, prune_overlays, prune_docker, prune_runs]):
        return _interactive_prune(args)

    print(f"\n{BOLD}CUA-Bench Prune{RESET}")
    print("=" * 60)
    if dry_run:
        print(f"\n{YELLOW}DRY RUN - No changes will be made{RESET}\n")

    total_freed = 0
    items_removed = 0
    for enabled, prune in (
        (prune_overlays, _prune_overlays),
        (prune_images, _prune_images),
        (prune_runs, _prune_runs),
    ):
        if enabled:
            freed, count = prune(dry_run, force)
            total_freed += freed
            items_removed += count
    if prune_docker:
        _docker_notice()

    print("\n" + "=" * 60)
    if dry_run:
        print(f"{YELLOW}Would free: {format_size(total_freed)}{RESET}")
    else:
        print(f"{GREEN}Freed: {format_size(total_freed)}{RESET}")
        print(f"Items removed: {items_removed}")
    return 0


def _prune_runs_retained(keep, older_than, dry_run: bool) -> int:
    """``--runs --keep N`` / ``--older-than DAYS``: delete whole old runs,
    oldest first; never a run that is still going."""
    from cua_bench import retention

    policy = retention.Retention(keep_runs=keep, max_age_days=older_than)
    runs_dir = get_runs_dir()
    doomed = retention.plan(runs_dir, policy)
    freed = sum(retention.dir_size(r) for r in doomed)
    verb = "Would remove" if dry_run else "Removed"
    if not dry_run:
        retention.apply(runs_dir, policy)
    for run in doomed:
        print(f"  {verb} {run}")
    print(f"{verb} {len(doomed)} run(s), {format_size(freed)}")
    return 0


def _docker_notice() -> None:
    print(
        f"\n{YELLOW}--docker is deprecated: cua-bench 0.3 creates no Docker resources itself "
        f"(the cua SDK owns sandboxes and releases them). Containers an older cua-bench left "
        f"behind: docker ps -a --filter label={LEGACY_OWNER_LABEL}{RESET}"
    )


def _interactive_prune(args) -> int:
    """Show what could be removed."""
    runs = calculate_dir_size(get_runs_dir())
    images = calculate_dir_size(get_images_base_path())
    overlays = calculate_dir_size(get_overlays_path())
    print(f"\n{BOLD}CUA-Bench Storage{RESET}")
    print("=" * 60)
    print(f"  Runs:                {format_size(runs):>10}  {GREY}{get_runs_dir()}{RESET}")
    print(
        f"  0.2.x images:        {format_size(images):>10}  {GREY}{get_images_base_path()}{RESET}"
    )
    print(f"  0.2.x overlays:      {format_size(overlays):>10}  {GREY}{get_overlays_path()}{RESET}")
    print(f"\n{CYAN}Commands:{RESET}")
    print(f"  cb prune --runs          {GREY}# Remove run logs and registry{RESET}")
    print(f"  cb prune --images        {GREY}# Remove the 0.2.x image store{RESET}")
    print(f"  cb prune --overlays      {GREY}# Remove 0.2.x overlays{RESET}")
    print(f"  cb prune --all           {GREY}# All of the above{RESET}")
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
    """Remove the 0.2.x local image store and its registry file."""
    images_path = get_images_base_path()
    registry_path = get_image_registry_path()
    if not images_path.exists() and not registry_path.exists():
        print(f"\n{GREY}No 0.2.x images found{RESET}")
        return 0, 0
    size = calculate_dir_size(images_path)
    count = len(list(images_path.iterdir())) if images_path.exists() else 0
    print(f"\n{CYAN}0.2.x images:{RESET}")
    print(f"  Path:  {images_path}")
    print(f"  Size:  {format_size(size)}")
    if dry_run:
        print(f"  {YELLOW}Would remove {count} images{RESET}")
        return size, count
    if not force:
        response = input(f"\n  Remove {count} images? [y/N] ").strip().lower()
        if response != "y":
            print("  Skipped.")
            return 0, 0
    try:
        if images_path.exists():
            shutil.rmtree(images_path)
        if registry_path.exists():
            registry_path.unlink()
        print(f"  {GREEN}Removed {count} images{RESET}")
        return size, count
    except Exception as e:
        print(f"  {RED}Failed: {e}{RESET}")
        return 0, 0


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
        "prune",
        help="Clean up cua-bench run data (and 0.2.x image leftovers)",
        **examples(
            ("Preview what would be removed", "cb prune --all --dry-run"),
            ("Remove run logs without asking", "cb prune --runs --force"),
            ("Keep the 20 newest runs, delete the rest", "cb prune --runs --keep 20"),
            ("Delete runs older than 30 days", "cb prune --runs --older-than 30"),
        ),
    )
    prune_parser.add_argument(
        "--all", "-a", action="store_true", help="Remove runs and 0.2.x image leftovers"
    )
    prune_parser.add_argument(
        "--images", action="store_true", help="Remove 0.2.x stored images (deprecated store)"
    )
    prune_parser.add_argument(
        "--overlays", action="store_true", help="Remove 0.2.x task overlays (deprecated store)"
    )
    prune_parser.add_argument("--runs", action="store_true", help="Remove run logs and registry")
    prune_parser.add_argument(
        "--keep",
        type=int,
        default=None,
        metavar="N",
        help="With --runs: keep the N newest runs and remove only older ones",
    )
    prune_parser.add_argument(
        "--older-than",
        dest="older_than",
        type=float,
        default=None,
        metavar="DAYS",
        help="With --runs: remove only runs older than DAYS",
    )
    prune_parser.add_argument(
        "--docker",
        action="store_true",
        help="Deprecated: cua-bench no longer creates Docker resources",
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
