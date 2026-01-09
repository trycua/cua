"""Centralized registry management for cua-bench.

Handles cloning, updating, and resolving paths from the cua dataset registry.
"""

import os
import subprocess
from pathlib import Path
from typing import Optional

# ANSI colors
GREEN = "\033[92m"
CYAN = "\033[96m"
YELLOW = "\033[93m"
RED = "\033[91m"
GREY = "\033[90m"
RESET = "\033[0m"


def get_registry_path() -> Path:
    """Get the path to the cua dataset registry.

    Priority:
    1. CUA_REGISTRY_HOME environment variable
    2. Default: ~/.cua/cbregistry/libs/cua-bench

    Returns:
        Path to the cua-bench directory in the cua repo (parent of 'datasets' folder)
    """
    registry_home = os.environ.get("CUA_REGISTRY_HOME")
    if registry_home:
        return Path(registry_home)
    return Path.home() / ".cua" / "cbregistry" / "libs" / "cua-bench"


def ensure_registry(update: bool = True, verbose: bool = True) -> Path:
    """Ensure the registry is cloned and optionally updated.

    Args:
        update: Whether to run git pull if registry exists (default: True)
        verbose: Whether to print status messages (default: True)

    Returns:
        Path to the datasets directory

    Raises:
        RuntimeError: If registry cannot be cloned
    """
    registry_path = get_registry_path()
    cua_repo_path = registry_path.parent.parent  # Go up to ~/.cua/cbregistry

    if not cua_repo_path.exists() or not (cua_repo_path / ".git").exists():
        # Clone cua repo
        if verbose:
            print(f"{CYAN}Cloning cua repository...{RESET}")
        cua_repo_path.parent.mkdir(parents=True, exist_ok=True)

        # Try SSH first
        try:
            result = subprocess.run(
                ["git", "clone", "git@github.com:trycua/cua.git", str(cua_repo_path)],
                capture_output=True,
                text=True,
                timeout=120,
            )
            if result.returncode != 0:
                raise Exception(f"SSH clone failed: {result.stderr}")
            if verbose:
                print(f"{GREEN}✓ Repository cloned successfully via SSH{RESET}")
        except Exception as ssh_error:
            if verbose:
                print(f"{YELLOW}SSH clone failed, trying HTTPS...{RESET}")
            # Try HTTPS
            try:
                result = subprocess.run(
                    ["git", "clone", "https://github.com/trycua/cua.git", str(cua_repo_path)],
                    capture_output=True,
                    text=True,
                    timeout=120,
                )
                if result.returncode != 0:
                    raise Exception(f"HTTPS clone failed: {result.stderr}")
                if verbose:
                    print(f"{GREEN}✓ Repository cloned successfully via HTTPS{RESET}")
            except Exception as https_error:
                # Check if git is installed
                try:
                    subprocess.run(["git", "--version"], capture_output=True, check=True)
                    raise RuntimeError(
                        f"Failed to clone cua repository. Both SSH and HTTPS failed.\n"
                        f"SSH error: {ssh_error}\n"
                        f"HTTPS error: {https_error}"
                    )
                except (subprocess.CalledProcessError, FileNotFoundError):
                    raise RuntimeError("git is not installed. Please install git and try again.")
    elif update:
        # Repo exists, update it with git pull
        if verbose:
            print(f"{CYAN}Updating datasets...{RESET}")
        try:
            result = subprocess.run(
                ["git", "-C", str(cua_repo_path), "pull"],
                capture_output=True,
                text=True,
                timeout=30,
            )
            if result.returncode == 0:
                if "Already up to date" in result.stdout or "Already up-to-date" in result.stdout:
                    if verbose:
                        print(f"{GREEN}✓ Datasets are up to date{RESET}")
                else:
                    if verbose:
                        print(f"{GREEN}✓ Datasets updated{RESET}")
            else:
                if verbose:
                    print(f"{YELLOW}Warning: Failed to update datasets: {result.stderr}{RESET}")
        except Exception as e:
            if verbose:
                print(f"{YELLOW}Warning: Failed to update datasets: {e}{RESET}")

    return registry_path


def resolve_dataset_path(dataset_name: str, update_registry: bool = True) -> Optional[Path]:
    """Resolve a dataset name to its path in the registry.

    Args:
        dataset_name: Name of the dataset (e.g., "cua-bench-basic")
        update_registry: Whether to update registry before resolving (default: True)

    Returns:
        Path to the dataset directory, or None if not found
    """
    try:
        registry_path = ensure_registry(update=update_registry)
        dataset_path = registry_path / "datasets" / dataset_name

        if dataset_path.exists():
            return dataset_path
        return None
    except Exception as e:
        print(f"{RED}Error accessing datasets: {e}{RESET}")
        return None


def resolve_task_path(
    dataset_name: str, task_name: str, update_registry: bool = True
) -> Optional[Path]:
    """Resolve a task within a dataset to its path in the registry.

    Args:
        dataset_name: Name of the dataset (e.g., "cua-bench-basic")
        task_name: Name of the task within the dataset
        update_registry: Whether to update registry before resolving (default: True)

    Returns:
        Path to the task directory, or None if not found
    """
    dataset_path = resolve_dataset_path(dataset_name, update_registry=update_registry)
    if dataset_path is None:
        return None

    task_path = dataset_path / task_name
    if task_path.exists():
        return task_path
    return None
