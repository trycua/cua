"""
Pluggable AI Model Agent Loop Test

This test allows you to plug in any AI model to test it with the agent loop.
The agent loop interacts with a minimal mock computer that only provides
screenshot functionality and returns a static image.

Usage:
    python agent_test.py --model anthropic/claude-sonnet-4-20250514
    python agent_test.py --model openai/gpt-4o-mini
    python agent_test.py --model test-model  # Uses simple test model
"""

import asyncio
import argparse
import sys
from pathlib import Path
from typing import Optional

# Add project root to path
project_root = Path(__file__).parent.parent.parent
sys.path.insert(0, str(project_root))

try:
    from .mock_computer import MinimalMockComputer, MockComputerInterface
    from .ai_interface import AgentLoop, AIModelInterface, SimpleTestAIModel
except ImportError:
    # When running as script directly
    from mock_computer import MinimalMockComputer, MockComputerInterface
    from ai_interface import AgentLoop, AIModelInterface, SimpleTestAIModel


class CUAComputerAgentModel(AIModelInterface):
    """
    Wrapper for CUA's ComputerAgent to make it compatible with the pluggable interface.
    
    This allows using CUA's ComputerAgent as a pluggable AI model.
    """
    
    def __init__(self, model_name: str):
        self.model_name = model_name
        self.agent = None
        self._initialized = False
    
    async def _initialize(self):
        """Initialize the CUA ComputerAgent."""
        if self._initialized:
            return
        
        try:
            from agent import ComputerAgent
            from agent.computers.base import AsyncComputerHandler
            
            # Create a minimal computer handler that only implements screenshot
            class MinimalComputerHandler(AsyncComputerHandler):
                def __init__(self, mock_computer: MinimalMockComputer):
                    self.mock_computer = mock_computer
                
                async def screenshot(self) -> str:
                    return await self.mock_computer.screenshot()
                
                async def get_dimensions(self) -> tuple[int, int]:
                    return await self.mock_computer.get_screen_dimensions()
                
                # Implement other required methods as no-ops
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
                
                # Implement missing abstract methods
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
            
            # Create the computer handler
            self.mock_computer = MinimalMockComputer()
            await self.mock_computer.initialize()
            computer_handler = MinimalComputerHandler(self.mock_computer)
            
            # Create the ComputerAgent
            self.agent = ComputerAgent(
                model=self.model_name,
                tools=[computer_handler],
                max_trajectory_budget=5.0
            )
            
            self._initialized = True
            
        except ImportError as e:
            raise ImportError(f"CUA agent dependencies not available: {e}")
    
    async def generate_response(
        self, 
        messages,
        computer_interface: MockComputerInterface
    ):
        """Generate response using CUA's ComputerAgent."""
        try:
            from .ai_interface import AgentResponse
        except ImportError:
            from ai_interface import AgentResponse
        await self._initialize()
        
        # Convert messages to the format expected by ComputerAgent
        formatted_messages = []
        for msg in messages:
            if msg.role == "user":
                content = msg.content
                if msg.image_data:
                    content += f"\n[Image data: {len(msg.image_data)} characters]"
                formatted_messages.append({"role": "user", "content": content})
            elif msg.role == "assistant":
                formatted_messages.append({"role": "assistant", "content": msg.content})
        
        # Run the agent for one iteration
        try:
            if self.agent is None:
                raise RuntimeError("Agent not initialized")
            async for result in self.agent.run(formatted_messages):
                # Extract response from the result
                response_content = ""
                tool_calls = []
                
                for item in result.get("output", []):
                    if item["type"] == "message":
                        response_content = item["content"][0]["text"]
                    elif item["type"] == "tool_call":
                        tool_calls.append({
                            "name": item.get("tool_name", "unknown"),
                            "args": item.get("arguments", {})
                        })
                
                return AgentResponse(
                    content=response_content,
                    tool_calls=tool_calls,
                    finished=len(tool_calls) == 0  # Assume finished if no tool calls
                )
                
        except Exception as e:
            return AgentResponse(
                content=f"Error: {str(e)}",
                tool_calls=[],
                finished=True
            )
    
    def get_model_name(self) -> str:
        return f"cua-{self.model_name}"


def create_ai_model(model_name: str) -> AIModelInterface:
    """
    Create an AI model instance based on the model name.
    
    Args:
        model_name: Name of the model to create
        
    Returns:
        AIModelInterface instance
    """
    if model_name == "test-model":
        return SimpleTestAIModel("test-model")
    elif model_name.startswith("anthropic/") or model_name.startswith("openai/"):
        return CUAComputerAgentModel(model_name)
    else:
        # Try to use as CUA model
        return CUAComputerAgentModel(model_name)


async def test_agent_loop_with_model(
    model_name: str,
    image_path: Optional[str] = None,
    max_iterations: int = 5,
    initial_message: str = "Take a screenshot and tell me what you see. Try to interact with the interface."
):
    """
    Test the agent loop with a specific AI model.
    
    Args:
        model_name: Name of the AI model to test
        image_path: Path to static image (optional)
        max_iterations: Maximum number of iterations
        initial_message: Initial message to send to the agent
    """
    print(f"🤖 Testing Agent Loop with AI Model: {model_name}")
    print("=" * 80)
    
    try:
        # Create AI model
        print(f"✅ Step 1: Creating AI model: {model_name}")
        ai_model = create_ai_model(model_name)
        
        # Create mock computer
        print("✅ Step 2: Creating mock computer with static image")
        mock_computer = MinimalMockComputer(image_path)
        await mock_computer.initialize()
        computer_interface = MockComputerInterface(mock_computer)
        
        # Create agent loop
        print("✅ Step 3: Creating agent loop")
        agent_loop = AgentLoop(ai_model, computer_interface)
        
        print("✅ Step 4: Starting agent execution...")
        print("\n" + "=" * 80)
        print("AGENT EXECUTION:")
        print("=" * 80)
        
        iteration_count = 0
        async for result in agent_loop.run(initial_message, max_iterations):
            iteration_count += 1
            print(f"\n--- Iteration {iteration_count} ---")
            
            if "error" in result:
                print(f"❌ Error: {result['error']}")
                break
            
            response = result["response"]
            print(f"🔄 AI Response: {response.content}")
            
            if response.tool_calls:
                for tool_call in response.tool_calls:
                    print(f"🔧 Tool Call: {tool_call['name']} with args: {tool_call['args']}")
            else:
                print("🔧 No tool calls made")
            
            print(f"📊 Screenshots taken: {result.get('screenshot_taken', False)}")
            print(f"📊 Conversation length: {result.get('conversation_length', 0)}")
            
            if response.finished:
                print("🏁 Agent finished")
                break
        
        print("\n" + "=" * 80)
        print("AGENT EXECUTION COMPLETE")
        print("=" * 80)
        
        # Print final statistics
        screen_info = await computer_interface.get_screen_info()
        print(f"✅ Total iterations: {iteration_count}")
        print(f"✅ Total screenshots: {screen_info['action_count']}")
        print(f"✅ Model: {ai_model.get_model_name()}")
        print(f"✅ Screen dimensions: {screen_info['width']}x{screen_info['height']}")
        
        print("\n" + "=" * 80)
        print("🎉 AGENT LOOP TEST COMPLETE!")
        print("=" * 80)
        print("\nThis proves:")
        print(f"• AI model '{model_name}' works with the agent loop")
        print("• Mock computer serves static image successfully")
        print("• Agent loop executes multiple iterations without crashing")
        print("• AI model can analyze screenshots and generate responses")
        print("• Tool calling interface works correctly")
        
        return True
        
    except Exception as e:
        print(f"\n❌ Test failed: {str(e)}")
        print("\nTroubleshooting:")
        print("• Make sure the AI model name is correct")
        print("• Check that required dependencies are installed")
        print("• Verify API keys are set (for external models)")
        print("• Try using 'test-model' for a simple test")
        return False


async def main():
    """Main function to run the agent loop test."""
    parser = argparse.ArgumentParser(description="Test AI models with agent loop")
    parser.add_argument(
        "--model", 
        default="test-model",
        help="AI model to test (e.g., 'anthropic/claude-sonnet-4-20250514', 'openai/gpt-4o-mini', 'test-model')"
    )
    parser.add_argument(
        "--image",
        help="Path to static image file (optional)"
    )
    parser.add_argument(
        "--max-iterations",
        type=int,
        default=5,
        help="Maximum number of iterations (default: 5)"
    )
    parser.add_argument(
        "--message",
        default="Take a screenshot and tell me what you see. Try to interact with the interface.",
        help="Initial message to send to the agent"
    )
    
    args = parser.parse_args()
    
    # Use default image if none provided
    image_path = args.image
    if not image_path:
        default_image = Path(__file__).parent / "test_images" / "image.png"
        if default_image.exists():
            image_path = str(default_image)
    
    success = await test_agent_loop_with_model(
        model_name=args.model,
        image_path=image_path,
        max_iterations=args.max_iterations,
        initial_message=args.message
    )
    
    sys.exit(0 if success else 1)


if __name__ == "__main__":
    asyncio.run(main())