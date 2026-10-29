#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Amendment 4 check (A4.2), no model call: start one CDB task's apps the way the runner does and show how
each Electron app appears to the system, to Cua Driver and to Codex computer use.

Run inside the benchmark VM, from a shell with the runner's launch.env sourced:
    check_named_electron.py CDB-G02 [out.json]

Prints one JSON object: for every Electron app of the descriptor, its bundle id and name from LaunchServices,
whether Cua Driver's list_apps shows it by that name, and what Codex computer use's cua.getApp(<name>)
returns. App approvals asked by the Codex server are accepted only for the task's own app names."""

from __future__ import annotations

import json
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
import recorder  # noqa: E402


def lsappinfo(pid: int) -> dict[str, str]:
    """Name, bundle id and bundle path that LaunchServices holds for a running pid."""
    out = subprocess.run(
        ["lsappinfo", "info", "-app", str(pid)], capture_output=True, text=True
    ).stdout
    info: dict[str, str] = {}
    first = out.splitlines()[0] if out.strip() else ""
    if first.startswith('"'):
        info["name"] = first.split('"')[1]
    for key in ("bundleID", "bundle path"):
        for line in out.splitlines():
            if line.strip().startswith(key + "="):
                info[key] = line.split("=", 1)[1].strip().strip('"')
    return info


def electron_pids() -> list[int]:
    out = subprocess.run(["pgrep", "-x", "Electron"], capture_output=True, text=True).stdout
    return [int(p) for p in out.split()]


def codex_get_apps(names: list[str], allowed: set[str], timeout: float = 60.0) -> dict:
    config = ca.resolve_codex_cu_config()
    if config is None:
        return {"error": "no Codex computer-use MCP config"}
    command, args, env = run_bench.mcp_entry(config)
    approvals: list[dict] = []

    def approve(text: str) -> dict:
        answer = run_bench.claude_driver.answer_elicitation(text, allowed)
        approvals.append({"message": text, "answer": answer["action"]})
        return answer

    client = recorder.McpStdioClient([command, *args], env, elicitation=approve)
    try:
        code = (
            f"const names = {json.dumps(names)}; const out = {{}};"
            "for (const n of names) { try { const a = await cua.getApp(n);"
            " out[n] = {ok: true, app: JSON.parse(JSON.stringify(a ?? null))}; }"
            " catch (e) { out[n] = {ok: false, error: String(e)}; } }"
            "let listed = null; try { listed = await cua.listApps({emit:false}); } catch (e) { listed = String(e); }"
            "nodeRepl.write(JSON.stringify({getApp: out, listApps: listed}));"
        )
        reply = client.request(
            "tools/call", {"name": "js", "arguments": {"code": code}}, timeout=timeout
        )
        texts = [c.get("text", "") for c in (reply.get("result") or {}).get("content", [])]
        data = {"raw": [t[:300] for t in texts]}
        for (
            text
        ) in texts:  # the reply also carries the API reference; find the object this code wrote
            at = text.find('{"getApp"')
            if at >= 0:
                try:
                    data = json.JSONDecoder().raw_decode(text[at:])[0]
                except ValueError:
                    continue
                break
        data["approvals"] = approvals
        return data
    finally:
        client.close()


def main() -> int:
    task_id = sys.argv[1]
    out_path = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    spec = json.loads((HERE.parent / "probes" / task_id / "task.json").read_text("utf-8"))
    art = Path(tempfile.mkdtemp(prefix="cdbnamed-"))
    rec = ca.start_cua_daemon(
        ca.RECORDER_SOCKET, ca.RECORDER_STATE, overlay=False, log_name="named.log"
    )
    task = cdb_adapter.CdbTask(spec, art)
    names = [cdb_adapter.app_display_name(a) for a in task.apps() if a.get("kind") == "electron"]
    report: dict = {"task": task_id, "electron_apps": names}
    try:
        task.reset()
        task.start_apps(windows=lambda win: run_bench.place_window(None, win))
        time.sleep(3)
        report["adapter_log"] = task.log
        report["launchservices"] = [{"pid": p, **lsappinfo(p)} for p in electron_pids()]
        listed = ca.cua_cli(
            "call",
            "list_apps",
            "{}",
            socket=ca.RECORDER_SOCKET,
            home=ca.RECORDER_STATE / "home",
            timeout=30,
        ).stdout
        report["cua_driver_list_apps_shows"] = {n: (n in listed) for n in names}
        report["cua_driver_list_apps_electron_named"] = listed.count('"Electron"')
        report["codex"] = codex_get_apps(
            names, run_bench.allowed_apps(run_bench.load_tasks(HERE.parent / "probes")[task_id])
        )
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
