#!/usr/bin/env python3
"""Harness X in image Y: run a coding agent over a task file in a sandbox.

    # start one sandbox from any image and launch every task, fire-and-forget
    python agents_in_sandboxes.py launch --image ghcr.io/trycua/linux:24.04 \
        --harness claude-code --tasks tasks.jsonl --key-var ANTHROPIC_API_KEY

    # later (any process, any machine that can reach the sandbox): verify and report
    python agents_in_sandboxes.py collect --state runs.json --out report.json

    # both, waiting in between
    python agents_in_sandboxes.py run --image ... --harness ... --tasks tasks.jsonl

Every run gets the sandbox's own MCP tools (cua-driver) unless
--no-sandbox-mcp, plus any --mcp NAME=URL servers. --base-url points the
harness at a proxy or a compatible server (for example the scripted mock
provider, cua-mock-llm); --scripted appends each task's `mock` directive to
its prompt so the mock provider knows what to do.

The pure helpers (task parsing, MCP flags, the report) have no SDK import
and are unit-tested in tests/.
"""

from __future__ import annotations

import argparse
import asyncio
import datetime as _dt
import json
import os
import re
import shlex
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterable, Optional

TASK_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$")
DEFAULT_IMAGE = "ghcr.io/trycua/linux:24.04"
DEFAULT_WORKDIR = "/tmp/agents-in-sandboxes"


# ── pure logic ──────────────────────────────────────────────────────────────


@dataclass(frozen=True)
class Task:
    """One task: what the agent is asked, and how the result is checked."""

    id: str
    prompt: str
    check: Optional[str] = None  # shell, run in the task's directory; exit 0 passes
    mock: Optional[str] = None  # cua-mock-llm script, used with --scripted
    timeout_s: int = 900

    def prompt_for(self, scripted: bool) -> str:
        if scripted and self.mock:
            return f"{self.prompt} mock: {self.mock}"
        return self.prompt


def parse_tasks(text: str) -> list[Task]:
    """Tasks from JSON Lines (one object per line; `#` lines and blank lines
    ignored) or a JSON array. Ids are unique and safe as directory names."""
    stripped = text.strip()
    if stripped.startswith("["):
        items = json.loads(stripped)
    else:
        items = []
        for n, line in enumerate(text.splitlines(), 1):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            try:
                items.append(json.loads(line))
            except json.JSONDecodeError as e:
                raise ValueError(f"line {n}: {e.msg}") from None
    tasks: list[Task] = []
    seen: set[str] = set()
    for i, item in enumerate(items, 1):
        if not isinstance(item, dict):
            raise ValueError(f"task {i}: expected an object")
        unknown = set(item) - {"id", "prompt", "check", "mock", "timeout_s"}
        if unknown:
            raise ValueError(f"task {i}: unknown fields {sorted(unknown)}")
        tid = str(item.get("id") or f"task-{i}")
        if not TASK_ID.match(tid):
            raise ValueError(f"task {i}: id {tid!r} must match {TASK_ID.pattern}")
        if tid in seen:
            raise ValueError(f"task {i}: duplicate id {tid!r}")
        prompt = item.get("prompt")
        if not isinstance(prompt, str) or not prompt.strip():
            raise ValueError(f"task {tid}: prompt is required")
        timeout = int(item.get("timeout_s", 900))
        if timeout <= 0:
            raise ValueError(f"task {tid}: timeout_s must be positive")
        seen.add(tid)
        tasks.append(Task(tid, prompt, item.get("check"), item.get("mock"), timeout))
    if not tasks:
        raise ValueError("no tasks")
    return tasks


def parse_mcp(spec: str) -> dict[str, Any]:
    """`NAME=URL` (streamable HTTP, as reachable from inside the sandbox) or
    `NAME=cmd:COMMAND ARGS...` (stdio, run in the sandbox)."""
    name, sep, rest = spec.partition("=")
    if not sep or not name or not rest:
        raise ValueError(f"--mcp {spec!r}: expected NAME=URL or NAME=cmd:COMMAND")
    if rest.startswith("cmd:"):
        words = shlex.split(rest[4:])
        if not words:
            raise ValueError(f"--mcp {spec!r}: empty command")
        return {"name": name, "command": words[0], "args": words[1:]}
    if not rest.startswith(("http://", "https://")):
        raise ValueError(f"--mcp {spec!r}: URL must be http(s)")
    return {"name": name, "url": rest}


def task_dir(workdir: str, task: Task) -> str:
    return f"{workdir.rstrip('/')}/{task.id}"


def check_command(cwd: str, check: str) -> str:
    """The verifier line: the task's check, in the task's directory."""
    return f"cd {shlex.quote(cwd)} && {check}"


def parse_usage(usage_json: Optional[str]) -> dict[str, int]:
    """ACP usage (`inputTokens`, `outputTokens`, ...) as snake_case ints."""
    if not usage_json:
        return {}
    try:
        raw = json.loads(usage_json)
    except json.JSONDecodeError:
        return {}
    if not isinstance(raw, dict):
        return {}
    out = {}
    for k, v in raw.items():
        if isinstance(v, (int, float)) and not isinstance(v, bool):
            out[re.sub(r"(?<!^)([A-Z])", r"_\1", k).lower()] = int(v)
    return out


@dataclass
class TaskOutcome:
    """What collect learned about one task (plain data, JSON-ready)."""

    id: str
    run_id: Optional[str]
    status: str  # the run's status, or "missing" / "not_started"
    phase: str = ""
    turn: int = 0
    stop_reason: Optional[str] = None
    text: str = ""
    tool_calls: int = 0
    usage: dict[str, int] = field(default_factory=dict)
    error: Optional[str] = None
    artifacts: list[dict[str, Any]] = field(default_factory=list)
    verified: Optional[bool] = None  # None: no check, or the run did not finish
    check_output: str = ""


FINISHED = {"idle", "failed", "crashed"}


def build_report(meta: dict[str, Any], outcomes: Iterable[TaskOutcome]) -> dict[str, Any]:
    """The JSON report: run metadata, one entry per task, and a summary."""
    rows = [o.__dict__ | {"artifacts": list(o.artifacts)} for o in outcomes]
    passed = sum(1 for r in rows if r["verified"] is True)
    failed = sum(1 for r in rows if r["verified"] is False)
    finished = sum(1 for r in rows if r["status"] in FINISHED)
    tokens_in = sum(r["usage"].get("input_tokens", 0) for r in rows)
    tokens_out = sum(r["usage"].get("output_tokens", 0) for r in rows)
    return {
        **meta,
        "summary": {
            "tasks": len(rows),
            "finished": finished,
            "pending": len(rows) - finished,
            "passed": passed,
            "failed": failed,
            "unverified": len(rows) - passed - failed,
            "input_tokens": tokens_in,
            "output_tokens": tokens_out,
        },
        "tasks": rows,
    }


def run_label(prefix: str, task: Task) -> str:
    return f"{prefix}:{task.id}"


def task_of_label(prefix: str, label: Optional[str]) -> Optional[str]:
    if label and label.startswith(prefix + ":"):
        return label[len(prefix) + 1 :]
    return None


# ── SDK side ────────────────────────────────────────────────────────────────


def _cua():
    import cua  # the cua SDK (pip install cua)

    return cua


async def open_sandbox(args) -> Any:
    """A sandbox from --connect REF, or a new one from --image."""
    cua = _cua()
    c = cua.embedded()
    if args.connect:
        return await c.sandboxes().connect(args.connect)
    return await c.sandboxes().create(
        cua.SandboxCreateOptions(
            on="cloud" if args.cloud else "local",
            image=args.image,
            name=args.name,
            memory_mb=args.memory_mb,
            wait_for=[cua.ReadinessProbe(service="env")],
            ready_timeout_ms=600_000,
        )
    )


def run_options(args, task: Task) -> Any:
    cua = _cua()
    servers = [cua.AgentRunMcpServer(**parse_mcp(s)) for s in args.mcp]
    return cua.AgentRunOptions(
        cwd=task_dir(args.workdir, task),
        env_from_host=list(args.key_var),
        base_url=args.base_url,
        model=args.model,
        mcp_servers=servers,
        sandbox_mcp=not args.no_sandbox_mcp,
        exit_when_idle=True,  # fire-and-forget: the run stops itself when done
        label=run_label(args.label, task),
    )


async def launch(args) -> dict[str, Any]:
    tasks = parse_tasks(Path(args.tasks).read_text())
    sb = await open_sandbox(args)
    agents = await sb.agents()
    if not args.no_ensure:
        for line in await agents.ensure([args.harness]):
            print(f"  install {line}", file=sys.stderr)
    runs = {}
    for task in tasks:
        run = await agents.run(args.harness, task.prompt_for(args.scripted), run_options(args, task))
        runs[task.id] = run.run_id()
        print(f"{task.id}: {run.run_id()}", file=sys.stderr)
    state = {
        "sandbox": args.connect or f"{'cloud' if args.cloud else 'local'}:{sb.name()}",
        "image": args.image,
        "harness": args.harness,
        "label": args.label,
        "workdir": args.workdir,
        "tasks_file": str(Path(args.tasks).resolve()),
        "scripted": bool(args.scripted),
        "runs": runs,
    }
    Path(args.state).write_text(json.dumps(state, indent=2) + "\n")
    return state


async def _outcome(agents, guest, task: Task, run_id: Optional[str], workdir: str, wait_s: float) -> TaskOutcome:
    if run_id is None:
        return TaskOutcome(task.id, None, "not_started")
    try:
        run = await agents.get(run_id)
    except Exception as e:  # noqa: BLE001 - a removed run is a report row
        return TaskOutcome(task.id, run_id, "missing", error=str(e))
    if wait_s > 0:
        try:
            await run.wait(int(min(wait_s, task.timeout_s) * 1000))
        except Exception:  # noqa: BLE001 - still running: report it as such
            pass
    info = await run.status()
    res = await run.result()
    o = TaskOutcome(
        task.id,
        run_id,
        info.status,
        phase=info.phase,
        turn=res.turn,
        stop_reason=res.stop_reason,
        text=res.text,
        tool_calls=res.tool_calls,
        usage=parse_usage(res.usage_json),
        error=res.error,
        artifacts=[{"path": a.path, "size": a.size} for a in await run.artifacts()],
    )
    if task.check and o.status in FINISHED:
        out = await guest.sh(check_command(task_dir(workdir, task), task.check), 60_000)
        o.verified = bool(out.exit.success)
        o.check_output = (out.stdout + out.stderr).decode(errors="replace")[-2000:]
    return o


async def collect(args) -> dict[str, Any]:
    state = json.loads(Path(args.state).read_text())
    tasks = parse_tasks(Path(args.tasks or state["tasks_file"]).read_text())
    cua = _cua()
    sb = await cua.embedded().sandboxes().connect(state["sandbox"])
    agents = await sb.agents()
    guest = await sb.spacesd(None)
    # Runs this launch started, found by label too: a crashed launcher may
    # not have written every run id.
    runs = dict(state["runs"])
    for info in await agents.list():
        tid = task_of_label(state["label"], info.label)
        if tid and tid not in runs:
            runs[tid] = info.run_id
    outcomes = [
        await _outcome(agents, guest, t, runs.get(t.id), state["workdir"], args.wait)
        for t in tasks
    ]
    meta = {
        "image": state["image"],
        "harness": state["harness"],
        "sandbox": state["sandbox"],
        "scripted": state.get("scripted", False),
        "generated_at": _dt.datetime.now(_dt.timezone.utc).isoformat(timespec="seconds"),
    }
    report = build_report(meta, outcomes)
    Path(args.out).write_text(json.dumps(report, indent=2) + "\n")
    s = report["summary"]
    print(
        f"{s['passed']}/{s['tasks']} passed, {s['failed']} failed, {s['pending']} pending -> {args.out}",
        file=sys.stderr,
    )
    if args.delete:
        await sb.delete()
    return report


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)

    def launch_flags(sp):
        sp.add_argument("--image", default=DEFAULT_IMAGE, help="any image with cua-spacesd")
        sp.add_argument("--connect", help="use an existing sandbox (local:NAME, cloud:NAME, ...)")
        sp.add_argument("--cloud", action="store_true", help="create the sandbox on Fleet")
        sp.add_argument("--name", default="agents-in-sandboxes", help="sandbox name")
        sp.add_argument("--memory-mb", type=int, default=4096)
        sp.add_argument("--harness", default="claude-code", help="see `cua agent harnesses`")
        sp.add_argument("--tasks", required=True, help="JSON Lines task file")
        sp.add_argument(
            "--key-var",
            action="append",
            default=[],
            help="provider key forwarded from this environment (repeatable)",
        )
        sp.add_argument("--base-url", help="custom model endpoint (proxy, gateway, mock)")
        sp.add_argument("--model")
        sp.add_argument(
            "--mcp", action="append", default=[], help="NAME=URL or NAME=cmd:COMMAND (repeatable)"
        )
        sp.add_argument("--no-sandbox-mcp", action="store_true")
        sp.add_argument("--no-ensure", action="store_true", help="install on first run instead")
        sp.add_argument("--scripted", action="store_true", help="append each task's mock script")
        sp.add_argument("--workdir", default=DEFAULT_WORKDIR)
        sp.add_argument("--label", default="ais")
        sp.add_argument("--state", default="runs.json")

    def collect_flags(sp, after_launch=False):
        if not after_launch:  # `run` has these from launch_flags
            sp.add_argument("--state", default="runs.json")
            sp.add_argument("--tasks", help="default: the file launch used")
        sp.add_argument("--wait", type=float, default=0, help="seconds to wait per unfinished run")
        sp.add_argument("--out", default="report.json")
        sp.add_argument("--delete", action="store_true", help="delete the sandbox afterwards")

    launch_flags(sub.add_parser("launch", help="start one run per task and return"))
    collect_flags(sub.add_parser("collect", help="verify finished runs and write the report"))
    both = sub.add_parser("run", help="launch, then collect (waiting --wait per task)")
    launch_flags(both)
    collect_flags(both, after_launch=True)
    return p


async def main(argv: Optional[list[str]] = None) -> int:
    args = _parser().parse_args(argv)
    if args.cmd in ("launch", "run"):
        for var in args.key_var:
            if not os.environ.get(var):
                print(f"{var} is not set in this environment", file=sys.stderr)
                return 2
        await launch(args)
    if args.cmd == "run":
        args.wait = args.wait or 900
    if args.cmd in ("collect", "run"):
        report = await collect(args)
        return 0 if report["summary"]["failed"] == 0 else 1
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
