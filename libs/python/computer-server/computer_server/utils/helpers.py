import asyncio
import os
import platform
import subprocess
from typing import overload


def get_current_os() -> str:
    """Determine the current OS.

    Returns:
        str: The OS type ('android', 'darwin' for macOS, 'linux' for Linux, or 'windows' for Windows)

    Raises:
        RuntimeError: If unable to determine the current OS
    """
    try:
        if os.environ.get("IS_CUA_ANDROID") == "true":
            # Verify emulator is actually running by checking adb devices
            try:
                result = subprocess.run(
                    ["adb", "devices"], capture_output=True, text=True, timeout=5
                )
                if result.returncode == 0 and "emulator-5554" in result.stdout:
                    return "android"
                else:
                    raise RuntimeError(
                        "IS_CUA_ANDROID is set but no emulator found. "
                        "Ensure Android emulator is running and accessible via adb."
                    )
            except subprocess.TimeoutExpired:
                raise RuntimeError(
                    "IS_CUA_ANDROID is set but adb command timed out. "
                    "Emulator may be starting up or unresponsive."
                )

        system = platform.system().lower()
        if system in ["darwin", "linux", "windows"]:
            return system

        # Fallback to uname if platform.system() doesn't return expected values (Unix-like systems only)
        result = subprocess.run(["uname", "-s"], capture_output=True, text=True)
        if result.returncode == 0:
            return result.stdout.strip().lower()

        raise RuntimeError(f"Unsupported OS: {system}")
    except Exception as e:
        raise RuntimeError(f"Failed to determine current OS: {str(e)}")


class CommandExecutor:
    def __init__(self, *base_cmd: str) -> None:
        """Initialize with a base command.

        Args:
            base_cmd: The base command and its initial arguments.
        """
        self.__base_cmd = list(base_cmd)

    @overload
    async def run(self, *args: str, timeout: int = 10) -> tuple[bool, bytes]: ...
    @overload
    async def run(self, *args: str, decode: bool = True, timeout: int = 10) -> tuple[bool, str]: ...

    async def run(
        self, *args: str, decode: bool = False, timeout: int = 10
    ) -> tuple[bool, bytes | str]:
        cmd = self.__base_cmd + list(args)
        try:
            result = await asyncio.create_subprocess_exec(
                *cmd, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
            )
            stdout, stderr = await asyncio.wait_for(result.communicate(), timeout=timeout)
            output = stdout or stderr
            if decode:
                output = output.decode("utf-8")
            return result.returncode == 0, output
        except asyncio.TimeoutError:
            return False, f"Command timed out after {timeout}s".encode("utf-8")
        except Exception as e:
            return False, str(e).encode("utf-8")
