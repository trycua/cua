#!/usr/bin/env python3
"""
Test script to verify local desktop MCP setup

This script tests that all components are properly installed and configured.
"""

import sys
import platform
import subprocess


def test_python_version():
    """Test Python version is 3.12+"""
    print("Testing Python version...", end=" ")
    version_info = sys.version_info
    if version_info.major >= 3 and version_info.minor >= 12:
        print(f"✓ Python {version_info.major}.{version_info.minor}.{version_info.micro}")
        return True
    else:
        print(f"✗ Python {version_info.major}.{version_info.minor} (need 3.12+)")
        return False


def test_import(module_name, package_name=None):
    """Test if a module can be imported"""
    display_name = package_name or module_name
    print(f"Testing {display_name}...", end=" ")
    try:
        __import__(module_name)
        print("✓")
        return True
    except ImportError as e:
        print(f"✗ ({e})")
        return False


def test_os_detection():
    """Test OS detection"""
    print("Testing OS detection...", end=" ")
    system = platform.system()
    if system == "Darwin":
        os_type = "macOS"
    elif system == "Linux":
        os_type = "Linux"
    elif system == "Windows":
        os_type = "Windows"
    else:
        os_type = "Unknown"
    print(f"✓ {os_type}")
    return True


def test_computer_server_import():
    """Test computer server import"""
    print("Testing computer_server module...", end=" ")
    try:
        import computer_server
        print("✓")
        return True
    except ImportError as e:
        print(f"✗ ({e})")
        print("  Hint: pip install -e libs/python/computer-server")
        return False


def main():
    """Run all tests"""
    print("=" * 60)
    print("Local Desktop MCP Setup Test")
    print("=" * 60)
    print()

    results = []

    # Test Python version
    results.append(test_python_version())

    # Test imports
    results.append(test_import("mcp", "mcp[cli]"))
    results.append(test_import("computer", "cua-computer"))
    results.append(test_computer_server_import())
    results.append(test_import("core", "cua-core"))
    results.append(test_import("PIL", "Pillow"))
    results.append(test_import("asyncio"))

    # Test OS detection
    results.append(test_os_detection())

    print()
    print("=" * 60)

    if all(results):
        print("✅ All tests passed! Setup is complete.")
        print()
        print("Next steps:")
        print("1. Start computer server: ./start-server.sh")
        print("2. Configure Claude Code (see QUICKSTART.md)")
        print("3. Run claude-code and start controlling your PC!")
        return 0
    else:
        print("❌ Some tests failed. Please check the errors above.")
        print()
        print("To fix:")
        print("- Run: ./setup.sh")
        print("- Check: README.md for troubleshooting")
        return 1


if __name__ == "__main__":
    sys.exit(main())
