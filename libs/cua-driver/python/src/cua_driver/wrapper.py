"""Subprocess wrapper for cua-driver binary with stdio passthrough."""

import os
import subprocess
import sys
from pathlib import Path
from typing import IO, Any, Optional


def _stdio_with_fileno(stream: Optional[IO[Any]]) -> Optional[IO[Any]]:
    """Keep a Python stream only when subprocess can use its descriptor."""
    if stream is None:
        return None
    try:
        descriptor = stream.fileno()
        if not isinstance(descriptor, int) or descriptor < 0:
            return None
        os.fstat(descriptor)
    except (AttributeError, OSError, OverflowError, TypeError, ValueError):
        return None
    return stream


def _fetch_module() -> Any:
    """The `fetch` module next to this one (also when this file is loaded alone)."""
    try:
        from . import fetch

        return fetch
    except ImportError:
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            "cua_driver_fetch", Path(__file__).parent / "fetch.py"
        )
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module


def get_binary_path() -> Path:
    """Get the path to the cua-driver executable.

    Order: ``$CUA_DRIVER_BINARY``; an executable bundled in the package
    (local builds); otherwise the verified download of this release, fetched on
    first use (see ``cua_driver.fetch``).

    Returns:
        Path to the cua-driver executable.

    Raises:
        FileNotFoundError: If the executable is not bundled and cannot be fetched.
    """
    package_dir = Path(__file__).parent
    binary_name = "cua-driver.exe" if sys.platform == "win32" else "cua-driver"

    override = os.environ.get("CUA_DRIVER_BINARY")
    bundled = package_dir / "bin" / binary_name
    if override:
        binary_path = Path(override)
    elif bundled.exists() or not (package_dir / "_release.json").is_file():
        binary_path = bundled
    else:
        fetch = _fetch_module()
        try:
            binary_path = fetch.binary()
        except fetch.FetchError as e:
            raise FileNotFoundError(str(e)) from e

    if not binary_path.exists():
        raise FileNotFoundError(
            f"cua-driver executable not found at {binary_path}. "
            f"This package may not have been built correctly for {sys.platform}."
        )

    if sys.platform != "win32" and not os.access(binary_path, os.X_OK):
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

    # Let the binary's consent-aware first-run registration distinguish a
    # bundled Python installation from an installer-script installation. Do
    # not overwrite an explicit bounded channel inherited from `update` or a
    # test harness. The Rust binary owns validation, consent, identity, and
    # per-version deduplication.
    child_env = os.environ.copy()
    child_env.setdefault("CUA_DRIVER_INSTALL_CHANNEL", "python_package")

    try:
        # Run with direct stdio inheritance - no buffering, no capturing
        result = subprocess.run(
            [str(binary_path), *args],
            stdin=_stdio_with_fileno(sys.stdin),
            stdout=_stdio_with_fileno(sys.stdout),
            stderr=_stdio_with_fileno(sys.stderr),
            env=child_env,
        )
        return result.returncode
    except KeyboardInterrupt:
        # Standard SIGINT exit code
        return 130
    except Exception as e:
        print(f"Error executing cua-driver: {e}", file=sys.stderr)
        return 1
