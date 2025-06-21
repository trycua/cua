#%%[markdown]
## Import Libraries

# %%
#### !pip uninstall -y cua-agent
#!pip install "cua-agent[all]"
#!pip install python-dotenv

# %%
import os
import logging
import asyncio
from pathlib import Path
from dotenv import load_dotenv
from computer import Computer  # You may need to install this package
from agent import ComputerAgent, LLM, AgentLoop, LLMProvider  # You may need to install this package


#%%[markdown]
## Set up API Key
# %%
# Load environment variables from .env file in the parent directory
dotenv_path = Path(__file__).parent.parent / '.env'
load_dotenv(dotenv_path)

# Get API keys from environment or prompt user
anthropic_key = os.getenv("ANTHROPIC_API_KEY") or input("Enter your Anthropic API key: ")
openai_key = os.getenv("OPENAI_API_KEY") or input("Enter your OpenAI API key: ")

computer = Computer(verbosity=logging.INFO)

# %% [markdown]
## Create Agent and Run Tasks
# %%
# Create agent with Anthropic loop and provider
agent = ComputerAgent(
    computer=computer,
    loop=AgentLoop.OMNI,
    model=LLM(provider=LLMProvider.ANTHROPIC, name="claude-3-7-sonnet-20250219"),
    save_trajectory=True,
    trajectory_dir=str(Path("trajectories")),
    only_n_most_recent_images=3,
    verbosity=logging.INFO,
    api_key=anthropic_key
    )

tasks = [
    "Launch Calculator, perform 123 + 456, then report the result. "

]

for i, task in enumerate(tasks):
    print(f"\nExecuting task {i}/{len(tasks)}: {task}")
    async for result in agent.run(task):
        # print(result)
        pass

    print(f"\n✅ Task {i+1}/{len(tasks)} completed: {task}")
# %%
print(anthropic_key)
# %%
