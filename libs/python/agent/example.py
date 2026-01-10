"""
Example usage of the agent library with docstring-based tool definitions.
"""

import asyncio
import logging

from agent import ComputerAgent
from computer import Computer
from computer.helpers import sandboxed


@sandboxed()
def read_file(location: str) -> str:
    """Read contents of a file

    Parameters
    ----------
    location : str
        Path to the file to read

    Returns
    -------
    str
        Contents of the file or error message
    """
    try:
        with open(location, "r") as f:
            return f.read()
    except Exception as e:
        return f"Error reading file: {str(e)}"


def save_note(content: str, filename: str = "note.txt") -> str:
    """Save content to a note file

    Parameters
    ----------
    content : str
        Content to save to the file
    filename : str, optional
        Name of the file to save to (default is "note.txt")

    Returns
    -------
    str
        Success or error message
    """
    try:
        with open(filename, "w") as f:
            f.write(content)
        return f"Saved note to {filename}"
    except Exception as e:
        return f"Error saving note: {str(e)}"


def calculate(a: int, b: int) -> int:
    """Calculate the sum of two integers

    Parameters
    ----------
    a : int
        First integer
    b : int
        Second integer

    Returns
    -------
    int
        Sum of the two integers
    """
    return a + b


async def main():
    """Example usage of ComputerAgent with different models"""

    # Example 1: Using Claude with computer and custom tools
    print("=== Example 1: Claude with Computer ===")

    import json
    import os

    import dotenv

    dotenv.load_dotenv()

    assert os.getenv("CUA_CONTAINER_NAME") is not None, "CUA_CONTAINER_NAME is not set"
    assert os.getenv("CUA_API_KEY") is not None, "CUA_API_KEY is not set"

    async with Computer(
        os_type="linux",
        provider_type="cloud",
        name=os.getenv("CUA_CONTAINER_NAME") or "",
        api_key=os.getenv("CUA_API_KEY") or "",
    ) as computer:
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
            # TODO: add local mlx provider
            # model="mlx-community/UI-TARS-1.5-7B-6bit",
            # model="ollama_chat/0000/ui-tars-1.5-7b",
            # == Omniparser + Any LLM ==
            # model="omniparser+..."
            # model="omniparser+anthropic/claude-opus-4-20250514",
            tools=[computer],
            only_n_most_recent_images=3,
            verbosity=logging.INFO,
            trajectory_dir="trajectories",
            use_prompt_caching=True,
            max_trajectory_budget={
                "max_budget": 1.0,
                "raise_error": True,
                "reset_after_each_run": False,
            },
        )

        history = []
        while True:
            user_input = input("> ")
            history.append({"role": "user", "content": user_input})

            # Non-streaming usage
            async for result in agent.run(history, stream=False):
                history += result["output"]

                # # Print output
                # for item in result["output"]:
                #     if item["type"] == "message":
                #         print(item["content"][0]["text"])
                #     elif item["type"] == "computer_call":
                #         action = item["action"]
                #         action_type = action["type"]
                #         action_args = {k: v for k, v in action.items() if k != "type"}
                #         print(f"{action_type}({action_args})")
                #     elif item["type"] == "function_call":
                #         action = item["name"]
                #         action_args = item["arguments"]
                #         print(f"{action}({action_args})")
                #     elif item["type"] == "function_call_output":
                #         print("===>", item["output"])


if __name__ == "__main__":
    asyncio.run(main())
