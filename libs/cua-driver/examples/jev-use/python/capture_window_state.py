"""Record a sanitized ``get_window_state`` fixture for the native unit tests.

The native source's role table and element rules are tested against real
Driver output from each harness (RFC #4268). This helper attaches to a running
harness (``--pid``), finds its task window by title, takes one observation
with the tree and screenshot together, exactly as ``run_native.py`` does, and
writes the structured result without screenshot bytes, ``tree_markdown``, or
local file paths. ``verify_native.py --capture-dir`` calls it around each run.
"""

from __future__ import annotations

import argparse
import asyncio
import uuid
import json
import os
from pathlib import Path
from typing import Any

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

from driver_env import driver_environment
from run import Driver
from run_native import find_window

DROPPED_KEYS = frozenset({"tree_markdown", "screenshot_file_path", "screenshot_out_file"})
MAX_STRING_CHARS = 4096


def sanitize(payload: dict[str, Any], source: str) -> dict[str, Any]:
    result: dict[str, Any] = {
        "_fixture": {
            "source": source,
            "sanitized": "Screenshot bytes, tree_markdown, and local file paths removed.",
        }
    }
    for key, value in payload.items():
        if key in DROPPED_KEYS or (isinstance(value, str) and len(value) > MAX_STRING_CHARS):
            continue
        result[key] = value
    return result


async def capture(pid: int, title: str, source: str, max_depth: int | None = None) -> dict[str, Any]:
    params = StdioServerParameters(
        command=os.getenv("CUA_DRIVER_BIN", "cua-driver"), args=["mcp"], env=driver_environment()
    )
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            driver = Driver(session, f"jev-native-capture-{uuid.uuid4().hex[:8]}")
            window = await find_window(driver, pid, title)
            payload = await driver.call(
                "get_window_state",
                {
                    "pid": pid,
                    "window_id": int(window["window_id"]),
                    "include_accessibility_tree": True,
                    "include_screenshot": True,
                    **({"max_depth": max_depth} if max_depth is not None else {}),
                },
            )
            return sanitize(payload, source)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--title", required=True, help="exact task window title")
    parser.add_argument("--source", required=True, help="fixture provenance, e.g. harness and Driver version")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--max-depth", type=int, help="the task scope's walk depth, if it sets one")
    args = parser.parse_args()
    state = asyncio.run(capture(args.pid, args.title, args.source, args.max_depth))
    args.output.write_text(json.dumps(state, indent=1, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
