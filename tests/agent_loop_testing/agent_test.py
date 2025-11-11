#!/usr/bin/env python3
"""
Simple CUA Agent Test

Tests the actual CUA ComputerAgent SDK with a mock computer.
Only provides screenshot functionality - no complex computer actions.
"""

import asyncio
import base64
import sys
from io import BytesIO
from pathlib import Path

from PIL import Image, ImageDraw

# Add project root to path
project_root = Path(__file__).parent.parent.parent
sys.path.insert(0, str(project_root))


class MockComputer:
    """Mock computer that only provides screenshots."""

    def __init__(self):
        self.action_count = 0
        self._image = self._create_image()

    def _create_image(self) -> str:
        """Create a simple desktop image."""
        img = Image.new("RGB", (1920, 1080), color="lightblue")
        draw = ImageDraw.Draw(img)

        # Draw Safari icon
        draw.rectangle([100, 950, 150, 1000], fill="blue", outline="black", width=2)
        draw.text((110, 960), "Safari", fill="white")

        # Draw Terminal icon
        draw.rectangle([200, 950, 250, 1000], fill="green", outline="black", width=2)
        draw.text((210, 960), "Terminal", fill="white")

        # Convert to base64
        img_bytes = BytesIO()
        img.save(img_bytes, format="PNG")
        return base64.b64encode(img_bytes.getvalue()).decode("utf-8")

    async def screenshot(self) -> str:
        self.action_count += 1
        return self._image

    async def get_dimensions(self) -> tuple[int, int]:
        return (1920, 1080)

    # All other methods are no-ops (required by CUA interface)
    async def click(self, x: int, y: int, button: str = "left") -> None:
        await asyncio.sleep(0.1)

    async def double_click(self, x: int, y: int) -> None:
        await asyncio.sleep(0.1)

    async def scroll(self, x: int, y: int, scroll_x: int, scroll_y: int) -> None:
        await asyncio.sleep(0.1)

    async def type(self, text: str) -> None:
        await asyncio.sleep(0.1)

    async def wait(self, ms: int = 1000) -> None:
        await asyncio.sleep(ms / 1000.0)

    async def move(self, x: int, y: int) -> None:
        await asyncio.sleep(0.1)

    async def keypress(self, keys) -> None:
        await asyncio.sleep(0.1)

    async def drag(self, path) -> None:
        await asyncio.sleep(0.1)

    async def get_current_url(self) -> str:
        return "desktop://mock"

    async def get_environment(self) -> str:
        return "mac"

    # Required abstract methods
    async def left_mouse_down(self, x: int = 0, y: int = 0) -> None:
        await asyncio.sleep(0.1)

    async def left_mouse_up(self, x: int = 0, y: int = 0) -> None:
        await asyncio.sleep(0.1)

    async def right_mouse_down(self, x: int = 0, y: int = 0) -> None:
        await asyncio.sleep(0.1)

    async def right_mouse_up(self, x: int = 0, y: int = 0) -> None:
        await asyncio.sleep(0.1)

    async def mouse_move(self, x: int, y: int) -> None:
        await asyncio.sleep(0.1)

    async def key_down(self, key: str) -> None:
        await asyncio.sleep(0.1)

    async def key_up(self, key: str) -> None:
        await asyncio.sleep(0.1)

    async def type_text(self, text: str) -> None:
        await asyncio.sleep(0.1)


async def test_cua_agent(model_name: str):
    """Test CUA agent with mock computer."""
    print(f"🤖 Testing CUA Agent: {model_name}")
    print("=" * 50)

    try:
        # Import the real CUA agent
        from agent import ComputerAgent

        # Create mock computer
        mock_computer = MockComputer()

        # Create the real CUA ComputerAgent
        agent = ComputerAgent(model=model_name, tools=[mock_computer], max_trajectory_budget=5.0)

        print("✅ CUA Agent created")
        print("✅ Mock computer ready")
        print("🚀 Running agent...")
        print()

        # Run the agent with a specific task
        message = "Open Safari browser"

        iteration = 0
        async for result in agent.run([{"role": "user", "content": message}]):
            iteration += 1
            print(f"Iteration {iteration}:")

            # Print agent output
            output_items = result.get("output", [])
            if not output_items:
                print("  (No output from agent)")
            else:
                for item in output_items:
                    if item["type"] == "message":
                        print(f"  Agent: {item['content'][0]['text']}")
                    elif item["type"] == "tool_call":
                        print(f"  Tool: {item.get('tool_name')} {item.get('arguments')}")
                    else:
                        print(f"  Unknown output type: {item}")

            # Debug: print full result for empty iterations
            if not output_items:
                print(f"  Debug - Full result: {result}")

            # Let the agent decide when to stop (it should try to complete the task)
            # Only stop after 5 iterations to prevent infinite loops
            if iteration >= 5:
                print("🏁 Stopping after 5 iterations (safety limit)")
                break

        print()
        print("=" * 50)
        print("🎉 TEST COMPLETE!")
        print("=" * 50)
        print(f"✅ Model: {model_name}")
        print(f"✅ Iterations: {iteration}")
        print(f"✅ Screenshots: {mock_computer.action_count}")
        print("✅ Agent executed successfully")

        return True

    except ImportError as e:
        print(f"❌ Import error: {e}")
        print("💡 Install CUA: pip install -e libs/python/agent -e libs/python/computer")
        return False
    except Exception as e:
        print(f"❌ Test failed: {e}")
        return False


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Test CUA Agent with mock computer")
    parser.add_argument(
        "--model", default="anthropic/claude-sonnet-4-20250514", help="CUA model to test"
    )
    args = parser.parse_args()

    success = asyncio.run(test_cua_agent(args.model))
    sys.exit(0 if success else 1)
