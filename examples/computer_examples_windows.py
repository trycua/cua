import asyncio
import os
import sys
import traceback
from pathlib import Path

# Load environment variables from .env file
project_root = Path(__file__).parent.parent
env_file = project_root / ".env"
print(f"Loading environment from: {env_file}")
from computer.helpers import sandboxed
from dotenv import load_dotenv

load_dotenv(env_file)

# Add paths to sys.path if needed
pythonpath = os.environ.get("PYTHONPATH", "")
for path in pythonpath.split(":"):
    if path and path not in sys.path:
        sys.path.insert(0, path)  # Insert at beginning to prioritize
        print(f"Added to sys.path: {path}")

from computer.computer import Computer
from computer.logger import LogLevel
from computer.providers.base import VMProviderType

# ANSI color codes
RED = "\033[91m"
RESET = "\033[0m"


async def main():
    try:
        print("\n=== Using direct initialization ===")

        # Create a remote Windows computer with Cua
        computer = Computer(
            os_type="windows",
            api_key=os.getenv("CUA_API_KEY"),
            name=os.getenv("CONTAINER_NAME") or "",
            provider_type=VMProviderType.CLOUD,
        )

        try:
            # Run the computer with default parameters
            await computer.run()

            # Create output directory if it doesn't exist
            output_dir = Path("./output")
            output_dir.mkdir(exist_ok=True)

            # Keyboard Actions Examples
            print("\n=== Keyboard Actions ===")
            await computer.interface.type_text("Hello, World!")
            await computer.interface.press_key("enter")

            # Mouse Actions Examples
            print("\n=== Mouse Actions ===")
            await computer.interface.move_cursor(100, 100)
            await computer.interface.left_click()
            await computer.interface.double_click(400, 400)
            await computer.interface.right_click(300, 300)

            print("\n=== RPC ===")
            await computer.venv_install("demo_venv", ["mss"])

            @sandboxed("demo_venv")
            def greet_and_print(name):
                import os

                from mss import mss

                # get username
                username = os.getlogin()
                print(f"Hello from inside the container, {name}!")
                print("Username:", username)
                print("Screens:", mss().monitors)

                # take a screenshot
                with mss() as sct:
                    filename = sct.shot(mon=-1, output="C:/Users/azureuser/Desktop/fullscreen.png")
                    print(filename)

                return {"greeted": name, "username": username}

            # Call with args and kwargs
            result = await greet_and_print("John Doe")
            print("Result from sandboxed function:", result)

            # Command Actions Examples
            print("\n=== Command Actions ===")
            result = await computer.interface.run_command("notepad")
            print("Result from command:", result)

            screenshot = await computer.interface.screenshot()
            screenshot_path = output_dir / "screenshot.png"
            with open(screenshot_path, "wb") as f:
                f.write(screenshot)
            print(f"Screenshot saved to: {screenshot_path.absolute()}")

            # Clipboard Actions Examples
            print("\n=== Clipboard Actions ===")
            await computer.interface.set_clipboard("Test clipboard")
            content = await computer.interface.copy_to_clipboard()
            print(f"Clipboard content: {content}")

            # Simple REPL Loop
            print("\n=== Command REPL ===")
            print("Enter commands to run on the remote computer.")
            print("Type 'exit' or 'quit' to leave the REPL.\n")

            while True:
                try:
                    # Get command from user
                    command = input("command> ").strip()

                    # Check for exit commands
                    if command.lower() in ["exit", "quit", ""]:
                        if command.lower() in ["exit", "quit"]:
                            print("Exiting REPL...")
                        break

                    # Run the command
                    result = await computer.interface.run_command(command)

                    print(result.stdout)
                    if result.stderr:
                        print(f"{RED}{result.stderr}{RESET}")
                except KeyboardInterrupt:
                    print("\nExiting REPL...")
                    break
                except Exception as e:
                    print(f"{RED}Error running command: {e}{RESET}")

        finally:
            # Important to clean up resources
            # await computer.stop()
            pass
    except Exception as e:
        print(f"Error in main: {e}")
        traceback.print_exc()


if __name__ == "__main__":
    asyncio.run(main())
