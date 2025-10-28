#!/usr/bin/env python3
"""
Example: Using the Pluggable AI Model Agent Loop Testing Framework

This example shows how to use the new framework to test different AI models
with the agent loop using a minimal mock computer.
"""

import asyncio
import sys
from pathlib import Path

# Add the current directory to path for imports
sys.path.insert(0, str(Path(__file__).parent))

from agent_test import create_ai_model, test_agent_loop_with_model
from ai_interface import AgentLoop, SimpleTestAIModel
from mock_computer import MinimalMockComputer, MockComputerInterface


async def example_basic_usage():
    """Example: Basic usage with test model."""
    print("🔬 Example 1: Basic Usage with Test Model")
    print("=" * 60)

    success = await test_agent_loop_with_model(
        model_name="test-model",
        max_iterations=2,
        initial_message="Look at the screen and tell me what you see.",
    )

    print(f"✅ Example 1 {'PASSED' if success else 'FAILED'}")
    print()


async def example_custom_model():
    """Example: Creating a custom AI model."""
    print("🔬 Example 2: Custom AI Model")
    print("=" * 60)

    class MyCustomModel(SimpleTestAIModel):
        def __init__(self):
            super().__init__("my-custom-model")
            self.custom_responses = [
                "I see a beautiful macOS desktop!",
                "The Safari icon looks very blue.",
                "I think this is a static image for testing.",
            ]
            self.response_index = 0

        async def generate_response(self, messages, computer_interface):
            """Generate custom responses."""
            from ai_interface import AgentResponse

            if self.response_index < len(self.custom_responses):
                content = self.custom_responses[self.response_index]
                self.response_index += 1
                finished = self.response_index >= len(self.custom_responses)
            else:
                content = "I've said everything I wanted to say."
                finished = True

            return AgentResponse(content=content, tool_calls=[], finished=finished)

    # Create custom model and test it
    custom_model = MyCustomModel()
    mock_computer = MinimalMockComputer()
    await mock_computer.initialize()
    computer_interface = MockComputerInterface(mock_computer)

    agent_loop = AgentLoop(custom_model, computer_interface)

    print("Running custom model...")
    async for result in agent_loop.run("Tell me about what you see.", max_iterations=5):
        print(f"Iteration {result['iteration']}: {result['response'].content}")
        if result["response"].finished:
            break

    print("✅ Example 2 COMPLETED")
    print()


async def example_direct_api():
    """Example: Using the framework directly without the test runner."""
    print("🔬 Example 3: Direct API Usage")
    print("=" * 60)

    # Create components
    ai_model = SimpleTestAIModel("direct-api-model")
    mock_computer = MinimalMockComputer()
    await mock_computer.initialize()
    computer_interface = MockComputerInterface(mock_computer)

    # Create agent loop
    agent_loop = AgentLoop(ai_model, computer_interface)

    # Take a screenshot directly
    screenshot = await computer_interface.take_screenshot()
    print(f"📸 Screenshot taken: {len(screenshot)} characters")

    # Get screen info
    screen_info = await computer_interface.get_screen_info()
    print(f"📊 Screen info: {screen_info}")

    # Run agent loop
    print("Running agent loop...")
    async for result in agent_loop.run("Analyze the screen.", max_iterations=2):
        print(f"  Iteration {result['iteration']}: {result['response'].content}")

    print("✅ Example 3 COMPLETED")
    print()


async def main():
    """Run all examples."""
    print("🚀 Pluggable AI Model Agent Loop Testing Framework Examples")
    print("=" * 80)
    print()

    try:
        await example_basic_usage()
        await example_custom_model()
        await example_direct_api()

        print("🎉 All examples completed successfully!")
        print()
        print("Key Benefits Demonstrated:")
        print("• ✅ Pluggable AI models - easy to swap different models")
        print("• ✅ Minimal mock computer - only screenshot functionality")
        print("• ✅ Clean agent loop - coordinates AI and computer")
        print("• ✅ Flexible interface - works with any AI model")
        print("• ✅ Easy testing - simple API for testing AI capabilities")

    except Exception as e:
        print(f"❌ Example failed: {e}")
        return False

    return True


if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)
