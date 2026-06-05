#!/usr/bin/env python3
"""Build script to download platform-specific cua-driver binary and create wheel.

This script:
1. Detects the current platform
2. Downloads the appropriate cua-driver-rs binary from GitHub releases
3. Places it in src/cua_driver/bin/
4. Builds the wheel with hatchling

Usage:
    python build_wheel.py [--version VERSION]
"""

import argparse
import hashlib
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import urllib.request
import zipfile
from pathlib import Path


def get_platform_info():
    """Determine platform and architecture for binary selection."""
    system = platform.system().lower()
    machine = platform.machine().lower()

    # Normalize architecture names
    if machine in ("x86_64", "amd64"):
        arch = "x86_64"
    elif machine in ("arm64", "aarch64"):
        arch = "arm64"
    else:
        raise ValueError(f"Unsupported architecture: {machine}")

    # Map to cua-driver-rs release naming
    if system == "darwin":
        # macOS uses universal binary
        return "darwin", "universal"
    elif system == "linux":
        return "linux", arch
    elif system == "windows":
        return "windows", arch
    else:
        raise ValueError(f"Unsupported platform: {system}")


def get_release_url(version: str, platform_name: str, arch: str) -> tuple[str, str]:
    """Get the GitHub release URL and binary name for the platform.

    Returns:
        Tuple of (download_url, binary_name_in_archive)
    """
    base_url = f"https://github.com/trycua/cua/releases/download/cua-driver-rs-v{version}"

    if platform_name == "darwin":
        # Universal binary tarball
        filename = f"cua-driver-rs-{version}-darwin-universal-binary.tar.gz"
        binary_name = "cua-driver"
    elif platform_name == "linux":
        filename = f"cua-driver-rs-{version}-linux-{arch}-binary.tar.gz"
        binary_name = "cua-driver"
    elif platform_name == "windows":
        filename = f"cua-driver-rs-{version}-windows-{arch}-binary.zip"
        binary_name = "cua-driver.exe"
    else:
        raise ValueError(f"Unknown platform: {platform_name}")

    return f"{base_url}/{filename}", binary_name


def download_file(url: str, dest: Path) -> None:
    """Download a file with progress indication."""
    print(f"Downloading {url}...")
    try:
        with urllib.request.urlopen(url) as response:
            total_size = int(response.headers.get("content-length", 0))
            dest.parent.mkdir(parents=True, exist_ok=True)

            with open(dest, "wb") as f:
                downloaded = 0
                block_size = 8192
                while True:
                    chunk = response.read(block_size)
                    if not chunk:
                        break
                    f.write(chunk)
                    downloaded += len(chunk)
                    if total_size > 0:
                        percent = (downloaded / total_size) * 100
                        print(f"  {percent:.1f}% ({downloaded}/{total_size} bytes)", end="\r")

        print(f"\nDownloaded to {dest}")
    except Exception as e:
        if dest.exists():
            dest.unlink()
        raise RuntimeError(f"Failed to download {url}: {e}")


def extract_binary(archive_path: Path, binary_name: str, dest_dir: Path) -> Path:
    """Extract the binary from the archive."""
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest_path = dest_dir / binary_name

    if archive_path.suffix == ".zip":
        with zipfile.ZipFile(archive_path, "r") as zf:
            # Find the binary in the zip
            for name in zf.namelist():
                if name.endswith(binary_name):
                    with zf.open(name) as src, open(dest_path, "wb") as dst:
                        shutil.copyfileobj(src, dst)
                    break
            else:
                raise ValueError(f"Binary {binary_name} not found in {archive_path}")
    else:
        # .tar.gz
        with tarfile.open(archive_path, "r:gz") as tf:
            # The -binary tarballs have the binary at the root
            tf.extract(binary_name, dest_dir)

    # Make executable on Unix
    if sys.platform != "win32":
        os.chmod(dest_path, 0o755)

    print(f"Extracted binary to {dest_path}")
    return dest_path


def build_wheel(package_dir: Path) -> None:
    """Build the wheel using hatchling."""
    print("\nBuilding wheel...")
    subprocess.run(
        [sys.executable, "-m", "build", "--wheel"],
        cwd=package_dir,
        check=True,
    )
    print("Wheel built successfully!")


def main():
    parser = argparse.ArgumentParser(description="Build cua-driver Python wheel with bundled binary")
    parser.add_argument(
        "--version",
        default="0.5.1",
        help="cua-driver-rs version to download (default: 0.5.1)",
    )
    parser.add_argument(
        "--skip-download",
        action="store_true",
        help="Skip download and use existing binary in bin/ (for local testing)",
    )
    args = parser.parse_args()

    # Determine paths
    script_dir = Path(__file__).parent
    bin_dir = script_dir / "src" / "cua_driver" / "bin"
    download_dir = script_dir / "downloads"

    if not args.skip_download:
        # Get platform info and download URL
        platform_name, arch = get_platform_info()
        print(f"Building for {platform_name}-{arch}")

        url, binary_name = get_release_url(args.version, platform_name, arch)
        archive_name = url.split("/")[-1]
        archive_path = download_dir / archive_name

        # Download the release archive
        if not archive_path.exists():
            download_file(url, archive_path)
        else:
            print(f"Using cached archive: {archive_path}")

        # Extract binary
        extract_binary(archive_path, binary_name, bin_dir)
    else:
        print("Skipping download (using existing binary)")

    # Verify binary exists
    expected_binary = "cua-driver.exe" if sys.platform == "win32" else "cua-driver"
    binary_path = bin_dir / expected_binary
    if not binary_path.exists():
        raise FileNotFoundError(
            f"Binary not found at {binary_path}. "
            f"Run without --skip-download or place binary manually."
        )

    print(f"Binary ready at: {binary_path}")
    print(f"Binary size: {binary_path.stat().st_size / 1024 / 1024:.2f} MB")

    # Build the wheel
    build_wheel(script_dir)


if __name__ == "__main__":
    main()
