"""QEMU qcow2 overlay (backing file) management.

Provides the 3-layer chain:
  Layer 0 (base):    windows-11-base.qcow2     — unattend + computer-server
  Layer 1 (user):    {hash}.qcow2              — user's .winget_install()/.run() etc
  Layer 2 (session): session-{uuid}.qcow2       — ephemeral sandbox runtime
"""

from __future__ import annotations

import hashlib
import json
import logging
import subprocess
from pathlib import Path

logger = logging.getLogger(__name__)

IMAGES_DIR = Path.home() / ".cua" / "cua-sandbox" / "images"


def _qemu_img() -> str:
    """Resolve qemu-img binary."""
    import shutil

    found = shutil.which("qemu-img")
    if found:
        return found
    from cua_sandbox.runtime.qemu_installer import qemu_bin

    parent = Path(qemu_bin("x86_64")).parent
    for name in ["qemu-img", "qemu-img.exe"]:
        p = parent / name
        if p.exists():
            return str(p)
    raise RuntimeError("qemu-img not found")


def create_overlay(backing: Path, overlay: Path) -> Path:
    """Create a qcow2 overlay with the given backing file."""
    overlay.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        _qemu_img(),
        "create",
        "-q",
        "-f",
        "qcow2",
        "-b",
        str(backing),
        "-F",
        "qcow2",
        str(overlay),
    ]
    subprocess.run(cmd, check=True, capture_output=True)
    logger.info(f"Created overlay: {overlay} (backing: {backing})")
    return overlay


def commit_overlay(overlay: Path) -> None:
    """Commit an overlay's changes into its backing file (flattens one level)."""
    cmd = [_qemu_img(), "commit", "-q", str(overlay)]
    subprocess.run(cmd, check=True, capture_output=True)
    logger.info(f"Committed overlay: {overlay}")


def rebase_standalone(disk: Path) -> None:
    """Remove backing file reference, making the disk standalone."""
    cmd = [_qemu_img(), "rebase", "-u", "-b", "", str(disk)]
    subprocess.run(cmd, check=True, capture_output=True)
    logger.info(f"Rebased standalone: {disk}")


def layers_hash(layers: list[dict]) -> str:
    """Compute a stable hash for a list of Image layers."""
    raw = json.dumps(layers, sort_keys=True).encode()
    return hashlib.sha256(raw).hexdigest()[:16]


def base_image_path(os_type: str, version: str) -> Path:
    """Path to the cached base image (OS + computer-server installed)."""
    return IMAGES_DIR / f"{os_type}-{version}-base" / "disk.qcow2"


def user_image_path(os_type: str, version: str, layer_hash: str) -> Path:
    """Path to a cached user layer image."""
    return IMAGES_DIR / f"{os_type}-{version}-{layer_hash}" / "disk.qcow2"


def session_overlay_path(name: str) -> Path:
    """Path to an ephemeral session overlay."""
    return IMAGES_DIR / "sessions" / f"{name}.qcow2"
