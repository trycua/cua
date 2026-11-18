#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Minimal stdlib MCP adapter for browser-use/windows-harness 0.2.2 (commit 46a37cf1d411c7816adffc274d5bdc2cca2bd9f0).

Same shape as the macOS adapter (bu_mcp.py): one tool carrying the vendor CLI syntax, the vendor skill as the server
instructions, and view_screenshot for image files the harness itself wrote. No shell: argv is parsed with shlex and only
the harness's own subcommands `apps`, `see`, `state`, `exec` and `skill` are run (`run` and `repl` need files or a
terminal and are not offered). Run it inside the interactive desktop session of the user the trial runs as.

Environment:
  WH_BIN   path of windows-harness.exe (default: `windows-harness` on PATH)
"""

import base64
import json
import os
import re
import shlex
import subprocess
import sys

WH = os.environ.get("WH_BIN") or "windows-harness"
ALLOWED = {"apps", "see", "state", "exec", "skill"}
NOWIN = 0x08000000
seen_images = set()


def run(argv, timeout=150):
    try:
        r = subprocess.run(
            [WH, *argv],
            capture_output=True,
            timeout=timeout,
            creationflags=NOWIN if os.name == "nt" else 0,
        )
        return r.returncode, (r.stdout + r.stderr).decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        return 124, "timed out"


SKILL = run(["skill"])[1]


def command(cmd):
    cmd = cmd.strip()
    if cmd.startswith("windows-harness"):
        cmd = cmd[len("windows-harness") :].strip()
    try:
        argv = shlex.split(cmd, posix=True)
    except ValueError as e:
        return {"content": [{"type": "text", "text": f"parse error: {e}"}], "isError": True}
    if not argv or argv[0] not in ALLOWED:
        return {
            "content": [
                {
                    "type": "text",
                    "text": f"allowed subcommands: {sorted(ALLOWED)} (run/repl need files or a terminal and are not offered)",
                }
            ],
            "isError": True,
        }
    rc, out = run(argv)
    for m in re.findall(r"[A-Za-z]:\\[^\s\"'<>|]+?\.(?:png|jpg|jpeg)", out):
        if os.path.isfile(m):
            seen_images.add(os.path.normcase(m))
    return {"content": [{"type": "text", "text": f"exit {rc}\n{out[-60000:]}"}], "isError": rc != 0}


def view(path):
    if os.path.normcase(path) not in seen_images:
        return {
            "content": [
                {
                    "type": "text",
                    "text": "only screenshots that windows-harness wrote can be viewed",
                }
            ],
            "isError": True,
        }
    data = open(path, "rb").read()
    mime = "image/png" if path.lower().endswith(".png") else "image/jpeg"
    return {
        "content": [{"type": "image", "data": base64.b64encode(data).decode(), "mimeType": mime}]
    }


TOOLS = [
    {
        "name": "windows_harness",
        "description": "Run one windows-harness CLI command, exactly as the skill shows, without the leading program name or with it: apps | see '<app>' | state '<app>' | exec '<python>' | skill. Python passed to exec has win, browser, Path and subprocess preloaded.",
        "inputSchema": {
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        },
    },
    {
        "name": "view_screenshot",
        "description": "Return a screenshot file that windows-harness wrote (a path it printed) as an image.",
        "inputSchema": {
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
        },
    },
]


def main():
    out = sys.stdout.buffer
    for raw in sys.stdin.buffer:
        try:
            msg = json.loads(raw)
        except Exception:
            continue
        mid, method = msg.get("id"), msg.get("method")
        if mid is None:
            continue
        if method == "initialize":
            res = {
                "protocolVersion": msg["params"].get("protocolVersion", "2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "windows-harness", "version": "bench-adapter"},
                "instructions": SKILL,
            }
        elif method == "tools/list":
            res = {"tools": TOOLS}
        elif method == "tools/call":
            p = msg["params"]
            a = p.get("arguments") or {}
            res = (
                command(a.get("command", ""))
                if p["name"] == "windows_harness"
                else view(a.get("path", ""))
            )
        else:
            out.write(
                (
                    json.dumps(
                        {
                            "jsonrpc": "2.0",
                            "id": mid,
                            "error": {"code": -32601, "message": "unknown"},
                        }
                    )
                    + "\n"
                ).encode()
            )
            out.flush()
            continue
        out.write((json.dumps({"jsonrpc": "2.0", "id": mid, "result": res}) + "\n").encode())
        out.flush()


main()
