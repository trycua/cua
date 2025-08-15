"""
Shell Command Tests (Bash)
Tests for the run_command method of the Computer interface using bash commands.
Required environment variables:
- CUA_API_KEY: API key for Cua cloud provider
- CUA_CONTAINER_NAME: Name of the container to use
"""

import os
import asyncio
import pytest
from pathlib import Path
import sys
import traceback

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
    # Create a remote Linux computer with Cua
    computer = Computer(
        os_type="linux",
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
async def test_bash_echo_command(computer):
    """Test basic echo command with bash."""
    result = await computer.interface.run_command("echo 'Hello World'")
    
    assert result.stdout.strip() == "Hello World"
    assert result.stderr == ""
    assert result.returncode == 0


@pytest.mark.asyncio(loop_scope="session")
async def test_bash_ls_command(computer):
    """Test ls command to list directory contents."""
    result = await computer.interface.run_command("ls -la /tmp")
    
    assert result.returncode == 0
    assert result.stderr == ""
    assert "total" in result.stdout  # ls -la typically starts with "total"
    assert "." in result.stdout      # Current directory entry
    assert ".." in result.stdout     # Parent directory entry


@pytest.mark.asyncio(loop_scope="session")
async def test_bash_command_with_error(computer):
    """Test command that produces an error."""
    result = await computer.interface.run_command("ls /nonexistent_directory_12345")
    
    assert result.returncode != 0
    assert result.stdout == ""
    assert "No such file or directory" in result.stderr or "cannot access" in result.stderr


if __name__ == "__main__":
    # Run tests directly
    pytest.main([__file__, "-v"])
