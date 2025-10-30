"""
hud_eval_examples.py — minimal HUD evaluation runner

- Auto-discovers .env anywhere up the directory tree (via find_dotenv)
- Requires HUD_API_KEY in the resolved environment
- No Docker/local computer usage
"""

# imports
import asyncio
import logging
import os
import uuid
from pathlib import Path
from pprint import pprint

from agent import ComputerAgent
from agent.integrations.hud import run_full_dataset
from dotenv import find_dotenv, load_dotenv

"""
Loading env
"""


def load_env_or_fail() -> None:
    # Walk up from CWD / file dir to find nearest .env
    env_path = find_dotenv(usecwd=False)
    if not env_path:
        raise FileNotFoundError(
            "❌ .env not found. Place a .env at your repo root (or export HUD_API_KEY)."
        )
    load_dotenv(env_path, override=True)
    if not os.getenv("HUD_API_KEY"):
        raise EnvironmentError("❌ HUD_API_KEY is missing in the loaded environment")


"""
Build Agent Config
- customize agent behavior, tool integration, callbacks, resource management, and more
- https://docs.trycua.com/docs/agent-sdk/agent-loops#parameters
- https://docs.trycua.com/docs/agent-sdk/supported-model-providers
"""


def build_agent_config() -> dict:

    instruction = "You are a computer-using agent graded by deterministic checkers."

    return {
        "model": "openai/computer-use-preview",
        "trajectory_dir": str(Path("trajectories")),
        "only_n_most_recent_images": 3,
        "verbosity": logging.INFO,
        "instruction": instruction,
    }


"""
Hud Eval
"""


async def run_hud_eval() -> None:
    # load env and agent config
    load_env_or_fail()
    agent_config = build_agent_config()

    # Initialize to ensure config is valid (tools, verbosity, etc.)
    _ = ComputerAgent(**agent_config)

    job_name = (
        f"osworld-test-{str(uuid.uuid4())[:4]}"  # job name (each run of your task is a job on hud)
    )
    print(f"🚀 Running HUD eval: {job_name}")

    """
    Customize your hud eval below, check the doc for additional params
    - https://docs.trycua.com/docs/agent-sdk/integrations/hud#parameters-1
    - recommend low max steps (5-10) for testing, then max 100 for benchmarking
    - also select specific tasks to run by using splitting the dataset
    """
    results = await run_full_dataset(
        dataset="ddupont/OSWorld-Tiny-Public",
        job_name=job_name,
        **agent_config,
        max_concurrent=20,
        max_steps=50,
        # split="train[0:1]"
    )

    print(f"\n📊 Job: {job_name}")
    print(f"Total results: {len(results)}")
    pprint(results[:3])


def main() -> None:
    logging.basicConfig(level=logging.INFO)
    asyncio.run(run_hud_eval())


if __name__ == "__main__":
    main()
