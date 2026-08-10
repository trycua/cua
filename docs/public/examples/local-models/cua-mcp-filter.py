#!/usr/bin/env python3
"""Expose an allowlisted subset of a Cua Driver MCP server over stdio.

This reduces the tool-schema context sent to a model. It is not an authorization
boundary; use Cua Driver permission policies to enforce tool access.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import threading


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--allow",
        required=True,
        help="Comma-separated Cua Driver tool names to advertise",
    )
    parser.add_argument(
        "--driver",
        default=shutil.which("cua-driver"),
        help="Path to cua-driver or a wrapper that accepts the mcp argument",
    )
    args = parser.parse_args()
    if not args.driver:
        parser.error("cua-driver was not found on PATH; pass --driver")
    return args


def main() -> int:
    args = parse_args()
    allowed = {name.strip() for name in args.allow.split(",") if name.strip()}
    child = subprocess.Popen(
        [args.driver, "mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        bufsize=0,
    )
    assert child.stdin is not None
    assert child.stdout is not None
    assert child.stderr is not None

    pending_tool_lists: set[object] = set()
    pending_lock = threading.Lock()

    def parent_to_child() -> None:
        try:
            for line in sys.stdin.buffer:
                try:
                    message = json.loads(line)
                    if message.get("method") == "tools/list" and "id" in message:
                        with pending_lock:
                            pending_tool_lists.add(message["id"])
                except (json.JSONDecodeError, AttributeError):
                    pass
                child.stdin.write(line)
                child.stdin.flush()
        except (BrokenPipeError, OSError):
            pass
        finally:
            try:
                child.stdin.close()
            except OSError:
                pass

    def forward_stderr() -> None:
        for chunk in iter(lambda: child.stderr.read(8192), b""):
            sys.stderr.buffer.write(chunk)
            sys.stderr.buffer.flush()

    threading.Thread(target=parent_to_child, daemon=True).start()
    threading.Thread(target=forward_stderr, daemon=True).start()

    try:
        for line in child.stdout:
            output = line
            try:
                message = json.loads(line)
                message_id = message.get("id")
                with pending_lock:
                    is_tool_list = message_id in pending_tool_lists
                    if is_tool_list:
                        pending_tool_lists.remove(message_id)
                tools = message.get("result", {}).get("tools")
                if is_tool_list and isinstance(tools, list):
                    message["result"]["tools"] = [
                        tool for tool in tools if tool.get("name") in allowed
                    ]
                    output = (json.dumps(message, separators=(",", ":")) + "\n").encode()
            except (json.JSONDecodeError, AttributeError, TypeError):
                pass
            sys.stdout.buffer.write(output)
            sys.stdout.buffer.flush()
    except KeyboardInterrupt:
        child.terminate()

    return child.wait()


if __name__ == "__main__":
    raise SystemExit(main())
