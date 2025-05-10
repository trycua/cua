import os
import asyncio
from pathlib import Path
import sys
import json
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
        sys.path.append(path)
        print(f"Added to sys.path: {path}")

from computer import Computer, VMProviderType
from computer.logger import LogLevel
from computer.utils import get_image_size


async def main():
    try:
        print("\n=== Using direct initialization ===")

        # Create computer with configured host
        computer = Computer(
            display="1024x768",  # Higher resolution
            memory="8GB",  # More memory
            cpu="4",  # More CPU cores
            os_type="macos",
            verbosity=LogLevel.NORMAL,  # Use QUIET to suppress most logs
            use_host_computer_server=False,
            provider_type=VMProviderType.LUME,  # Explicitly use the Lume provider
        )
        try:
            await computer.run()

            await computer.interface.hotkey("command", "space")

            # res = await computer.interface.run_command("touch ./Downloads/empty_file")
            # print(f"Run command result: {res}")

            accessibility_tree = await computer.interface.get_accessibility_tree()
            print(f"Accessibility tree: {accessibility_tree}")

            # Screen Actions Examples
            # print("\n===  Screen Actions ===")
            # screenshot = await computer.interface.screenshot()
            # with open("screenshot_direct.png", "wb") as f:
            #     f.write(screenshot)

            screen_size = await computer.interface.get_screen_size()
            print(f"Screen size: {screen_size}")

            # Demonstrate coordinate conversion
            center_x, center_y = 733, 736
            print(f"Center in screen coordinates: ({center_x}, {center_y})")

            screenshot_center = await computer.to_screenshot_coordinates(center_x, center_y)
            print(f"Center in screenshot coordinates: {screenshot_center}")

            screen_center = await computer.to_screen_coordinates(*screenshot_center)
            print(f"Back to screen coordinates: {screen_center}")

            # Mouse Actions Examples
            print("\n=== Mouse Actions ===")
            await computer.interface.move_cursor(100, 100)
            await computer.interface.left_click()
            await computer.interface.right_click(300, 300)
            await computer.interface.double_click(400, 400)

            # Keyboard Actions Examples
            print("\n=== Keyboard Actions ===")
            await computer.interface.type_text("Hello, World!")
            await computer.interface.press_key("enter")

            # Clipboard Actions Examples
            print("\n=== Clipboard Actions ===")
            await computer.interface.set_clipboard("Test clipboard")
            content = await computer.interface.copy_to_clipboard()
            print(f"Clipboard content: {content}")

        finally:
            # Important to clean up resources
            pass
            # await computer.stop()
    except Exception as e:
        print(f"Error in main: {e}")
        traceback.print_exc()


if __name__ == "__main__":
    asyncio.run(main())
