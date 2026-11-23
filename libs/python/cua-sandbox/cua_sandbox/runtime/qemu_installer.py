"""Auto-download and manage QEMU portable + wimlib for Windows."""

from __future__ import annotations

import hashlib
import logging
import os
import platform
import shutil
import tempfile
import zipfile
from pathlib import Path

import httpx
from cua_sandbox._paths import cua_home

logger = logging.getLogger(__name__)

QEMU_PORTABLE_URL = (
    "https://github.com/ganarcasas/qemu-portable/releases/download/20241220/"
    "qemu-portable-20241220.zip"
)
QEMU_DIR = cua_home() / "cua-sandbox" / "qemu"
QEMU_INNER_DIR = "qemu-portable-20241220"
QEMU_PORTABLE_SHA256 = "6ec04bc4a57d802734c5a1ca744d2c7348fe3399d4a6c5a69500afb8be144388"

WIMLIB_URL = "https://wimlib.net/downloads/wimlib-1.14.5-windows-x86_64-bin.zip"
WIMLIB_SHA256 = "2f446d6fa3866582175f1a22a7be198eeee0aec7aba5b4e04ad25c99eae2d265"
WIMLIB_DIR = cua_home() / "cua-sandbox" / "wimlib"


def qemu_bin(arch: str = "x86_64") -> str:
    """Return the path to qemu-system-{arch}, installing if needed on Windows.

    Resolution order:
      1. Already on PATH → use it
      2. Common macOS Homebrew/MacPorts install locations (may not be on PATH)
      3. Already downloaded to ~/.cua/cua-sandbox/qemu/ → use it
      4. Windows only: download portable zip → extract → use it
      5. Raise RuntimeError with install instructions
    """
    binary = f"qemu-system-{arch}"

    # 1. On PATH
    found = shutil.which(binary)
    if found:
        return found

    # 2. Common macOS install locations not always on PATH in subprocess envs
    if platform.system() == "Darwin":
        for prefix in (
            "/opt/homebrew/bin",  # Homebrew on Apple Silicon
            "/usr/local/bin",  # Homebrew on Intel
            "/opt/local/bin",  # MacPorts
        ):
            candidate = os.path.join(prefix, binary)
            if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
                return candidate

    # 3. Portable install
    exe_name = f"{binary}.exe" if platform.system() == "Windows" else binary
    local_bin = QEMU_DIR / QEMU_INNER_DIR / exe_name
    if local_bin.exists():
        return str(local_bin)

    # 4. Auto-download (Windows only)
    if platform.system() == "Windows":
        _download_portable()
        if local_bin.exists():
            return str(local_bin)

    if platform.system() == "Darwin":
        raise RuntimeError(
            f"{binary} not found. Install QEMU via Homebrew or MacPorts:\n\n"
            f"  brew install qemu\n"
            f"       — or —\n"
            f"  sudo port install qemu\n"
        )

    raise RuntimeError(
        f"{binary} not found. Install QEMU or, on Windows, let cua-sandbox "
        f"download it automatically (failed to find {local_bin})."
    )


def wimlib_imagex() -> str:
    """Return path to wimlib-imagex, installing if needed on Windows.

    Resolution order:
      1. Already on PATH → use it
      2. Already downloaded to ~/.cua/cua-sandbox/wimlib/ → use it
      3. Windows only: download zip → extract → use it
      4. Raise RuntimeError
    """
    # 1. On PATH
    found = shutil.which("wimlib-imagex")
    if found:
        return found

    # 2. Local install
    local_bin = WIMLIB_DIR / "wimlib-imagex.exe"
    if local_bin.exists():
        return str(local_bin)

    # 3. Auto-download (Windows only)
    if platform.system() == "Windows":
        _download_wimlib()
        if local_bin.exists():
            return str(local_bin)

    raise RuntimeError(
        "wimlib-imagex not found. Install wimlib or, on Windows, let cua-sandbox "
        "download it automatically."
    )


def _download_zip(url: str, dest_dir: Path, description: str, sha256: str) -> None:
    """Download a zip, check its SHA-256 and extract it to dest_dir.

    The archive is staged in a fresh private directory (mode 0700, random name)
    next to dest_dir, inside the user's own cua home, and opened with exclusive
    create. It never goes through a predictable path or the shared system temp
    directory, so another local user cannot redirect the write through a planted
    link or swap the archive before it is checked and extracted.
    """
    dest_dir.mkdir(parents=True, exist_ok=True)

    logger.info(f"Downloading {description} from {url} ...")
    with tempfile.TemporaryDirectory(prefix=".download-", dir=dest_dir.parent) as staging:
        archive = Path(staging) / "download.zip"
        digest = hashlib.sha256()
        with httpx.Client(follow_redirects=True, timeout=300) as client:
            with client.stream("GET", url) as resp:
                resp.raise_for_status()
                total = int(resp.headers.get("content-length", 0))
                downloaded = 0
                with open(archive, "xb") as f:
                    for chunk in resp.iter_bytes(chunk_size=1024 * 256):
                        f.write(chunk)
                        digest.update(chunk)
                        downloaded += len(chunk)
                        if total:
                            pct = downloaded * 100 // total
                            print(f"\r  downloading {description} … {pct}%", end="", flush=True)
                print()

        actual = digest.hexdigest()
        if actual != sha256:
            raise RuntimeError(
                f"{description} download from {url} failed its integrity check "
                f"(expected SHA-256 {sha256}, got {actual}); nothing was installed."
            )

        logger.info(f"Extracting to {dest_dir} ...")
        with zipfile.ZipFile(archive) as zf:
            zf.extractall(dest_dir)

    logger.info(f"{description} installed successfully")


def _download_portable() -> None:
    """Download and extract QEMU portable to ~/.cua/cua-sandbox/qemu/."""
    marker = QEMU_DIR / QEMU_INNER_DIR / "qemu-system-x86_64.exe"
    if marker.exists():
        return
    _download_zip(QEMU_PORTABLE_URL, QEMU_DIR, "QEMU portable", QEMU_PORTABLE_SHA256)


def _download_wimlib() -> None:
    """Download and extract wimlib to ~/.cua/cua-sandbox/wimlib/."""
    marker = WIMLIB_DIR / "wimlib-imagex.exe"
    if marker.exists():
        return
    _download_zip(WIMLIB_URL, WIMLIB_DIR, "wimlib", WIMLIB_SHA256)
