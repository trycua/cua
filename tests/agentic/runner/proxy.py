"""The MCP server the agent sees: a pass-through to `cua-driver mcp` that

* starts Cua Driver's trajectory recording (start_recording, with video) on
  the agent's own session, so every action the agent takes is a turn folder,
  and stops it when the agent disconnects;
* logs every tool call and result to calls.jsonl, and every snapshot the
  agent read to snapshots.jsonl (replay maps element tokens back to rows);
* hides tools a test agent must not use (recording, replay, config, updates);
* enforces the step budget: once it is spent, every call is refused.

Run by the agent CLI from the MCP config agent.py writes:
    python -m runner.proxy <run-dir>/proxy.json
"""

from __future__ import annotations

import base64
import json
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

HIDDEN = {
    "start_recording",
    "stop_recording",
    "get_recording_state",
    "replay_trajectory",
    "set_config",
    "check_for_update",
    "install_ffmpeg",
    "install_extension",
    "set_agent_cursor_enabled",
    "set_agent_cursor_motion",
    "set_agent_cursor_theme",
    "escalate_session",
}


def main(config_path: str) -> int:
    cfg = json.loads(Path(config_path).read_text(encoding="utf-8"))
    run_dir = Path(cfg["run_dir"])
    shots = run_dir / "screenshots"
    shots.mkdir(parents=True, exist_ok=True)
    calls_log = open(run_dir / "calls.jsonl", "a", encoding="utf-8")
    snaps_log = open(run_dir / "snapshots.jsonl", "a", encoding="utf-8")
    max_steps = int(cfg["max_steps"])
    t0 = time.time()

    driver = subprocess.Popen(
        cfg["driver_argv"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=open(run_dir / "proxy-driver.log", "ab"),
        text=True,
        encoding="utf-8",
    )
    out_lock = threading.Lock()
    pending: dict = {}  # request id -> (tool, args, t)
    own: dict = {}  # our own request ids -> threading.Event + slot
    state = {"steps": 0, "shot": 0, "recording": False}

    def to_client(msg: dict) -> None:
        with out_lock:
            sys.stdout.write(json.dumps(msg) + "\n")
            sys.stdout.flush()

    driver_lock = threading.Lock()

    def to_driver(msg: dict) -> None:
        with driver_lock:
            driver.stdin.write(json.dumps(msg) + "\n")
            driver.stdin.flush()

    def own_call(tool: str, args: dict, timeout: float = 60) -> dict | None:
        rid = f"proxy-{tool}-{time.time_ns()}"
        ev = threading.Event()
        own[rid] = {"ev": ev, "msg": None}
        params = {"name": tool, "arguments": args}
        if state.get("meta"):
            # Stateless MCP clients send per-request metadata; ours must too.
            params["_meta"] = state["meta"]
        to_driver({"jsonrpc": "2.0", "id": rid, "method": "tools/call", "params": params})
        ev.wait(timeout)
        msg = own.pop(rid, {}).get("msg")
        log({"own": tool, "args": args, "result": _summary(msg)})
        return msg

    def finish(*_):
        if state.get("finished"):
            return
        state["finished"] = True
        if state["recording"]:
            own_call("stop_recording", {}, timeout=90)
        log({"end": True, "steps": state["steps"]})
        try:
            driver.stdin.close()
            driver.wait(timeout=15)
        except Exception:
            driver.kill()
        if _:
            os._exit(0)

    signal.signal(signal.SIGTERM, finish)
    if hasattr(signal, "SIGINT"):
        signal.signal(signal.SIGINT, finish)

    def log(entry: dict) -> None:
        entry["t"] = round(time.time() - t0, 3)
        calls_log.write(json.dumps(entry) + "\n")
        calls_log.flush()

    def save_images(result: dict, step: int) -> list[str]:
        names = []
        for part in result.get("content") or []:
            if part.get("type") == "image" and part.get("data"):
                state["shot"] += 1
                ext = "png" if "png" in part.get("mimeType", "png") else "jpg"
                name = f"{step:04d}-{state['shot']:04d}.{ext}"
                try:
                    (shots / name).write_bytes(base64.b64decode(part["data"]))
                    names.append(name)
                except ValueError:
                    pass
        return names

    def from_driver():
        for line in driver.stdout:
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            rid = msg.get("id")
            if rid in own:
                own[rid]["msg"] = msg
                own[rid]["ev"].set()
                continue
            if rid in pending:
                tool, args, step = pending.pop(rid)
                result = msg.get("result") or {}
                if tool == "tools/list":
                    tools = [t for t in result.get("tools") or [] if t.get("name") not in HIDDEN]
                    msg = {**msg, "result": {**result, "tools": tools}}
                else:
                    images = save_images(result, step)
                    structured = result.get("structuredContent") or {}
                    # Every snapshot the agent saw (get_window_state, a
                    # run_steps observation, ...), for token mapping in replay.
                    for snap in _snapshots(structured, args):
                        if not snap["tree"] and tool == "get_window_state":
                            snap["tree"] = _text(result)
                        snaps_log.write(json.dumps(snap) + "\n")
                    snaps_log.flush()
                    log(
                        {
                            "step": step,
                            "tool": tool,
                            "args": args,
                            "is_error": bool(msg.get("error")) or bool(result.get("isError")),
                            "text": _text(result)[:4000],
                            "structured": _trim(structured),
                            "images": images,
                        }
                    )
            to_client(msg)

    threading.Thread(target=from_driver, daemon=True).start()

    for line in sys.stdin:
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        method = msg.get("method")
        rid = msg.get("id")
        meta = (
            (msg.get("params") or {}).get("_meta") if isinstance(msg.get("params"), dict) else None
        )
        if meta:
            state["meta"] = meta
        if method == "tools/list" and rid is not None:
            pending[rid] = ("tools/list", {}, 0)
        elif method == "tools/call" and rid is not None:
            params = msg.get("params") or {}
            tool, args = params.get("name", ""), params.get("arguments") or {}
            if tool in HIDDEN:
                to_client(_refuse(rid, f"`{tool}` is not available in this test."))
                log({"refused": tool, "args": args, "why": "hidden"})
                continue
            if state["steps"] >= max_steps:
                to_client(
                    _refuse(
                        rid,
                        f"Step budget of {max_steps} tool calls is spent. Stop now and say what you finished.",
                    )
                )
                log({"refused": tool, "args": args, "why": "budget"})
                continue
            state["steps"] += 1
            pending[rid] = (tool, args, state["steps"])
            if not state["recording"]:
                # Before the agent's first call, on its own session (clients
                # may skip notifications/initialized: newer ones discover).
                state["recording"] = True
                own_call(
                    "start_recording",
                    {
                        "output_dir": cfg["record_dir"],
                        "record_video": bool(cfg.get("record_video", True)),
                    },
                )
        to_driver(msg)

    finish()
    return 0


def _snapshots(v, args: dict, depth: int = 0) -> list[dict]:
    out = []
    if depth > 6:
        return out
    if isinstance(v, dict):
        if v.get("snapshot_id") and isinstance(v.get("snapshot_id"), str):
            tree = v.get("tree_markdown") or ""
            if not tree and isinstance(v.get("elements"), list):
                # tree_format "elements": the same rows, written as markdown.
                tree = "\n".join(
                    f'- [{e.get("element_index")}] {e.get("role", "")} "{e.get("label") or ""}"'
                    for e in v["elements"]
                    if isinstance(e, dict) and "element_index" in e
                )
            out.append(
                {
                    "snapshot_id": v["snapshot_id"],
                    "pid": v.get("pid", args.get("pid")),
                    "window_id": v.get("window_id", args.get("window_id")),
                    "tree": tree,
                }
            )
        for x in v.values():
            out += _snapshots(x, args, depth + 1)
    elif isinstance(v, list):
        for x in v[:50]:
            out += _snapshots(x, args, depth + 1)
    return out


def _refuse(rid, text: str) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": rid,
        "result": {"isError": True, "content": [{"type": "text", "text": text}]},
    }


def _text(result: dict) -> str:
    return "\n".join(
        p.get("text", "") for p in result.get("content") or [] if p.get("type") == "text"
    )


def _summary(msg: dict | None) -> str:
    if not msg:
        return "no answer"
    if msg.get("error"):
        return json.dumps(msg["error"])[:500]
    return _text(msg.get("result") or {})[:500]


def _trim(v, depth=0):
    if isinstance(v, str):
        return v[:4000]
    if isinstance(v, list):
        return [_trim(x, depth + 1) for x in v[:200]]
    if isinstance(v, dict):
        return {
            k: _trim(x, depth + 1) for k, x in v.items() if k not in ("image", "screenshot_base64")
        }
    return v


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
