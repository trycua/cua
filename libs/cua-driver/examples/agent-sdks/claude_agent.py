"""Run a Claude Agent SDK desktop task through Cua Driver native tools or MCP."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
from pathlib import Path
from typing import Literal, cast
from uuid import uuid4

from claude_agent_sdk import (
    ClaudeAgentOptions,
    ClaudeSDKClient,
    ResultMessage,
    create_sdk_mcp_server,
    tool,
)

from native_tools import NativeDesktopTools

CaptureScope = Literal["auto", "window", "desktop"]
Route = Literal["native", "mcp"]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("task", help="Trusted desktop task for Claude to perform")
    parser.add_argument(
        "--route",
        choices=("native", "mcp"),
        default="mcp",
        help="same-process SDK callbacks or the external-agent MCP boundary",
    )
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


async def driver_call(binary: str, name: str, arguments: dict[str, str]) -> None:
    process = await asyncio.create_subprocess_exec(
        binary,
        "call",
        name,
        json.dumps(arguments),
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    try:
        stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=30)
    except TimeoutError:
        process.kill()
        await process.wait()
        raise RuntimeError(f"cua-driver call {name} timed out") from None
    if process.returncode != 0:
        detail = stderr.decode().strip() or stdout.decode().strip()
        raise RuntimeError(f"cua-driver call {name} failed: {detail}")


def prompt(task: str, route: Route, session: str | None = None) -> str:
    lifecycle = (
        f"The host already started session {session!r}. Pass it to every Cua "
        "tool that accepts a session; do not call start_session or end_session."
        if session
        else "The host owns the native driver session; the custom tools do not expose lifecycle."
    )
    return f"""Complete this trusted desktop task through Cua Driver:

{task}

Route: {route}. {lifecycle}
Use only the supplied Cua tools for desktop observation and interaction. Inspect
state before each action and verify state after it. A timeout after a mutation
means the outcome is unknown: observe before retrying and never blindly replay.
Do not purchase, send, delete, expose credentials, or perform another
irreversible action unless the task explicitly requests that exact action.
Return a concise result and mention any step you could not verify.
"""


def native_server(runtime: NativeDesktopTools):
    @tool(
        "observe_desktop",
        "Capture the whole desktop. Use before every action and to verify its result.",
        {},
    )
    async def observe_desktop(_args):
        return await runtime.observe()

    @tool(
        "click_desktop",
        "Click an absolute desktop coordinate grounded in the latest screenshot.",
        {"x": float, "y": float},
    )
    async def click_desktop(args):
        return await runtime.click(args["x"], args["y"])

    @tool(
        "type_text",
        "Type text into the currently focused desktop control, then re-observe.",
        {"text": str},
    )
    async def type_text(args):
        return await runtime.type_text(args["text"])

    @tool(
        "press_key",
        "Press one named key in the desktop session, then re-observe.",
        {"key": str},
    )
    async def press_key(args):
        return await runtime.press_key(args["key"])

    return create_sdk_mcp_server(
        name="cua_native",
        version="1.0.0",
        tools=[observe_desktop, click_desktop, type_text, press_key],
    )


async def run_agent(options: ClaudeAgentOptions, task_prompt: str) -> None:
    final_result: ResultMessage | None = None
    async with ClaudeSDKClient(options=options) as client:
        await client.query(task_prompt)
        async for message in client.receive_response():
            if isinstance(message, ResultMessage):
                final_result = message
    if final_result is None:
        raise RuntimeError("Claude Agent SDK returned no final result")
    if final_result.is_error:
        raise RuntimeError(final_result.result or ", ".join(final_result.errors or []))
    print(final_result.result or "Claude completed the task without a text result.")


async def run_native(task: str) -> None:
    runtime = NativeDesktopTools()
    try:
        await runtime.start()
        options = ClaudeAgentOptions(
            model=os.environ.get("CLAUDE_MODEL"),
            cwd=Path.cwd(),
            tools=[],
            mcp_servers={"cua_native": native_server(runtime)},
            strict_mcp_config=True,
            allowed_tools=[
                "mcp__cua_native__observe_desktop",
                "mcp__cua_native__click_desktop",
                "mcp__cua_native__type_text",
                "mcp__cua_native__press_key",
            ],
            permission_mode="dontAsk",
            max_turns=40,
        )
        await run_agent(options, prompt(task, "native"))
    finally:
        await runtime.close()


async def run_mcp(task: str) -> None:
    binary = driver_binary()
    scope = capture_scope()
    session = f"claude-mcp-{uuid4().hex[:12]}"
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
            # Claude's MCP allowlist accepts the server name to enable all
            # tools from that server; wildcard tool-name globs are unsupported.
            allowed_tools=["mcp__cua_driver"],
            permission_mode="dontAsk",
            max_turns=40,
        )
        await run_agent(options, prompt(task, "mcp", session))
    finally:
        if started:
            await driver_call(binary, "end_session", {"session": session})


async def main() -> None:
    args = parse_args()
    if args.route == "native":
        await run_native(args.task)
    else:
        await run_mcp(args.task)


if __name__ == "__main__":
    asyncio.run(main())
