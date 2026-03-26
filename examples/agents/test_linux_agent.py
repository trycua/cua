"""Run a ComputerAgent on a local Linux container using Sandbox.ephemeral.

    async with Sandbox.ephemeral(Image.linux(), local=True) as sb:
        agent = ComputerAgent(model="anthropic/claude-sonnet-4-5-20250929", tools=[sb])
        async for chunk in agent.run("What OS is this?"):
            for item in chunk.get("output", []):
                if item.get("type") == "message":
                    for block in item.get("content", []):
                        print(block.get("text", ""), end="")

The Sandbox is passed directly as a tool — no separate Computer wrapper needed.
Requires Docker and ANTHROPIC_API_KEY.
"""

from __future__ import annotations

import asyncio
import os
import subprocess

import pytest
from agent import ComputerAgent
from cua_sandbox import Image, Sandbox

pytestmark = pytest.mark.asyncio


def _has_docker() -> bool:
    try:
        subprocess.run(["docker", "info"], capture_output=True, check=True, timeout=10)
        return True
    except (subprocess.SubprocessError, FileNotFoundError):
        return False


@pytest.mark.skipif(not _has_docker(), reason="Docker not available")
@pytest.mark.skipif(not os.getenv("ANTHROPIC_API_KEY"), reason="ANTHROPIC_API_KEY not set")
async def test_linux_agent():
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),
        local=True,
        name="example-linux-agent",
    ) as sb:
        agent = ComputerAgent(
            model="anthropic/claude-sonnet-4-5-20250929",
            tools=[sb],
        )

        chunks = []
        async for chunk in agent.run(
            "Run the command 'echo hello' in the terminal and tell me the output."
        ):
            chunks.append(chunk)

        assert chunks, "Agent produced no output"
        output_items = [item for chunk in chunks for item in chunk.get("output", [])]
        assert output_items, "Agent produced no output items"


async def main():
    async with Sandbox.ephemeral(
        Image.linux("ubuntu", "24.04"),
        local=True,
        name="example-linux-agent",
    ) as sb:
        agent = ComputerAgent(
            model="anthropic/claude-sonnet-4-5-20250929",
            tools=[sb],
        )

        print("Agent running...")
        async for chunk in agent.run(
            "Run the command 'echo hello' in the terminal and tell me the output."
        ):
            for item in chunk.get("output", []):
                if item.get("type") == "message":
                    for block in item.get("content", []):
                        print(block.get("text", ""), end="", flush=True)
        print()


if __name__ == "__main__":
    asyncio.run(main())
