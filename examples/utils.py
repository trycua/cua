"""Utility functions for example scripts."""

import os
import signal
import sys
from pathlib import Path
from typing import Optional


def load_env_file(path: Path) -> bool:
    """Load environment variables from a file.

    Args:
        path: Path to the .env file

    Returns:
        True if file was loaded successfully, False otherwise
    """
    if not path.exists():
        return False

    print(f"Loading environment from {path}")
    with open(path, "r") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue

            key, value = line.split("=", 1)
            os.environ[key] = value

    return True


def load_dotenv_files():
    """Load environment variables from .env files.

    Tries to load from .env.local first, then .env if .env.local doesn't exist.
    """
    # Get the project root directory (parent of the examples directory)
    project_root = Path(__file__).parent.parent

    # Try loading .env.local first, then .env if .env.local doesn't exist
    env_local_path = project_root / ".env.local"
    env_path = project_root / ".env"

    # Load .env.local if it exists, otherwise try .env
    if not load_env_file(env_local_path):
        load_env_file(env_path)


def handle_sigint(signum, frame):
    """Handle SIGINT (Ctrl+C) gracefully."""
    print("\nExiting gracefully...")
    sys.exit(0)
