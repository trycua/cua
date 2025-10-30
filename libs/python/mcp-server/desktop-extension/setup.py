#!/usr/bin/env python3
"""
Setup script for CUA Desktop Extension
Installs required dependencies if not available
"""

import subprocess
import sys


def check_and_install_package(package_name, import_name=None):
    """Check if a package is available, install if not."""
    if import_name is None:
        import_name = package_name

    try:
        __import__(import_name)
        print(f"✓ {package_name} is available")
        return True
    except ImportError:
        print(f"⚠ {package_name} not found, installing...")
        try:
            subprocess.check_call([sys.executable, "-m", "pip", "install", package_name])
            print(f"✓ {package_name} installed successfully")
            return True
        except subprocess.CalledProcessError as e:
            print(f"✗ Failed to install {package_name}: {e}")
            return False


def main():
    """Install required packages."""
    print("Setting up CUA Desktop Extension dependencies...")

    # Required packages
    packages = [
        ("mcp", "mcp"),
        ("anyio", "anyio"),
        ("cua-agent[all]", "agent"),
        ("cua-computer", "computer"),
    ]

    all_installed = True
    for package, import_name in packages:
        if not check_and_install_package(package, import_name):
            all_installed = False

    if all_installed:
        print("✓ All dependencies are ready!")
        return 0
    else:
        print("✗ Some dependencies failed to install")
        return 1


if __name__ == "__main__":
    sys.exit(main())
