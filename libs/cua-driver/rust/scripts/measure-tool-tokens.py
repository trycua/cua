#!/usr/bin/env python3
"""Per-tool token cost of `cua-driver mcp` tools/list and server instructions.

  measure-tool-tokens.py <cua-driver binary> [--profile core|standard|full|<groups>] [--rows]

Starts `cua-driver mcp --direct` (own runtime, no daemon), reads initialize and
tools/list, and counts tokens with tiktoken o200k_base. That is a PROXY for the
model tokenizer, not Anthropic's count_tokens; use it to compare builds, not as
an absolute bill. "visible" is what MCP clients forward to the model (name,
description, inputSchema); "wire" is the whole tools/list entry.

Needs: pip install tiktoken
"""
import argparse, json, os, subprocess, sys
import tiktoken

enc = tiktoken.get_encoding("o200k_base")
count = lambda s: len(enc.encode(s))
compact = lambda o: json.dumps(o, separators=(",", ":"))

ap = argparse.ArgumentParser()
ap.add_argument("binary")
ap.add_argument("--profile", help="sets CUA_DRIVER_TOOL_PROFILE")
ap.add_argument("--rows", action="store_true", help="print one line per tool")
args = ap.parse_args()

env = dict(os.environ)
if args.profile:
    env["CUA_DRIVER_TOOL_PROFILE"] = args.profile
proc = subprocess.Popen([args.binary, "mcp", "--direct"], stdin=subprocess.PIPE,
                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env)

def rpc(i, method, params=None):
    proc.stdin.write(compact({"jsonrpc": "2.0", "id": i, "method": method, "params": params or {}}) + "\n")
    proc.stdin.flush()
    while True:
        reply = json.loads(proc.stdout.readline())
        if reply.get("id") == i:
            return reply["result"]

init = rpc(1, "initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                              "clientInfo": {"name": "measure-tool-tokens", "version": "1"}})
tools = rpc(2, "tools/list")["tools"]
proc.stdin.close(); proc.terminate()

rows = [(t["name"], count(t["name"]), count(t.get("description", "")),
         count(compact(t.get("inputSchema", {}))), count(compact(t))) for t in tools]
if args.rows:
    print("tool,name,description,schema,visible,wire")
    for r in sorted(rows, key=lambda r: -(r[1] + r[2] + r[3])):
        print(f"{r[0]},{r[1]},{r[2]},{r[3]},{r[1] + r[2] + r[3]},{r[4]}")
visible = sum(r[1] + r[2] + r[3] for r in rows)
instructions = count(init.get("instructions", ""))
print(f"tools={len(tools)} visible_tokens={visible} wire_tokens={sum(r[4] for r in rows)} "
      f"instructions_tokens={instructions} visible_plus_instructions={visible + instructions}")
