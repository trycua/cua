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


def get_platform_info(arch_override: str = None):
    """Determine platform and architecture for binary selection.

    Args:
        arch_override: Optional architecture override (e.g., 'arm64', 'x86_64', 'universal')
    """
    system = platform.system().lower()

    if arch_override:
        arch = arch_override
    else:
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


def get_release_url(version: str, platform_name: str, arch: str) -> tuple[str, list[str]]:
    """Get the GitHub release URL and binary names for the platform.

    Returns:
        Tuple of (download_url, list_of_binary_names_in_archive)
    """
    base_url = f"https://github.com/trycua/cua/releases/download/cua-driver-rs-v{version}"

    if platform_name == "darwin":
        # Universal binary tarball
        filename = f"cua-driver-rs-{version}-darwin-universal-binary.tar.gz"
        binary_names = ["cua-driver"]
    elif platform_name == "linux":
        filename = f"cua-driver-rs-{version}-linux-{arch}-binary.tar.gz"
        binary_names = ["cua-driver"]
    elif platform_name == "windows":
        filename = f"cua-driver-rs-{version}-windows-{arch}-binary.zip"
        # Windows includes both main executable and UIAccess worker
        binary_names = ["cua-driver.exe", "cua-driver-uia.exe"]
    else:
        raise ValueError(f"Unknown platform: {platform_name}")

    return f"{base_url}/{filename}", binary_names


def verify_sha256(file_path: Path, expected_sha256: str) -> None:
    """Verify file matches expected SHA256 hash."""
    h = hashlib.sha256()
    with open(file_path, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            h.update(chunk)
    actual = h.hexdigest()
    if actual != expected_sha256:
        raise ValueError(
            f"SHA256 mismatch for {file_path.name}: expected {expected_sha256}, got {actual}"
        )


def get_expected_sha256(version: str, archive_name: str) -> str:
    """Fetch and parse checksums.txt from GitHub release."""
    checksums_url = f"https://github.com/trycua/cua/releases/download/cua-driver-rs-v{version}/checksums.txt"
    print(f"Fetching checksums from {checksums_url}...")

    try:
        with urllib.request.urlopen(checksums_url) as response:
            content = response.read().decode("utf-8")

        for line in content.splitlines():
            line = line.strip()
            # Skip empty lines, comments, and headers
            if not line or line.startswith("#") or "Checksums" in line or line.startswith("```"):
                continue
            # Parse "SHA256  filename" format
            parts = line.split()
            if len(parts) >= 2:
                sha, name = parts[0], parts[-1]
                if name == archive_name:
                    return sha

        raise ValueError(f"SHA256 for {archive_name} not found in checksums.txt")
    except Exception as e:
        raise RuntimeError(f"Failed to fetch or parse checksums: {e}")


def download_file(url: str, dest: Path, expected_sha256: str) -> None:
    """Download a file with progress indication and SHA256 verification."""
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

        # Verify SHA256
        print("Verifying SHA256 checksum...")
        verify_sha256(dest, expected_sha256)
        print("✓ Checksum verified")

    except Exception as e:
        if dest.exists():
            dest.unlink()
        raise RuntimeError(f"Failed to download {url}: {e}")


def extract_binaries(archive_path: Path, binary_names: list[str], dest_dir: Path) -> list[Path]:
    """Extract the binaries from the archive.

    Args:
        archive_path: Path to the archive file
        binary_names: List of binary names to extract
        dest_dir: Destination directory

    Returns:
        List of paths to extracted binaries
    """
    dest_dir.mkdir(parents=True, exist_ok=True)
    extracted_paths = []

    if archive_path.suffix == ".zip":
        with zipfile.ZipFile(archive_path, "r") as zf:
            for binary_name in binary_names:
                dest_path = dest_dir / binary_name
                # Find the binary in the zip (exact match or ends with /<binary_name>)
                for name in zf.namelist():
                    # Match exact name or path ending with /binary_name
                    if name == binary_name or name.endswith(f"/{binary_name}"):
                        with zf.open(name) as src, open(dest_path, "wb") as dst:
                            shutil.copyfileobj(src, dst)
                        extracted_paths.append(dest_path)
                        print(f"Extracted binary to {dest_path}")
                        break
                else:
                    raise ValueError(f"Binary {binary_name} not found in {archive_path}")
    else:
        # .tar.gz
        with tarfile.open(archive_path, "r:gz") as tf:
            for binary_name in binary_names:
                # The -binary tarballs have the binary at the root
                tf.extract(binary_name, dest_dir)
                dest_path = dest_dir / binary_name
                extracted_paths.append(dest_path)
                print(f"Extracted binary to {dest_path}")

    # Make executable on Unix
    if sys.platform != "win32":
        for path in extracted_paths:
            os.chmod(path, 0o755)

    return extracted_paths


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
        "--arch",
        help="Architecture override (e.g., 'arm64', 'x86_64', 'universal')",
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
        platform_name, arch = get_platform_info(args.arch)
        print(f"Building for {platform_name}-{arch}")

        url, binary_names = get_release_url(args.version, platform_name, arch)
        archive_name = url.split("/")[-1]
        archive_path = download_dir / archive_name

        # Get expected SHA256 from checksums.txt
        expected_sha256 = get_expected_sha256(args.version, archive_name)

        # Download the release archive (or verify cached)
        if not archive_path.exists():
            download_file(url, archive_path, expected_sha256)
        else:
            print(f"Using cached archive: {archive_path}")
            # Verify cached archive too
            print("Verifying cached archive SHA256...")
            verify_sha256(archive_path, expected_sha256)
            print("✓ Cached archive checksum verified")

        # Extract binaries
        extract_binaries(archive_path, binary_names, bin_dir)
    else:
        print("Skipping download (using existing binary)")

    # Verify main binary exists
    expected_binary = "cua-driver.exe" if sys.platform == "win32" else "cua-driver"
    binary_path = bin_dir / expected_binary
    if not binary_path.exists():
        raise FileNotFoundError(
            f"Binary not found at {binary_path}. "
            f"Run without --skip-download or place binary manually."
        )

    print(f"\nBinary ready at: {binary_path}")
    print(f"Binary size: {binary_path.stat().st_size / 1024 / 1024:.2f} MB")

    # List all binaries in bin directory
    print(f"\nAll binaries in {bin_dir}:")
    for binary in bin_dir.iterdir():
        if binary.is_file():
            print(f"  - {binary.name} ({binary.stat().st_size / 1024 / 1024:.2f} MB)")

    # Build the wheel
    build_wheel(script_dir)


if __name__ == "__main__":
    main()
