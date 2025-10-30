"""
Shell Command Tests (CMD)
Tests for the run_command method of the Computer interface using cmd.exe commands.
Required environment variables:
- CUA_API_KEY: API key for Cua cloud provider
- CUA_CONTAINER_NAME: Name of the container to use
"""

import asyncio
import os
import sys
import traceback
from pathlib import Path

import pytest

# Load environment variables from .env file
project_root = Path(__file__).parent.parent
env_file = project_root / ".env"
print(f"Loading environment from: {env_file}")
from dotenv import load_dotenv

load_dotenv(env_file)

# Add paths to sys.path if needed
pythonpath = os.environ.get("PYTHONPATH", "")
for path in pythonpath.split(":"):
    if path and path not in sys.path:
        sys.path.insert(0, path)  # Insert at beginning to prioritize
        print(f"Added to sys.path: {path}")

from computer import Computer, VMProviderType


@pytest.fixture(scope="session")
async def computer():
    """Shared Computer instance for all test cases."""
    # Create a remote Windows computer with Cua
    computer = Computer(
        os_type="windows",
        api_key=os.getenv("CUA_API_KEY"),
        name=str(os.getenv("CUA_CONTAINER_NAME")),
        provider_type=VMProviderType.CLOUD,
    )

    try:
        await computer.run()
        yield computer
    finally:
        await computer.disconnect()


# Sample test cases
@pytest.mark.asyncio(loop_scope="session")
async def test_cmd_echo_command(computer):
    """Test basic echo command with cmd.exe."""
    result = await computer.interface.run_command("echo Hello World")

    assert result.stdout.strip() == "Hello World"
    assert result.stderr == ""
    assert result.returncode == 0


@pytest.mark.asyncio(loop_scope="session")
async def test_cmd_dir_command(computer):
    """Test dir command to list directory contents."""
    result = await computer.interface.run_command("dir C:\\")

    assert result.returncode == 0
    assert result.stderr == ""
    assert "Directory of C:\\" in result.stdout
    assert "bytes" in result.stdout.lower()  # dir typically shows file sizes


@pytest.mark.asyncio(loop_scope="session")
async def test_cmd_command_with_error(computer):
    """Test command that produces an error."""
    result = await computer.interface.run_command("dir C:\\nonexistent_directory_12345")

    assert result.returncode != 0
    assert result.stdout == ""
    assert (
        "File Not Found" in result.stderr
        or "cannot find the path" in result.stderr
        or "The system cannot find" in result.stderr
    )


if __name__ == "__main__":
    # Run tests directly
    pytest.main([__file__, "-v"])
