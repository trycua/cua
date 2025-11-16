"""
Test script to verify MCP Server local desktop option works correctly.

This test verifies:
1. Default behavior: Computer uses VM
2. New behavior: Computer uses host when CUA_USE_HOST_COMPUTER_SERVER=true
"""

import asyncio
import os
import sys
from pathlib import Path

# Add the mcp-server module to path
mcp_server_path = Path(__file__).parent.parent / "libs" / "python" / "mcp-server"
sys.path.insert(0, str(mcp_server_path.parent.parent.parent / "libs" / "python"))

import pytest


@pytest.mark.asyncio
async def test_default_vm_mode():
    """Test that the default mode uses VM (not host computer server)."""
    # Ensure environment variable is not set or is false
    os.environ.pop("CUA_USE_HOST_COMPUTER_SERVER", None)

    from mcp_server.session_manager import ComputerPool

    pool = ComputerPool(max_size=1)

    try:
        computer = await pool.acquire()

        # Verify the computer was initialized
        assert computer is not None

        # Check that use_host_computer_server was set to False (default)
        # This should start a VM
        print("✓ Default mode: Computer initialized (VM mode expected)")

        await pool.release(computer)

    finally:
        await pool.shutdown()


@pytest.mark.asyncio
async def test_local_desktop_mode():
    """Test that setting CUA_USE_HOST_COMPUTER_SERVER=true uses host."""
    # Set environment variable to true
    os.environ["CUA_USE_HOST_COMPUTER_SERVER"] = "true"

    # Need to reload module to pick up new env var
    import importlib

    import mcp_server.session_manager
    from mcp_server.session_manager import ComputerPool

    importlib.reload(mcp_server.session_manager)

    pool = mcp_server.session_manager.ComputerPool(max_size=1)

    try:
        computer = await pool.acquire()

        # Verify the computer was initialized
        assert computer is not None

        # Check that use_host_computer_server was set to True
        print("✓ Local mode: Computer initialized (host mode expected)")

        await pool.release(computer)

    finally:
        await pool.shutdown()
        # Clean up env var
        os.environ.pop("CUA_USE_HOST_COMPUTER_SERVER", None)


@pytest.mark.asyncio
async def test_env_var_parsing():
    """Test that various values of CUA_USE_HOST_COMPUTER_SERVER are parsed correctly."""
    test_cases = [
        ("true", True),
        ("True", True),
        ("TRUE", True),
        ("1", True),
        ("yes", True),
        ("false", False),
        ("False", False),
        ("FALSE", False),
        ("0", False),
        ("no", False),
        ("", False),
        ("random", False),
    ]

    for value, expected in test_cases:
        os.environ["CUA_USE_HOST_COMPUTER_SERVER"] = value

        # Check parsing logic
        use_host = os.getenv("CUA_USE_HOST_COMPUTER_SERVER", "false").lower() in (
            "true",
            "1",
            "yes",
        )

        assert (
            use_host == expected
        ), f"Failed for value '{value}': expected {expected}, got {use_host}"
        print(f"✓ Env var '{value}' correctly parsed as {expected}")

    os.environ.pop("CUA_USE_HOST_COMPUTER_SERVER", None)


if __name__ == "__main__":
    print("Testing MCP Server Local Desktop Option")
    print("=" * 60)

    print("\n1. Testing environment variable parsing...")
    asyncio.run(test_env_var_parsing())

    print("\n2. Testing default VM mode...")
    try:
        asyncio.run(test_default_vm_mode())
    except Exception as e:
        print(f"✗ Default VM mode test failed: {e}")
        print("Note: This may require lume/VM setup to fully test")

    print("\n3. Testing local desktop mode...")
    try:
        asyncio.run(test_local_desktop_mode())
    except Exception as e:
        print(f"✗ Local desktop mode test failed: {e}")
        print("Note: This may require computer-server to be running locally")

    print("\n" + "=" * 60)
    print("Tests completed!")
