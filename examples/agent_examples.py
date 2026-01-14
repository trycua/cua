"""Example demonstrating the ComputerAgent capabilities with the Omni provider."""

import asyncio
import logging
import signal
import traceback

# Import the unified agent class and types
from agent import ComputerAgent
from computer import Computer, VMProviderType

# Import utility functions
from utils import handle_sigint, load_dotenv_files

# Set up logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def run_agent_example():
    """Run example of using the ComputerAgent with different models."""
    print("\n=== Example: ComputerAgent with different models ===")

    try:
        # Create a local macOS computer
        computer = Computer(
            os_type="macos",
            verbosity=logging.DEBUG,
        )

        # Create a remote Linux computer with Cua
        # computer = Computer(
        #     os_type="linux",
        #     api_key=os.getenv("CUA_API_KEY"),
        #     name=os.getenv("CUA_CONTAINER_NAME"),
        #     provider_type=VMProviderType.CLOUD,
        # )

        # Create ComputerAgent with new API
        agent = ComputerAgent(
            # Supported models:
            # == OpenAI Cua (computer-use-preview) ==
            model="openai/computer-use-preview",
            # == Anthropic Cua (Claude > 3.5) ==
            # model="anthropic/claude-opus-4-20250514",
            # model="anthropic/claude-sonnet-4-20250514",
            # model="anthropic/claude-3-7-sonnet-20250219",
            # model="anthropic/claude-sonnet-4-5-20250929",
            # == UI-TARS ==
            # model="huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B",
            # model="mlx/mlx-community/UI-TARS-1.5-7B-6bit",
            # model="ollama_chat/0000/ui-tars-1.5-7b",
            # == Omniparser + Any LLM ==
            # model="omniparser+anthropic/claude-opus-4-20250514",
            # model="omniparser+ollama_chat/gemma3:12b-it-q4_K_M",
            # == Omniparser + Vertex AI Gemini 3 (with thinking_level) ==
            # model="omni+vertex_ai/gemini-3-flash",
            # thinking_level="high",  # or "low"
            # media_resolution="medium",  # or "low" or "high"
            tools=[computer],
            only_n_most_recent_images=3,
            verbosity=logging.DEBUG,
            trajectory_dir="trajectories",
            use_prompt_caching=True,
            max_trajectory_budget=1.0,
        )

        # Example tasks to demonstrate the agent
        tasks = [
            "Look for a repository named trycua/cua on GitHub.",
            "Check the open issues, open the most recent one and read it.",
            "Clone the repository in users/lume/projects if it doesn't exist yet.",
            "Open the repository with an app named Cursor (on the dock, black background and white cube icon).",
            "From Cursor, open Composer if not already open.",
            "Focus on the Composer text area, then write and submit a task to help resolve the GitHub issue.",
        ]

        # Use message-based conversation history
        history = []

        for i, task in enumerate(tasks):
            print(f"\nExecuting task {i+1}/{len(tasks)}: {task}")

            # Add user message to history
            history.append({"role": "user", "content": task})

            # Run agent with conversation history
            async for result in agent.run(history, stream=False):
                # Add agent outputs to history
                history += result.get("output", [])

                # Print output for debugging
                for item in result.get("output", []):
                    if item.get("type") == "message":
                        content = item.get("content", [])
                        for content_part in content:
                            if content_part.get("text"):
                                print(f"Agent: {content_part.get('text')}")
                    elif item.get("type") == "computer_call":
                        action = item.get("action", {})
                        action_type = action.get("type", "")
                        print(f"Computer Action: {action_type}({action})")
                    elif item.get("type") == "computer_call_output":
                        print("Computer Output: [Screenshot/Result]")

            print(f"✅ Task {i+1}/{len(tasks)} completed: {task}")

    except Exception as e:
        logger.error(f"Error in run_agent_example: {e}")
        traceback.print_exc()
        raise


def main():
    """Run the Anthropic agent example."""
    try:
        load_dotenv_files()

        # Register signal handler for graceful exit
        signal.signal(signal.SIGINT, handle_sigint)

        asyncio.run(run_agent_example())
    except Exception as e:
        print(f"Error running example: {e}")
        traceback.print_exc()


if __name__ == "__main__":
    main()
