#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Stdio filter between Claude Code and OpenAI's computer-use `cua_repl` MCP server (bench arm `cc-codex-cu`).

OpenAI's plugin is written for the ChatGPT/Codex app as its client. Claude Code is a different client, so four
differences are bridged here and nothing else is touched (no tool is added, removed or reworded, except item 4):

1. `tools/call` gets the `codex/sandbox-state-meta` metadata the plugin expects (`sandboxCwd`, and a disabled
   permission profile because the bench machine is the sandbox). Claude Code sends no such metadata.
2. `elicitation/create` is the plugin's per-app approval prompt. It is accepted only for the exact text
   `Allow Computer Use to use "<App>"?` (or `Allow Codex to use ...`) where <App> is on the allowlist, and declined
   otherwise. No human answers during a trial.
3. `server/discover` (sent first by newer Claude Code builds) is answered locally with -32601 so the plugin's
   Rust MCP layer, which aborts on any first message but `initialize`, never sees it.
4. Windows app 26.1029 only: the `js` tool's input schema is a top-level `oneOf`, which the Anthropic API rejects, so
   Claude Code drops the tool. The schema is flattened to one object with the same properties, none required.

Usage:
  codex_cu_filter.py --allow "Calculator,Finder" [--cwd DIR] [--log FILE] -- <server command> [args...]
  codex_cu_filter.py --allow "BenchLab" [--cwd DIR] [--log FILE] --windows-app "<...>\\resources"

`--windows-app` builds the server command and environment for the Windows ChatGPT/Codex app package from its
`resources` directory (cua_node\\bin\\node.exe running @oai\\cua-repl\\bin\\cua-repl.mjs). On macOS use the server entry
from the plugin's own `.mcp.json` (see make_codex_cu_mcp.py) as the command after `--`.
No credentials are read here; the plugin uses the ChatGPT sign-in of the user the trial runs as.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

KEY = "codex/sandbox-state-meta"
ELICIT_APP = re.compile(r'Allow (?:Computer Use|Codex) to use "?([^"?]+)"?\?')


class Filter:
    def __init__(self, allow: set[str], cwd: str, log_path: str | None = None):
        self.allow = {a.strip().lower() for a in allow if a.strip()}
        self.cwd = cwd
        self.log_path = log_path

    def log(self, message: str) -> None:
        if self.log_path:
            with open(self.log_path, "a", encoding="utf-8") as f:
                f.write(time.strftime("%H:%M:%S ") + message + "\n")

    def cwd_uri(self) -> str:
        return Path(self.cwd).absolute().as_uri()

    def rewrite_up(self, line: bytes) -> bytes:
        """Client -> server: inject the sandbox metadata on tools/call."""
        if b'"tools/call"' not in line:
            return line
        try:
            msg = json.loads(line)
            if msg.get("method") == "tools/call":
                meta = msg.setdefault("params", {}).setdefault("_meta", {}).setdefault(KEY, {})
                meta["sandboxCwd"] = self.cwd_uri()
                meta["permissionProfile"] = {"type": "disabled"}
                return json.dumps(msg, separators=(",", ":")).encode() + b"\n"
        except ValueError as error:
            self.log(f"inject failed {error!r}")
        return line

    def discover_reply(self, line: bytes) -> bytes | None:
        if b"server/discover" not in line:
            return None
        try:
            msg = json.loads(line)
        except ValueError:
            return None
        if msg.get("method") != "server/discover" or "id" not in msg:
            return None
        self.log("server/discover answered -32601")
        reply = {
            "jsonrpc": "2.0",
            "id": msg["id"],
            "error": {"code": -32601, "message": "Method not found"},
        }
        return json.dumps(reply, separators=(",", ":")).encode() + b"\n"

    def elicit_reply(self, line: bytes) -> bytes | None:
        """Server -> client elicitation, answered here. Returns the reply for the server, or None."""
        if b"elicitation/create" not in line:
            return None
        try:
            msg = json.loads(line)
        except ValueError:
            return None
        if msg.get("method") != "elicitation/create" or "id" not in msg:
            return None
        text = (msg.get("params") or {}).get("message") or ""
        found = ELICIT_APP.match(text)
        ok = bool(found) and found.group(1).strip().lower() in self.allow
        result = (
            {"action": "accept", "content": {}, "_meta": {"persist": "session"}}
            if ok
            else {"action": "decline"}
        )
        self.log(f"elicitation {text!r} -> {result['action']}")
        return (
            json.dumps(
                {"jsonrpc": "2.0", "id": msg["id"], "result": result}, separators=(",", ":")
            ).encode()
            + b"\n"
        )

    def flatten_down(self, line: bytes) -> bytes:
        """Server -> client: flatten a top-level oneOf input schema in a tools/list result."""
        if b'"oneOf"' not in line or b'"tools"' not in line:
            return line
        try:
            msg = json.loads(line)
            changed = False
            for tool in (msg.get("result") or {}).get("tools") or []:
                schema = tool.get("inputSchema") or {}
                if "oneOf" in schema:
                    props = dict(schema.get("properties") or {})
                    for branch in schema["oneOf"]:
                        props.update(branch.get("properties") or {})
                    tool["inputSchema"] = {
                        "type": "object",
                        "additionalProperties": False,
                        "properties": props,
                    }
                    changed = True
                    self.log(f"flattened oneOf schema of {tool.get('name')}")
            if changed:
                return json.dumps(msg, separators=(",", ":")).encode() + b"\n"
        except ValueError as error:
            self.log(f"flatten failed {error!r}")
        return line


def windows_command(
    resources: str, home: str | None = None, work: str | None = None
) -> tuple[list[str], dict[str, str]]:
    """Server command and environment for the Windows ChatGPT/Codex app, from its `resources` directory."""
    res = Path(resources)
    node = res / "cua_node" / "bin"
    mods = node / "node_modules"
    user = Path(home or os.environ.get("USERPROFILE") or Path.home())
    env = dict(os.environ)
    env.update(
        {
            "NODE_REPL_NATIVE_PIPE_CONNECT_TIMEOUT_MS": "1000",
            "NODE_REPL_NODE_MODULE_DIRS": str(mods),
            "NODE_REPL_NODE_PATH": str(node / "node.exe"),
            "NODE_REPL_TRUSTED_CODE_PATHS": f"{user / '.codex'};{mods}",
            "CODEX_HOME": str(user / ".codex"),
            "NODE_REPL_INSTRUCTIONS_USE_CASE_COMPUTER_USE": "Control desktop apps on Windows through Computer Use.",
            "NODE_REPL_TRUSTED_SERVICES": json.dumps({"sky": "@oai/sky/service"}),
            "SKY_CUA_SERVICE_PATH": str(
                mods / "@oai" / "sky" / "bin" / "windows" / "codex-computer-use.exe"
            ),
            "CODEX_CLI_PATH": str(res / "codex.exe"),
            "CUA_REPL_NODE_REPL_PATH": str(node / "node_repl.exe"),
            "CUA_REPL_ENABLED_SURFACES": "computer",
        }
    )
    return [str(node / "node.exe"), str(mods / "@oai" / "cua-repl" / "bin" / "cua-repl.mjs")], env


def run(flt: Filter, command: list[str], env: dict[str, str] | None, cwd: str) -> int:
    Path(cwd).mkdir(parents=True, exist_ok=True)
    proc = subprocess.Popen(
        command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, cwd=cwd, bufsize=0
    )
    flt.log(f"spawned pid={proc.pid} allow={sorted(flt.allow)}")
    write_lock, out_lock = threading.Lock(), threading.Lock()

    def down() -> None:
        out = sys.stdout.buffer
        for line in iter(proc.stdout.readline, b""):
            reply = flt.elicit_reply(line)
            if reply is not None:
                with write_lock:
                    proc.stdin.write(reply)
                    proc.stdin.flush()
                continue
            with out_lock:
                out.write(flt.flatten_down(line))
                out.flush()

    thread = threading.Thread(target=down, daemon=True)
    thread.start()
    try:
        for line in iter(sys.stdin.buffer.readline, b""):
            reply = flt.discover_reply(line)
            if reply is not None:
                with out_lock:
                    sys.stdout.buffer.write(reply)
                    sys.stdout.buffer.flush()
                continue
            with write_lock:
                proc.stdin.write(flt.rewrite_up(line))
                proc.stdin.flush()
    except OSError:
        pass
    try:
        proc.stdin.close()
    except OSError:
        pass
    code = proc.wait()
    thread.join(timeout=5)
    flt.log(f"exit {code}")
    return code


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    command: list[str] = []
    if "--" in argv:
        split = argv.index("--")
        argv, command = argv[:split], argv[split + 1 :]
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument(
        "--allow", default="", help="comma-separated apps whose approval prompt is accepted"
    )
    ap.add_argument("--cwd", default=str(Path.home() / ".cache" / "cua-bench-h2h" / "codex-cu-cwd"))
    ap.add_argument("--log")
    ap.add_argument("--windows-app", help="`resources` directory of the Windows ChatGPT/Codex app")
    args = ap.parse_args(argv)
    env = None
    if args.windows_app:
        command, env = windows_command(args.windows_app)
    if not command:
        ap.error("give a server command after `--` or --windows-app")
    flt = Filter(set(args.allow.split(",")), args.cwd, args.log)
    return run(flt, command, env, args.cwd)


if __name__ == "__main__":
    sys.exit(main())
