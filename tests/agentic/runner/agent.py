"""Run the agent: Claude Code in headless mode, the pinned model, the Cua
Driver MCP server (through proxy.py) as its only tools, inside the budget.

The agent never decides the verdict. It reports what it did; the hard checks
run afterwards.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path

RUNNER_PARENT = Path(__file__).resolve().parents[1]  # tests/agentic

SYSTEM = """You are a QA tester. You use a real computer through the Cua Driver \
tools, the way a careful person would: read the screen, act, check the result. \
Your goal comes from a test; reach it through the app's own UI.

Rules:
- Use only what a user could use: the app windows and the terminal windows \
named in the goal. Never edit files, settings or the registry.
- If the app shows something wrong (an error, a dialog you did not expect, a \
step that does not work), do not work around it. Say exactly what you saw.
- Prefer acting by element_token from a fresh get_window_state. Batch with \
run_steps where you can. Do not loop on the same failing action more than twice.
- Terminals: bring the terminal window to the front, type the command, press \
Enter, then read the window again to check the command and its output appear. \
If they do not, try once more before going on.
- Never report `RESULT: done` for a step you could not confirm on screen.
- You have a step budget. When the goal is reached, or cannot be reached, stop.
- End with one line: `RESULT: done` or `RESULT: blocked: <what stopped you>`.

The computer: {os}. {context}"""


@dataclass
class AgentRun:
    ok: bool
    exit: str  # done | blocked | budget | timeout | error
    final_text: str = ""
    cost_usd: float = 0.0
    turns: int = 0
    steps: int = 0
    seconds: float = 0.0
    usage: dict = field(default_factory=dict)


def run_agent(
    *,
    prompt: str,
    context: str,
    os_name: str,
    model: str,
    effort: str,
    max_steps: int,
    minutes: float,
    run_dir: Path,
    driver_argv: list[str],
    record_dir: str,
    bare: bool,
    extra_env: dict[str, str] | None = None,
    record_video: bool = True,
) -> AgentRun:
    run_dir.mkdir(parents=True, exist_ok=True)
    proxy_cfg = run_dir / "proxy.json"
    proxy_cfg.write_text(
        json.dumps(
            {
                "run_dir": str(run_dir),
                "driver_argv": driver_argv,
                "max_steps": max_steps,
                "record_dir": record_dir,
                "record_video": record_video,
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    mcp_cfg = run_dir / "mcp.json"
    mcp_cfg.write_text(
        json.dumps(
            {
                "mcpServers": {
                    "cua-driver": {
                        "type": "stdio",
                        "command": sys.executable,
                        "args": ["-m", "runner.proxy", str(proxy_cfg)],
                        "env": {"PYTHONPATH": str(RUNNER_PARENT)},
                    }
                }
            },
            indent=2,
        ),
        encoding="utf-8",
    )

    claude = shutil.which("claude")
    if not claude:
        raise SystemExit(
            "the `claude` CLI is not on PATH (CI installs the version pinned in config.yaml)"
        )
    argv = [
        claude,
        "-p",
        prompt,
        "--model",
        model,
        "--effort",
        effort,
        "--output-format",
        "stream-json",
        "--verbose",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        str(mcp_cfg),
        "--allowedTools",
        "mcp__cua-driver",
        "--setting-sources",
        "",
        "--no-session-persistence",
        "--system-prompt",
        SYSTEM.format(os=os_name, context=context),
    ]
    if bare:
        argv.insert(1, "--bare")
    env = dict(os.environ)
    env.update(extra_env or {})
    # Never let the agent reach the runner's own secrets.
    for k in list(env):
        if k.startswith("CUA_E2E_CANARY"):
            env.pop(k)
    transcript = open(run_dir / "agent.jsonl", "w", encoding="utf-8")
    t0 = time.monotonic()
    proc = subprocess.Popen(
        argv,
        stdout=subprocess.PIPE,
        stderr=open(run_dir / "agent.stderr.log", "w"),
        stdin=subprocess.DEVNULL,
        text=True,
        encoding="utf-8",
        env=env,
        cwd=str(run_dir),
    )
    result: dict = {}
    timed_out = threading.Event()

    timer = threading.Timer(minutes * 60, lambda: (timed_out.set(), proc.kill()))
    timer.start()
    try:
        for line in proc.stdout:
            transcript.write(line)
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            if msg.get("type") == "result":
                result = msg
        proc.wait()
    finally:
        timer.cancel()
        transcript.close()
    seconds = time.monotonic() - t0

    steps = 0
    budget_hit = False
    calls = run_dir / "calls.jsonl"
    if calls.exists():
        for line in calls.read_text(encoding="utf-8").splitlines():
            e = json.loads(line)
            if "step" in e:
                steps = max(steps, e["step"])
            if e.get("why") == "budget":
                budget_hit = True

    text = str(result.get("result") or "")
    last = [ln for ln in text.strip().splitlines() if ln.strip().startswith("RESULT:")]
    if timed_out.is_set():
        exit_ = "timeout"
    elif budget_hit:
        exit_ = "budget"
    elif result.get("is_error") or proc.returncode not in (0, None):
        exit_ = "error"
    elif last and last[-1].strip().startswith("RESULT: done"):
        exit_ = "done"
    else:
        exit_ = "blocked"
    return AgentRun(
        ok=exit_ == "done",
        exit=exit_,
        final_text=text[-4000:],
        cost_usd=float(result.get("total_cost_usd") or 0.0),
        turns=int(result.get("num_turns") or 0),
        steps=steps,
        seconds=round(seconds, 1),
        usage=result.get("usage") or {},
    )
