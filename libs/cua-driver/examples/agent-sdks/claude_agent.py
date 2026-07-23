"""Run a Claude Agent SDK desktop task through Cua Driver's MCP server."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
from pathlib import Path
from typing import Literal, cast
from uuid import uuid4

from claude_agent_sdk import ClaudeAgentOptions, ClaudeSDKClient, ResultMessage

CaptureScope = Literal["auto", "window", "desktop"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", help="Trusted desktop task for Claude to perform")
    return parser.parse_args()


def driver_binary() -> str:
    configured = os.environ.get("CUA_DRIVER_BIN")
    if configured:
        return configured
    discovered = shutil.which("cua-driver")
    if discovered:
        return discovered
    raise RuntimeError("cua-driver is not on PATH; set CUA_DRIVER_BIN")


def capture_scope() -> CaptureScope:
    value = os.environ.get("CUA_CAPTURE_SCOPE", "auto")
    if value not in {"auto", "window", "desktop"}:
        raise ValueError("CUA_CAPTURE_SCOPE must be auto, window, or desktop")
    return cast(CaptureScope, value)


async def driver_call(binary: str, tool: str, arguments: dict[str, str]) -> None:
    process = await asyncio.create_subprocess_exec(
        binary,
        "call",
        tool,
        json.dumps(arguments),
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await process.communicate()
    if process.returncode != 0:
        detail = stderr.decode().strip() or stdout.decode().strip()
        raise RuntimeError(f"cua-driver call {tool} failed: {detail}")


def agent_prompt(task: str, session: str, scope: CaptureScope) -> str:
    return f"""Complete this desktop task through the cua_driver MCP server:

{task}

The host has already started session {session!r} with capture scope {scope!r}.
Use only cua_driver MCP tools for desktop observation and interaction. Do not
substitute shell, filesystem, web, or code-execution tools. Pass session
{session!r} to every Cua tool that accepts a session. Inspect state before each
action and verify state after it. Do not call start_session or end_session; the
host owns lifecycle cleanup. Do not perform purchases, send messages, delete
data, expose credentials, or take another irreversible action unless the task
explicitly requests that exact action. Return a concise result and mention any
step you could not verify.
"""


async def run(task: str) -> None:
    binary = driver_binary()
    scope = capture_scope()
    session = f"claude-agent-{uuid4().hex[:12]}"
    started = False

    try:
        await driver_call(
            binary,
            "start_session",
            {"session": session, "capture_scope": scope},
        )
        started = True

        options = ClaudeAgentOptions(
            model=os.environ.get("CLAUDE_MODEL"),
            cwd=Path.cwd(),
            tools=[],
            mcp_servers={
                "cua_driver": {
                    "type": "stdio",
                    "command": binary,
                    "args": ["mcp"],
                }
            },
            strict_mcp_config=True,
            allowed_tools=["mcp__cua_driver"],
            permission_mode="dontAsk",
            max_turns=40,
        )

        final_result: ResultMessage | None = None
        async with ClaudeSDKClient(options=options) as client:
            await client.query(agent_prompt(task, session, scope))
            async for message in client.receive_response():
                if isinstance(message, ResultMessage):
                    final_result = message

        if final_result is None:
            raise RuntimeError("Claude Agent SDK returned no final result")
        if final_result.is_error:
            raise RuntimeError(final_result.result or ", ".join(final_result.errors or []))
        print(final_result.result or "Claude completed the task without a text result.")
    finally:
        if started:
            await driver_call(binary, "end_session", {"session": session})


if __name__ == "__main__":
    arguments = parse_args()
    asyncio.run(run(arguments.task))
