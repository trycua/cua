"""Example of using the Windows Sandbox computer provider.

Learn more at: https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/
"""

import asyncio

from computer import Computer


async def main():
    """Test the Windows Sandbox provider."""

    # Create a computer instance using Windows Sandbox
    computer = Computer(
        provider_type="winsandbox",
        os_type="windows",
        memory="4GB",
        # ephemeral=True,  # Always true for Windows Sandbox
    )

    try:
        print("Starting Windows Sandbox...")
        await computer.run()

        print("Windows Sandbox is ready!")
        print(f"IP Address: {await computer.get_ip()}")

        # Test basic functionality
        print("Testing basic functionality...")
        screenshot = await computer.interface.screenshot()
        print(f"Screenshot taken: {len(screenshot)} bytes")

        # Test running a command
        print("Testing command execution...")
        result = await computer.interface.run_command("echo Hello from Windows Sandbox!")
        print(f"Command output: {result.stdout}")

        print("Press any key to continue...")
        input()

    except Exception as e:
        print(f"Error: {e}")
        import traceback

        traceback.print_exc()

    finally:
        print("Stopping Windows Sandbox...")
        await computer.stop()
        print("Windows Sandbox stopped.")


if __name__ == "__main__":
    asyncio.run(main())
