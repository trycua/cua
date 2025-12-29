"""
Browser Agent Example

Demonstrates how to use the ComputerAgent with BrowserTool to control a browser
programmatically via the computer server using Azure AI model (Fara-7B).

Prerequisites:
    - Computer server running (Docker container or local)
    - For Docker: Container should be running with browser tool support
    - For local: Playwright and Firefox must be installed

Usage:
    python examples/browser_agent_example.py
"""

import asyncio
import logging
import signal
import sys
import traceback
from pathlib import Path

# Add the libs path to sys.path
libs_path = Path(__file__).parent.parent / "libs" / "python"
sys.path.insert(0, str(libs_path))

from agent import ComputerAgent
from agent.tools import BrowserTool
from computer import Computer

# Import utility functions
from utils import handle_sigint, load_dotenv_files

# Configure logging to see what's happening
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def run_browser_agent_example():
    """Run example of using ComputerAgent with BrowserTool."""
    print("\n=== Example: ComputerAgent with BrowserTool (Azure AI - Fara-7B) ===")

    try:
        # Initialize the computer interface
        # For local testing, use provider_type="docker"
        # For provider_type="cloud", provide name and api_key
        computer = Computer(
            provider_type="docker",
            os_type="linux",
            image="cua-xfce:dev",
            # verbosity=logging.DEBUG,
        )
        await computer.run()

        # Initialize the browser tool with the computer interface
        browser = BrowserTool(interface=computer)

        # Create ComputerAgent with BrowserTool
        agent = ComputerAgent(
            model="azure_ml/Fara-7B",
            tools=[browser],
            api_base="https://foundry-inference-gxttr.centralus.inference.ml.azure.com",
            api_key="YOUR API KEY HERE",
            only_n_most_recent_images=3,
            # verbosity=logging.DEBUG,
            trajectory_dir="trajectories",
            max_trajectory_budget=1.0,
        )

        # Open playground UI
        agent.open()

        # Wait until key press
        input("Press enter to exit")

    except Exception as e:
        logger.error(f"Error in run_browser_agent_example: {e}")
        traceback.print_exc()
        raise


def main():
    """Run the browser agent example."""
    try:
        load_dotenv_files()

        # Register signal handler for graceful exit
        signal.signal(signal.SIGINT, handle_sigint)

        asyncio.run(run_browser_agent_example())
    except Exception as e:
        print(f"Error running example: {e}")
        traceback.print_exc()


if __name__ == "__main__":
    main()
