#!/usr/bin/env python3
"""
Build script for CUA Desktop Extension (.mcpb file)

This script:
1. Creates a temporary build directory
2. Copies necessary files from mcp_server/ to the build directory
3. Copies manifest and other static files
4. Creates a .mcpb (zip) file
5. Cleans up the temporary directory

Usage:
    python build-extension.py
"""

import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path


def main():
    """Build the desktop extension."""
    # Get the script directory (libs/python/mcp-server)
    script_dir = Path(__file__).parent
    repo_root = script_dir.parent.parent.parent

    # Define paths
    output_dir = script_dir / "desktop-extension"
    output_file = output_dir / "cua-extension.mcpb"

    # Source directories
    mcp_server_dir = script_dir / "mcp_server"

    # Required files to copy
    files_to_copy = {
        "manifest.json": output_dir / "manifest.json",
        "desktop_extension.png": output_dir / "desktop_extension.png",
        "requirements.txt": output_dir / "requirements.txt",
        "run_server.sh": output_dir / "run_server.sh",
        "setup.py": output_dir / "setup.py",
    }

    # MCP server files to copy
    mcp_server_files = [
        "server.py",
        "session_manager.py",
    ]

    print("Building CUA Desktop Extension...")
    print(f"  Output: {output_file}")

    # Create temporary build directory
    with tempfile.TemporaryDirectory(prefix="cua-extension-build-") as build_dir:
        build_path = Path(build_dir)

        # Copy MCP server files
        print("  Copying MCP server files...")
        for filename in mcp_server_files:
            src = mcp_server_dir / filename
            dst = build_path / filename
            if src.exists():
                shutil.copy2(src, dst)
                print(f"    ✓ {filename}")
            else:
                print(f"    ✗ {filename} (not found)")
                sys.exit(1)

        # Copy static files from desktop-extension directory
        print("  Copying static files...")
        for src_name, src_path in files_to_copy.items():
            if src_path.exists():
                dst = build_path / src_name
                # Special handling for shell script - ensure executable
                shutil.copy2(src_path, dst)
                if src_name.endswith(".sh"):
                    os.chmod(dst, 0o755)
                print(f"    ✓ {src_name}")
            else:
                print(f"    ✗ {src_name} (not found)")
                sys.exit(1)

        # Validate manifest.json exists
        manifest_path = build_path / "manifest.json"
        if not manifest_path.exists():
            print("  ✗ manifest.json not found in build directory")
            sys.exit(1)

        # Create the .mcpb file (zip archive)
        print("  Creating .mcpb archive...")
        with zipfile.ZipFile(output_file, "w", zipfile.ZIP_DEFLATED) as zipf:
            # Add all files from build directory to the zip
            for root, dirs, files in os.walk(build_path):
                # Skip __pycache__ and other unwanted directories
                dirs[:] = [d for d in dirs if d not in ["__pycache__", ".git"]]

                for file in files:
                    file_path = Path(root) / file
                    # Use relative path from build directory as archive name
                    arcname = file_path.relative_to(build_path)
                    zipf.write(file_path, arcname)
                    print(f"    ✓ Added {arcname}")

        print(f"✓ Build complete: {output_file}")
        print(f"  Archive size: {output_file.stat().st_size / 1024:.1f} KB")

        # Set custom file icon based on platform
        icon_file = output_dir / "desktop_extension.png"
        if sys.platform == "darwin":
            _set_icon_macos(output_file, icon_file)
        elif sys.platform == "win32":
            _set_icon_windows(output_file, icon_file)
        elif sys.platform.startswith("linux"):
            _set_icon_linux(output_file, icon_file)


def _set_icon_macos(output_file: Path, icon_file: Path):
    """Set custom file icon on macOS."""
    try:
        # Check if fileicon is installed
        result = subprocess.run(["which", "fileicon"], capture_output=True, text=True)
        if result.returncode == 0:
            # Use the logo as the file icon
            if icon_file.exists():
                print("  Setting custom file icon (macOS)...")
                subprocess.run(
                    ["fileicon", "set", str(output_file), str(icon_file)],
                    check=False,
                    capture_output=True,
                )
                print("    ✓ File icon set")
            else:
                print(f"    ⚠ Icon file not found: {icon_file}")
        else:
            print("  ⚠ fileicon not installed (optional - for custom file icon)")
            print("    Install with: brew install fileicon")
    except Exception as e:
        print(f"  ⚠ Could not set file icon: {e}")


def _set_icon_windows(output_file: Path, icon_file: Path):
    """Set custom file icon on Windows."""
    try:
        # Windows uses a desktop.ini approach, which is complex
        # For simplicity, we'll skip this for now
        print("  ⚠ Custom file icons not supported on Windows yet")
    except Exception as e:
        print(f"  ⚠ Could not set file icon: {e}")


def _set_icon_linux(output_file: Path, icon_file: Path):
    """Set custom file icon on Linux."""
    try:
        # Linux uses .desktop files and thumbnail generation
        # This is complex and depends on the desktop environment
        print("  ⚠ Custom file icons not supported on Linux yet")
    except Exception as e:
        print(f"  ⚠ Could not set file icon: {e}")


if __name__ == "__main__":
    main()
