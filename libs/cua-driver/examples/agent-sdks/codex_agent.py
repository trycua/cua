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
unverified. When both history_status and history_query are advertised and the
task asks to continue, resume, or recall prior Cua work, call history_status
first. If history is healthy and access is admitted, make one bounded initial
history_query before broad application or window discovery. Use its
metadata only as a lead and verify current state; omitted content, geometry,
arguments, results, and user intent remain unknown. Continue without history
if either tool is absent, access is denied, results are empty, or history is
unhealthy. Do not query history for unrelated tasks merely because the tools
exist, and never mutate history lifecycle or settings. Make another bounded
query only for a relevant session or sequence boundary, never to reconstruct
excluded fields.
"""
        )
        print(result.final_response)


if __name__ == "__main__":
    asyncio.run(main())
