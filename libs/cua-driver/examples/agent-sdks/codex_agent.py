"""Run a Codex Python SDK desktop task through Cua Driver's MCP boundary."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
from typing import Literal, cast
from uuid import uuid4

from openai_codex import AsyncCodex, CodexConfig, Sandbox

CaptureScope = Literal["auto", "window", "desktop"]


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


async def main() -> None:
    args = parse_args()
    binary = driver_binary()
    scope = capture_scope()
    session = f"codex-python-{uuid4().hex[:12]}"
    started = False

    try:
        await driver_call(
            binary,
            "start_session",
            {"session": session, "capture_scope": scope},
        )
        started = True
        config = CodexConfig(
            config_overrides=(
                f'mcp_servers.cua_driver.command={json.dumps(binary)}',
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

The host already started session {session!r} with capture scope {scope!r}.
Use only cua_driver MCP tools for desktop observation and interaction. Pass
session {session!r} to every tool that accepts a session. Inspect before each
action and verify afterward. Do not call start_session or end_session. If a
mutation times out, observe before any retry; never blindly replay an action
with an unknown outcome. Return a concise result and name anything unverified.
"""
            )
            print(result.final_response)
    finally:
        if started:
            await driver_call(binary, "end_session", {"session": session})


if __name__ == "__main__":
    asyncio.run(main())
