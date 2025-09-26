"""Example demonstrating the ComputerAgent capabilities with the Omni provider."""

import asyncio
import logging
import traceback
import signal
import sys
import os

from computer import Computer

# Import the unified agent class and types
from agent import ComputerAgent

from libs.python.agent.agent.callbacks.snapshot_manager import SnapshotManagerCallback

# Import utility functions
from utils import load_dotenv_files, handle_sigint

# Set up logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

async def run_agent_example():
    """Run example of using the ComputerAgent with different models."""
    print("\n=== Example: ComputerAgent with different models ===")

    try:
        # Create a local macOS computer
        computer = Computer(
            os_type="linux",
            provider_type="docker",
            image="trycua/cua-ubuntu:latest",
            name='snapshot-container',
            verbosity=logging.DEBUG,
        )

        await computer.run()
        
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

            # == OpenAI CUA (computer-use-preview) ==
            # model="claude-3-5-sonnet-20241022",

            # == Anthropic CUA (Claude > 3.5) ==
            # model="anthropic/claude-opus-4-20250514",
            model="anthropic/claude-sonnet-4-20250514",
            # model="anthropic/claude-3-7-sonnet-20250219",
            # model="anthropic/claude-3-5-sonnet-20241022",

            # == UI-TARS ==
            # model="huggingface-local/ByteDance-Seed/UI-TARS-1.5-7B",
            # model="mlx/mlx-community/UI-TARS-1.5-7B-6bit",
            # model="ollama_chat/0000/ui-tars-1.5-7b",

            # == Omniparser + Any LLM ==
            # model="omniparser+anthropic/claude-opus-4-20250514",
            # model="omniparser+ollama_chat/gemma3:12b-it-q4_K_M",

            tools=[computer],
            only_n_most_recent_images=3,
            verbosity=logging.DEBUG,
            trajectory_dir="trajectories",
            use_prompt_caching=True,
            max_trajectory_budget=1.0,
        )

        # Create snapshot callback after computer is configured
        # The agent will initialize the computer when first used
        snapshot_callback = SnapshotManagerCallback(
            computer=computer,
            snapshot_interval="run_boundaries",  # Snapshot at start and end of runs
            max_snapshots=10,  # Keep up to 10 snapshots
            retention_days=7,   # Delete snapshots older than 7 days
            auto_cleanup=True   # Automatically cleanup old snapshots
        )

        # Add the callback to the agent's callback list
        agent.callbacks.append(snapshot_callback)

        # Example tasks to demonstrate the agent
        tasks = [
            "Create a file called test.txt"
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
                # manual_snapshot = await snapshot_callback.create_manual_snapshot(f"Step number {i} in {task}")
                
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

        # Demonstrate manual snapshot operations
        print("\n=== Snapshot Management Demo ===")

        # List snapshots created during the run
        snapshots = await snapshot_callback.list_snapshots()
        print(f"Agent run created {len(snapshots)} snapshots:")
        for snap in snapshots:
            metadata = snap.get('metadata', {})
            print(f"  - {snap.get('tag')} (Trigger: {metadata.get('trigger', 'unknown')})")

        # Create a manual snapshot
        manual_snapshot = await snapshot_callback.create_manual_snapshot("End of demo run")
        if manual_snapshot:
            print(f"Created manual snapshot: {manual_snapshot.get('tag')}")

        print("💡 You can restore to any snapshot using:")
        print("   await snapshot_callback.restore_snapshot(snapshot_id)")

    except Exception as e:
        logger.error("Error in run_agent_example: %s", e)
        traceback.print_exc()
        raise
    finally:
        # Ensure we clean up the computer connection
        if 'computer' in locals():
            await computer.stop()

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
