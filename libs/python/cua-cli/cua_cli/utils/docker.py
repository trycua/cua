"""Docker utilities for CUA CLI."""

import platform
import shutil
import subprocess
from pathlib import Path


def find_free_port(start: int = 5000, end: int = 9000) -> int:
    """Find a free port in the given range.

    Args:
        start: Start of port range (inclusive)
        end: End of port range (exclusive)

    Returns:
        First available port in the range

    Raises:
        RuntimeError: If no free port found
    """
    import socket

    for port in range(start, end):
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.bind(("", port))
                return port
        except OSError:
            continue
    raise RuntimeError(f"No free port found in range {start}-{end}")


def allocate_ports(vnc_default: int = 8006, api_default: int = 5000) -> tuple[int, int]:
    """Allocate VNC and API ports, auto-selecting if defaults are in use.

    Args:
        vnc_default: Preferred VNC port
        api_default: Preferred API port

    Returns:
        Tuple of (vnc_port, api_port)
    """
    import socket

    def is_port_free(port: int) -> bool:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
                s.bind(("", port))
                return True
        except OSError:
            return False

    vnc_port = vnc_default if is_port_free(vnc_default) else find_free_port(8000, 9000)
    api_port = api_default if is_port_free(api_default) else find_free_port(5000, 6000)

    return vnc_port, api_port


def create_overlay_copy(golden_path: Path, overlay_path: Path, verbose: bool = False) -> None:
    """Copy golden image to overlay directory for COW-like behavior.

    This is a temporary workaround until proper QEMU overlay support is added.
    Uses native `cp -a` on Unix for speed (5x faster than Python shutil).
    Falls back to shutil.copytree on Windows.

    WIP: https://github.com/trycua/cua/issues/699

    Args:
        golden_path: Path to golden image directory
        overlay_path: Path to overlay directory (will be created/cleaned)
        verbose: Print progress messages

    Raises:
        RuntimeError: If copy fails
    """
    if overlay_path.exists():
        shutil.rmtree(overlay_path)
    overlay_path.mkdir(parents=True, exist_ok=True)

    if verbose:
        print(f"   Source:  {golden_path}")
        print(f"   Overlay: {overlay_path}")
        print("   (This may take a while for large images)")
        print("   WIP: https://github.com/trycua/cua/issues/699")

    if platform.system() == "Windows":
        try:
            for item in golden_path.iterdir():
                src = golden_path / item.name
                dst = overlay_path / item.name
                if src.is_dir():
                    shutil.copytree(src, dst)
                else:
                    shutil.copy2(src, dst)
        except Exception as e:
            raise RuntimeError(f"Failed to create overlay: {e}")
    else:
        result = subprocess.run(
            ["cp", "-a", f"{golden_path}/.", str(overlay_path)], capture_output=True, text=True
        )
        if result.returncode != 0:
            raise RuntimeError(f"Failed to create overlay: {result.stderr or 'cp failed'}")
