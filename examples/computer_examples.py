import asyncio
import os
import sys
import traceback
from pathlib import Path

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

from computer.computer import Computer
from computer.logger import LogLevel
from computer.providers.base import VMProviderType


async def main():
    try:
        print("\n=== Using direct initialization ===")

        # Create a local macOS computer
        computer = Computer(
            display="1024x768",
            memory="8GB",
            cpu="4",
            os_type="macos",
            name="macos",
            verbosity=LogLevel.VERBOSE,
            provider_type=VMProviderType.LUME,
            storage="/Users/<USER>/repos/trycua/computer/examples/storage",
            shared_directories=["/Users/<USER>/repos/trycua/computer/examples/shared"],
            ephemeral=False,
        )

        # Create a remote Linux computer with Cua
        # computer = Computer(
        #     os_type="linux",
        #     api_key=os.getenv("CUA_API_KEY"),
        #     name=os.getenv("CONTAINER_NAME"),
        #     provider_type=VMProviderType.CLOUD,
        # )

        try:
            # Run the computer with default parameters
            await computer.run()

            screenshot = await computer.interface.screenshot()

            # Create output directory if it doesn't exist
            output_dir = Path("./output")
            output_dir.mkdir(exist_ok=True)

            screenshot_path = output_dir / "screenshot.png"
            with open(screenshot_path, "wb") as f:
                f.write(screenshot)
            print(f"Screenshot saved to: {screenshot_path.absolute()}")

            # await computer.interface.hotkey("command", "space")

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
            await computer.stop()
    except Exception as e:
        print(f"Error in main: {e}")
        traceback.print_exc()


if __name__ == "__main__":
    asyncio.run(main())
