from computer import Computer
from agent import ComputerAgent
import asyncio

computer = Computer(
    os_type="linux",
    provider_type="docker",
    image="trycua/cua-ubuntu:latest",
    name="strange_yalow"
)

asyncio.run(computer.run()) # Launch & connect to Docker container

agent = ComputerAgent(
    model="claude-sonnet-4-20250514",
    tools=[computer],
    max_trajectory_budget=5.0
)
messages = [{"role": "user", "content": "Take a screenshot and tell me what you see"}]
for result in agent.run(messages):
    for item in result["output"]:
        if item["type"] == "message":
            print(item["content"][0]["text"])

