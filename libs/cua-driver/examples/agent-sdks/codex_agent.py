"""Run a Codex Python SDK desktop task through Cua Driver's MCP boundary."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil

from openai_codex import AsyncCodex, CodexConfig, Sandbox


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", help="Trusted desktop task for Codex to perform")
    return parser.parse_args()


def driver_binary() -> str:
    configured = os.environ.get("CUA_DRIVER_BIN")
    if configured:
        return configured
    discovered = shutil.which("cua-driver")
    if discovered:
        return discovered
    raise RuntimeError("cua-driver is not on PATH; set CUA_DRIVER_BIN")


async def main() -> None:
    args = parse_args()
    binary = driver_binary()
    config = CodexConfig(
        config_overrides=(
            f"mcp_servers.cua_driver.command={json.dumps(binary)}",
            'mcp_servers.cua_driver.args=["mcp"]',
            "mcp_servers.cua_driver.required=true",
        )
    )
    async with AsyncCodex(config) as codex:
        thread = await codex.thread_start(
            model=os.environ.get("CODEX_MODEL"),
            sandbox=Sandbox.read_only,
        )
        result = await thread.run(
            f"""Complete this trusted desktop task through the cua_driver MCP server:

{args.task}

This MCP transport owns one implicit lifecycle session. Omit the session field
for ordinary calls so the runtime creates and reuses it. Select an exact window
or desktop target on every action; session identity never selects capture
modality or permission authority. Use only cua_driver MCP tools for desktop
observation and interaction. Inspect before each action and verify afterward.
If a mutation times out, observe before any retry; never blindly replay an
action with an unknown outcome. Return a concise result and name anything
unverified.
"""
        )
        print(result.final_response)


if __name__ == "__main__":
    asyncio.run(main())
