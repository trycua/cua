"""Subprocess wrapper for cua-driver binary with stdio passthrough."""

import os
import sys
import subprocess
from pathlib import Path
from typing import Optional


def get_binary_path() -> Path:
    """Get the path to the bundled cua-driver binary.

    Returns:
        Path to the cua-driver executable.

    Raises:
        FileNotFoundError: If the binary is not found in the package.
    """
    # Binary is bundled in the package at: cua_driver/bin/cua-driver[.exe]
    package_dir = Path(__file__).parent

    if sys.platform == "win32":
        binary_name = "cua-driver.exe"
    else:
        binary_name = "cua-driver"

    binary_path = package_dir / "bin" / binary_name

    if not binary_path.exists():
        raise FileNotFoundError(
            f"cua-driver binary not found at {binary_path}. "
            f"This package may not have been built correctly for {sys.platform}."
        )

    # Ensure binary is executable on Unix
    if sys.platform != "win32":
        os.chmod(binary_path, 0o755)

    return binary_path


def run_cua_driver(args: Optional[list[str]] = None) -> int:
    """Execute cua-driver binary with stdio passthrough.

    Args:
        args: Command-line arguments to pass to cua-driver.
              If None, uses sys.argv[1:].

    Returns:
        Exit code from the cua-driver process.
    """
    if args is None:
        args = sys.argv[1:]

    binary_path = get_binary_path()

    try:
        # Run with direct stdio inheritance - no buffering, no capturing
        result = subprocess.run(
            [str(binary_path), *args],
            stdin=sys.stdin,
            stdout=sys.stdout,
            stderr=sys.stderr,
        )
        return result.returncode
    except KeyboardInterrupt:
        # Standard SIGINT exit code
        return 130
    except Exception as e:
        print(f"Error executing cua-driver: {e}", file=sys.stderr)
        return 1
