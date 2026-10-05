#!/usr/bin/env python3
"""Live preflight for the two arms' computer-use servers.

`codex-native-cu`: start the node_repl MCP server exactly as the isolated Codex home
configures it, then call `sky.list_apps()` once. Success means Codex Computer Use answered.
`cua-driver-mcp`: call `cua-driver status` (the permissioned daemon must be running).

Exit status 0 when the selected arm is usable. Output is a single JSON object.
"""

from __future__ import annotations

import json
import os
import select
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import arms  # noqa: E402


def _node_repl_env() -> dict[str, str]:
    node_dir = arms.CODEX_APP_RESOURCES / "cua_node"
    return {
        **{k: os.environ[k] for k in ("HOME", "USER", "TMPDIR") if k in os.environ},
        "PATH": arms.CLOSED_PATH,
        "NODE_REPL_NATIVE_PIPE_CONNECT_TIMEOUT_MS": "1000",
        "NODE_REPL_NODE_MODULE_DIRS": str(node_dir / "lib/node_modules"),
        "NODE_REPL_NODE_PATH": str(node_dir / "bin/node"),
        "NODE_REPL_TRUSTED_CODE_PATHS": str(node_dir / "lib/node_modules"),
        "NODE_REPL_TRUSTED_SERVICES": json.dumps({"sky": "@oai/sky/service"}),
        "SKY_CUA_SERVICE_PATH": str(arms.CODEX_CU_APP),
    }


def codex_cu_live_check(timeout: float = 40.0) -> tuple[bool, str]:
    node_repl = arms.CODEX_APP_RESOURCES / "cua_node/bin/node_repl"
    if not node_repl.exists():
        return False, f"node_repl missing: {node_repl}"
    proc = subprocess.Popen(
        [str(node_repl)],
        env=_node_repl_env(),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        bufsize=1,
    )

    def send(message: dict) -> None:
        assert proc.stdin
        proc.stdin.write(json.dumps(message) + "\n")
        proc.stdin.flush()

    def recv(wait: float) -> dict | None:
        assert proc.stdout
        ready, _, _ = select.select([proc.stdout], [], [], wait)
        return json.loads(proc.stdout.readline()) if ready else None

    try:
        send(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "cdb-pilot-preflight", "version": "1"},
                },
            }
        )
        if recv(20) is None:
            return False, "node_repl did not answer initialize"
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        code = (
            'globalThis.sky = (await import("@oai/sky")).sky;'
            "const apps = await sky.list_apps();"
            "nodeRepl.write(String(apps.length));"
        )
        send(
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "js", "arguments": {"code": code}},
            }
        )
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            reply = recv(deadline - time.monotonic())
            if reply and reply.get("id") == 2:
                result = reply.get("result", {})
                text = " ".join(c.get("text", "") for c in result.get("content", []))
                return (not result.get("isError"), text[:300])
        return False, f"sky.list_apps() did not answer within {timeout:.0f}s"
    finally:
        proc.terminate()


def cua_driver_live_check() -> tuple[bool, str]:
    command = [str(arms.CUA_DRIVER_BIN), "status"]
    if arms.CUA_DRIVER_SOCKET:
        command += ["--socket", arms.CUA_DRIVER_SOCKET]
    done = subprocess.run(command, capture_output=True, text=True, timeout=20)
    ok = done.returncode == 0 and "daemon is running" in done.stdout
    return ok, done.stdout.strip().splitlines()[0] if done.stdout.strip() else done.stderr[:200]


def live_check(arm: str) -> tuple[bool, str]:
    return codex_cu_live_check() if arm == "codex-native-cu" else cua_driver_live_check()


if __name__ == "__main__":
    selected = sys.argv[1] if len(sys.argv) > 1 else "codex-native-cu"
    ok, detail = live_check(selected)
    print(json.dumps({"arm": selected, "ok": ok, "detail": detail}))
    raise SystemExit(0 if ok else 1)
