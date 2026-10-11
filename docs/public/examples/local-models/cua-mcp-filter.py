#!/usr/bin/env python3
"""Expose an allowlisted subset of a Cua Driver MCP server over stdio.

This reduces the tool-schema context sent to a model. It is not an authorization
boundary; use Cua Driver permission policies to enforce tool access.

With --text-only, tool results also stay usable by a model that cannot accept
images: get_window_state is asked for the tree without a screenshot, and image
content blocks are replaced by a short text note.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import threading
from typing import Any


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
    parser.add_argument(
        "--text-only",
        action="store_true",
        help="Keep image content out of tool results for models without image input",
    )
    args = parser.parse_args()
    if not args.driver:
        parser.error("cua-driver was not found on PATH; pass --driver")
    return args


def is_image_block(block: Any) -> bool:
    if not isinstance(block, dict):
        return False
    if block.get("type") == "image":
        return True
    if block.get("type") == "resource_link":
        mime_type = block.get("mimeType")
    elif block.get("type") == "resource" and isinstance(block.get("resource"), dict):
        mime_type = block["resource"].get("mimeType")
    else:
        return False
    return str(mime_type or "").lower().startswith("image/")


def request_without_screenshot(message: dict[str, Any]) -> bool:
    params = message.get("params")
    if not isinstance(params, dict) or params.get("name") != "get_window_state":
        return False
    arguments = params.get("arguments")
    if not isinstance(arguments, dict):
        arguments = {}
        params["arguments"] = arguments
    if arguments.get("include_screenshot") is False:
        return False
    arguments["include_screenshot"] = False
    return True


def text_only_result(result: dict[str, Any]) -> bool:
    content = result.get("content")
    if not isinstance(content, list):
        return False
    kept = [block for block in content if not is_image_block(block)]
    omitted = len(content) - len(kept)
    if not omitted:
        return False
    kept.append(
        {
            "type": "text",
            "text": f"[{omitted} image block(s) omitted: this session is text-only]",
        }
    )
    result["content"] = kept
    return True


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
    pending_tool_calls: set[object] = set()
    pending_lock = threading.Lock()

    def parent_to_child() -> None:
        try:
            for line in sys.stdin.buffer:
                try:
                    message = json.loads(line)
                    if message.get("method") == "tools/list" and "id" in message:
                        with pending_lock:
                            pending_tool_lists.add(message["id"])
                    if args.text_only and message.get("method") == "tools/call" and "id" in message:
                        with pending_lock:
                            pending_tool_calls.add(message["id"])
                        if request_without_screenshot(message):
                            line = (json.dumps(message, separators=(",", ":")) + "\n").encode()
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
                    is_tool_call = message_id in pending_tool_calls
                    if is_tool_call:
                        pending_tool_calls.remove(message_id)
                result = message.get("result")
                tools = result.get("tools") if isinstance(result, dict) else None
                if is_tool_list and isinstance(tools, list):
                    message["result"]["tools"] = [
                        tool for tool in tools if tool.get("name") in allowed
                    ]
                    output = (json.dumps(message, separators=(",", ":")) + "\n").encode()
                elif is_tool_call and isinstance(result, dict) and text_only_result(result):
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
