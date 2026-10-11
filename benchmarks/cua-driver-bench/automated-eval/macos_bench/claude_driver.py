"""Run one `claude -p` trial through the stream-json host protocol.

Both arms use this same host. The Codex computer-use server asks the host for per-app approval
through MCP elicitation requests; the host answers from a per-task allowlist of app names and
declines everything else. Cua Driver never asks (the daemon runs with approvals bypassed), so
for arm A the host loop only forwards the prompt and records events. Any other host question
(for example a tool permission prompt) is answered with an error so nothing blocks.
"""

from __future__ import annotations

import json
import os
import re
import signal
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

import claude_arms as ca

ELICIT_APP = re.compile(r'Allow Computer Use to use "(.+)"\?')


def kill_group(pid: int, grace: float = 1.0) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pid, sig)
        except OSError:
            return
        time.sleep(grace)


def answer_elicitation(message: str, allowed_apps: set[str]) -> dict[str, Any]:
    """Decision for one MCP elicitation request. Only an exact `Allow Computer Use to use "<App>"?`
    for an app on the task's allowlist is accepted."""
    match = ELICIT_APP.match(message or "")
    ok = bool(match) and match.group(1).strip().lower() in allowed_apps
    if ok:
        return {"action": "accept", "content": {}, "_meta": {"persist": "session"}}
    return {"action": "decline"}


def run_claude(
    argv: list[str],
    env: dict[str, str],
    cwd: Path,
    prompt: str,
    allowed_apps: set[str],
    timeout_s: float,
    stream_path: Path,
    stderr_path: Path,
    elicitation_path: Path,
    post_result_grace_s: float = 20.0,
    abort: threading.Event | None = None,
) -> dict[str, Any]:
    """Launch claude in its own process group, feed the prompt, log every stdout line with an epoch
    millisecond stamp, and stop at the result event or at the wall-time limit."""
    started = time.monotonic()
    started_wall = time.time()
    allowed = {a.strip().lower() for a in allowed_apps}
    state: dict[str, Any] = {"result_seen": False, "elicitations": [], "lines": 0}
    done = threading.Event()
    with (
        stream_path.open("w", encoding="utf-8") as stream,
        stderr_path.open("wb") as err,
        elicitation_path.open("w", encoding="utf-8") as elog,
    ):
        token_fd = ca.open_token_fd()
        if token_fd is not None:
            env = {**env, "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR": str(token_fd)}
        try:
            proc = subprocess.Popen(
                argv,
                cwd=str(cwd),
                env=env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=err,
                text=True,
                bufsize=1,
                start_new_session=True,
                pass_fds=(token_fd,) if token_fd is not None else (),
            )
        finally:
            if token_fd is not None:
                os.close(token_fd)
        lock = threading.Lock()

        def send(obj: dict[str, Any]) -> None:
            with lock:
                try:
                    assert proc.stdin is not None
                    proc.stdin.write(json.dumps(obj) + "\n")
                    proc.stdin.flush()
                except (OSError, ValueError):
                    pass

        def pump() -> None:
            assert proc.stdout is not None
            for line in proc.stdout:
                line = line.rstrip("\n")
                if not line.strip():
                    continue
                stream.write(f"{time.time() * 1000:.1f}\t{line}\n")
                stream.flush()
                state["lines"] += 1
                try:
                    obj = json.loads(line)
                except json.JSONDecodeError:
                    continue
                kind = obj.get("type")
                if kind == "control_request":
                    request = obj.get("request") or {}
                    if request.get("subtype") == "elicitation":
                        message = request.get("message", "")
                        reply = answer_elicitation(message, allowed)
                        record = {"ts": time.time(), "message": message, "answer": reply["action"]}
                        state["elicitations"].append(record)
                        elog.write(json.dumps(record) + "\n")
                        elog.flush()
                        send(
                            {
                                "type": "control_response",
                                "response": {
                                    "subtype": "success",
                                    "request_id": obj.get("request_id"),
                                    "response": reply,
                                },
                            }
                        )
                    else:
                        record = {
                            "ts": time.time(),
                            "unhandled": request.get("subtype"),
                            "tool": request.get("tool_name"),
                        }
                        state["elicitations"].append(record)
                        elog.write(json.dumps(record) + "\n")
                        elog.flush()
                        send(
                            {
                                "type": "control_response",
                                "response": {
                                    "subtype": "error",
                                    "request_id": obj.get("request_id"),
                                    "error": "not handled by the benchmark host",
                                },
                            }
                        )
                elif kind == "result":
                    state["result_seen"] = True
                    done.set()
                    try:
                        assert proc.stdin is not None
                        proc.stdin.close()
                    except (OSError, ValueError):
                        pass

        reader = threading.Thread(target=pump, daemon=True)
        reader.start()
        send({"type": "user", "message": {"role": "user", "content": prompt}})
        timed_out = False
        aborted = False
        result_at: float | None = None
        while proc.poll() is None:
            now = time.monotonic()
            if done.is_set():
                result_at = result_at or now
                if now - result_at > post_result_grace_s:
                    kill_group(proc.pid)
                    break
            if now - started > timeout_s:
                timed_out = True
                kill_group(proc.pid)
                break
            if abort is not None and abort.is_set():
                aborted = True
                kill_group(proc.pid)
                break
            time.sleep(0.25)
        reader.join(timeout=5)
        # A killed claude can leave MCP children in the group; make sure none survive the trial.
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            pass
    return {
        "returncode": proc.returncode,
        "timed_out": timed_out,
        "aborted": aborted,
        "result_seen": state["result_seen"],
        "wall_s": round(time.monotonic() - started, 2),
        "started_wall": started_wall,
        "elicitations": state["elicitations"],
        "lines": state["lines"],
    }
