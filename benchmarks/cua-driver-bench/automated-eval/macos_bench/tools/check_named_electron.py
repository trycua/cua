#!/usr/bin/env python3
"""Amendment 4 check (A4.2), no model call: start one CDB task's apps the way the runner does and show how
each Electron app appears to the system, to Cua Driver and to Codex computer use.

Run inside the benchmark VM, from a shell with the runner's launch.env sourced:
    check_named_electron.py CDB-G02 [out.json]

Prints one JSON object: for every Electron app of the descriptor, its bundle id and name from LaunchServices,
whether Cua Driver's list_apps shows it by that name, and what Codex computer use's cua.getApp(<name>)
returns. App approvals asked by the Codex server are accepted only for the task's own app names."""

from __future__ import annotations

import json
import select
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import cdb_adapter  # noqa: E402
import claude_arms as ca  # noqa: E402
import run_bench  # noqa: E402


def lsappinfo(pid: int) -> dict[str, str]:
    out = subprocess.run(
        ["lsappinfo", "info", "-only", "bundleid", "-only", "name", "-app", f"pid={pid}"],
        capture_output=True,
        text=True,
    ).stdout
    info = {}
    for line in out.splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            info[key.strip().strip('"').lower()] = value.strip().strip('"')
    return info


def electron_pids() -> list[int]:
    out = subprocess.run(["pgrep", "-x", "Electron"], capture_output=True, text=True).stdout
    return [int(p) for p in out.split()]


def codex_get_apps(names: list[str], allowed: set[str], timeout: float = 60.0) -> dict:
    config = ca.resolve_codex_cu_config()
    if config is None:
        return {"error": "no Codex computer-use MCP config"}
    command, args, env = run_bench.mcp_entry(config)
    proc = subprocess.Popen(
        [command, *args], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True, bufsize=1,
    )
    approvals: list[dict] = []

    def send(message: dict) -> None:
        assert proc.stdin
        proc.stdin.write(json.dumps(message) + "\n")
        proc.stdin.flush()

    def wait_for(msg_id: int, limit: float) -> dict | None:
        assert proc.stdout
        deadline = time.monotonic() + limit
        while time.monotonic() < deadline:
            ready, _, _ = select.select([proc.stdout], [], [], deadline - time.monotonic())
            if not ready:
                return None
            line = proc.stdout.readline()
            if not line:
                return None
            msg = json.loads(line)
            if msg.get("method") == "elicitation/create":
                text = (msg.get("params") or {}).get("message", "")
                answer = run_bench.claude_driver.answer_elicitation(text, allowed)
                approvals.append({"message": text, "answer": answer["action"]})
                send({"jsonrpc": "2.0", "id": msg["id"], "result": {"action": answer["action"], "content": {}}})
                continue
            if msg.get("id") == msg_id:
                return msg
        return None

    try:
        send({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {"elicitation": {}},
                       "clientInfo": {"name": "cdb-check-named-electron", "version": "1"}},
        })
        if wait_for(1, 30) is None:
            return {"error": "no initialize reply"}
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        code = (
            f"const names = {json.dumps(names)}; const out = {{}};"
            "for (const n of names) { try { const a = await cua.getApp(n);"
            " out[n] = {ok: true, app: JSON.parse(JSON.stringify(a ?? null))}; }"
            " catch (e) { out[n] = {ok: false, error: String(e)}; } }"
            "let listed = null; try { listed = await cua.listApps({emit:false}); } catch (e) { listed = String(e); }"
            "nodeRepl.write(JSON.stringify({getApp: out, listApps: listed}));"
        )
        send({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
              "params": {"name": "js", "arguments": {"code": code}}})
        reply = wait_for(2, timeout)
        if reply is None:
            return {"error": "js call timed out", "approvals": approvals}
        text = " ".join(c.get("text", "") for c in (reply.get("result") or {}).get("content", []))
        try:
            data = json.loads(text[text.index("{"):])
        except ValueError:
            data = {"raw": text[:2000]}
        data["approvals"] = approvals
        return data
    finally:
        proc.terminate()


def main() -> int:
    task_id = sys.argv[1]
    out_path = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    spec = json.loads((HERE.parent / "probes" / task_id / "task.json").read_text("utf-8"))
    art = Path(tempfile.mkdtemp(prefix="cdbnamed-"))
    rec = ca.start_cua_daemon(ca.RECORDER_SOCKET, ca.RECORDER_STATE, overlay=False, log_name="named.log")
    task = cdb_adapter.CdbTask(spec, art)
    names = [cdb_adapter.app_display_name(a) for a in task.apps() if a.get("kind") == "electron"]
    report: dict = {"task": task_id, "electron_apps": names}
    try:
        task.reset()
        task.start_apps(windows=lambda win: run_bench.place_window(None, win))
        time.sleep(3)
        report["adapter_log"] = task.log
        report["launchservices"] = [{"pid": p, **lsappinfo(p)} for p in electron_pids()]
        listed = ca.cua_cli("call", "list_apps", "{}", socket=ca.RECORDER_SOCKET,
                            home=ca.RECORDER_STATE / "home", timeout=30).stdout
        report["cua_driver_list_apps_shows"] = {n: (n in listed) for n in names}
        report["cua_driver_list_apps_electron_named"] = listed.count('"Electron"')
        report["codex"] = codex_get_apps(names, run_bench.allowed_apps(run_bench.load_tasks(HERE.parent / "probes")[task_id]))
    finally:
        task.stop_apps()
        task.clean_workspace()
        ca.stop_cua_daemon(ca.RECORDER_SOCKET, ca.RECORDER_STATE / "home")
        rec.terminate()
    text = json.dumps(report, indent=2, default=str)
    print(text)
    if out_path:
        out_path.write_text(text + "\n", "utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
