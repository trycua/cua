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
        "logo_black.png": output_dir / "logo_black.png",
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


if __name__ == "__main__":
    main()
