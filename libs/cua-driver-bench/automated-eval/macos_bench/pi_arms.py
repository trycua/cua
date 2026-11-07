# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The Cua Driver setups with pi as the agent host instead of Claude Code.

Each ``pi-*`` arm mirrors one ``cc-*`` Cua Driver arm (``SOURCE_ARM``): the same driver build and daemon, the same MCP
server entry (that arm's ``--mcp-config``, copied), skill directory, system prompt, brief, turn cap and wall-time
limit. Only the host and the model differ: ``pi --mode json`` on ``openai-codex`` / ``gpt-6.1-terra`` by default
(``CDB_PI_PROVIDER``, ``CDB_PI_MODEL``, ``CDB_PI_THINKING``).

The tool surface mirrors Claude Code's ``Skill, Read, ToolSearch`` plus ``mcp__cua``: pi's ``read`` and ``tool_search``
built-ins, the ``cua`` server with ``deferred`` exposure (loaded through ``tool_search``, as Claude Code defers it
behind ToolSearch) and the build's skill through ``--skill``. Claude Code denies every ``Read`` in those arms
(``dontAsk`` with only ``mcp__cua`` allowed) and loads the skill through ``Skill``, so ``pi_gate.ts`` blocks
every ``read`` except the skill's ``SKILL.md``. Every other built-in, extension, context file, prompt
template and codemode is off. ``Translator`` turns pi's JSON events into the Claude-shaped stream that the GUI-only
guard and ``claude_events.summarize`` read; a pi tool outside that surface maps to a forbidden built-in, so it ends
the trial as a violation.

pi has no turn cap, so ``pi_gate.ts`` gives it Claude Code's ``--max-turns``: the ``max_turns``-th turn may run its
tools, the next one is aborted before its model request, and the host records ``error_max_turns``. A pi that exits
without settling (a crash, not a cap, timeout or abort) leaves no result event, so the row is a harness failure. Cost is pi's own figure for the reported tokens at the model's list price;
``openai-codex`` is a ChatGPT subscription, so it is not a billed amount.

Nothing here reads credentials: the trial's pi agent directory, under the private ``/tmp`` of the session, gets a
symlink to the operator's ``auth.json`` (``CDB_PI_AUTH_FILE``, default ``~/.pi/agent/auth.json``); only its
configuration is copied into the run directory.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

from arms import PI_ARMS as ARMS
from claude_events import _text_of
from desktop_runtime import record_or_reap, stop_group

SOURCE_ARM = {arm: "cc-" + arm.removeprefix("pi-") for arm in ARMS}
PROVIDER = os.environ.get("CDB_PI_PROVIDER") or "openai-codex"
MODEL = os.environ.get("CDB_PI_MODEL") or "gpt-6.1-terra"
THINKING = (
    os.environ.get("CDB_PI_THINKING") or "medium"
)  # pi's own default level, passed explicitly
PI_BIN = Path(os.environ.get("CDB_PI_BIN") or shutil.which("pi") or Path.home() / ".local/bin/pi")
AGENT_DIR = Path(os.environ.get("CDB_PI_AGENT_DIR") or Path.home() / ".pi" / "agent")
TOOLS = ("read", "tool_search", "mcp__cua__*")
GATE = Path(__file__).resolve().parent / "pi_gate.ts"
# pi built-in -> the Claude Code built-in the guard knows; any other non-MCP tool is "Task" (forbidden in v2)
AS_CLAUDE = {
    "read": "Read",
    "tool_search": "ToolSearch",
    "bash": "Bash",
    "powershell": "Bash",
    "edit": "Edit",
    "write": "Write",
}
COST_SOURCE = "pi usage cost at list price (openai-codex subscription, not billed)"
DESCRIPTIONS = {arm: f"pi + {PROVIDER}/{MODEL}, the {SOURCE_ARM[arm]} setup" for arm in ARMS}


def claude_name(name: str) -> str:
    return name if name.startswith("mcp__cua__") else AS_CLAUDE.get(name, "Task")


def skill_dir(cwd: Path) -> Path:
    return (
        cwd / ".claude" / "skills" / "cua-driver"
    )  # written by claude_arms.prepare_cwd(source arm)


def render_agent_dir(root: Path, mcp_json: Path, skills: Path, max_turns: int) -> dict[str, str]:
    """A fresh pi agent directory and HOME under ``root`` for one trial; returns pi's closed environment."""
    agent_dir = root / "agent"
    agent_dir.mkdir(parents=True, exist_ok=True)
    entry = json.loads(mcp_json.read_text("utf-8"))["mcpServers"]["cua"]
    config = {"mcpServers": {"cua": {**entry, "exposure": "deferred"}}, "autoEnableCodemode": False}
    (agent_dir / "mcp.json").write_text(json.dumps(config, indent=2) + "\n", "utf-8")
    settings = {"defaultProvider": PROVIDER, "defaultModel": MODEL}
    (agent_dir / "settings.json").write_text(json.dumps(settings) + "\n", "utf-8")
    store = AGENT_DIR / "models-store.json"  # the model catalog: context window and list prices
    if store.is_file():
        shutil.copyfile(store, agent_dir / "models-store.json")
    auth = (
        Path(os.environ.get("CDB_PI_AUTH_FILE") or AGENT_DIR / "auth.json").expanduser().absolute()
    )
    if not auth.is_file():
        raise FileNotFoundError("pi auth file is missing")
    (agent_dir / "auth.json").symlink_to(auth)
    home = root / "home"
    (home / "tmp").mkdir(parents=True, exist_ok=True)
    return {
        "HOME": str(home),
        "PATH": f"{PI_BIN.parent}:/usr/local/bin:/usr/bin:/bin",
        "TMPDIR": str(home / "tmp"),
        "LANG": "en_US.UTF-8",
        "PI_CODING_AGENT_DIR": str(agent_dir),
        "PI_OFFLINE": "1",
        "PI_SKIP_VERSION_CHECK": "1",
        "PI_TELEMETRY": "0",
        "CDB_PI_READ_ALLOW": str(skills / "SKILL.md"),  # pi_gate.ts
        "CDB_PI_MAX_TURNS": str(max_turns),
    }


def new_root() -> Path:
    """The trial's pi state, outside the run directory (the session's private /tmp on Linux)."""
    return Path(tempfile.mkdtemp(prefix="cdb-pi-"))


def keep_config(root: Path, dest: Path) -> None:
    """Copy the trial's pi configuration (never the credential) to ``dest``, then delete ``root``."""
    dest.mkdir(parents=True, exist_ok=True)
    for name in ("mcp.json", "settings.json"):
        if (root / "agent" / name).is_file():
            shutil.copyfile(root / "agent" / name, dest / name)
    shutil.rmtree(root, ignore_errors=True)


def argv(system_prompt: str, skills: Path | None, pi_bin: Path | None = None) -> list[str]:
    out = [
        str(pi_bin or PI_BIN),
        "--mode",
        "json",
        "--no-session",
        "--provider",
        PROVIDER,
        "--model",
        MODEL,
        "--thinking",
        THINKING,
        "--system-prompt",
        system_prompt,
        "--no-extensions",
        "-e",
        "builtin:mcp",
        "-e",
        "builtin:tool-search",
        "-e",
        str(GATE),
        "--no-skills",
        "--no-context-files",
        "--no-prompt-templates",
        "--no-themes",
        "--tools",
        ",".join(TOOLS),
    ]
    return out + (["--skill", str(skills)] if skills else [])


class Translator:
    """pi JSON events -> Claude-shaped stream-json events (assistant tool_use / user tool_result / init / result)."""

    def __init__(self, max_turns: int) -> None:
        self.max_turns = max_turns
        self.turns = 0
        self.final_text = ""
        self.error = ""
        self.last_stop: str | None = None
        self.started = 0
        self.capped = (
            threading.Event()
        )  # a turn past the cap started; pi_gate.ts aborts it before its request
        self.usage = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
        self.cost = 0.0
        self.models: set[str] = set()
        self.sections: dict[str, Any] = {}
        self.declared: list[str] = []
        self.mcp_loaded = False
        self.empty_search = False
        self.settled = False

    def feed(self, event: dict[str, Any]) -> list[dict[str, Any]]:
        if self.capped.is_set():
            return []  # the aborted turn past the cap is not a turn of the trial
        kind = event.get("type")
        if kind == "turn_start":
            self.started += 1
            if self.started > self.max_turns:
                self.capped.set()
                return []
        message = event.get("message") or {}
        role = message.get("role")
        if kind == "message_end" and role == "system":
            added = [str(t.get("name")) for t in message.get("toolsAdded") or []]
            self.declared += added
            self.mcp_loaded |= any(t.startswith("mcp__") for t in added)
            self.sections = self.sections or message.get("sections") or {}
        elif kind == "message_end" and role == "assistant":
            self.turns += 1
            usage = message.get("usage") or {}
            for key in self.usage:
                self.usage[key] += int(usage.get(key) or 0)
            self.cost += float((usage.get("cost") or {}).get("total") or 0.0)
            self.models.add(f"{message.get('provider')}/{message.get('model')}")
            content: list[dict[str, Any]] = []
            for block in message.get("content") or []:
                if block.get("type") == "text":
                    content.append({"type": "text", "text": str(block.get("text") or "")})
                elif block.get("type") == "toolCall":
                    content.append(
                        {
                            "type": "tool_use",
                            "id": str(block.get("id")),
                            "name": claude_name(str(block.get("name"))),
                            "input": block.get("arguments") or {},
                        }
                    )
            text = "".join(b["text"] for b in content if b["type"] == "text")
            self.final_text = text if text.strip() else self.final_text
            self.last_stop = message.get("stopReason")
            if self.last_stop == "error":
                self.error = str(message.get("errorMessage") or "pi provider error")
            return [
                {
                    "type": "assistant",
                    "message": {
                        "id": message.get("responseId") or f"pi-{self.turns}",
                        "content": content,
                        "usage": {
                            "input_tokens": int(usage.get("input") or 0),
                            "output_tokens": int(usage.get("output") or 0),
                            "cache_read_input_tokens": int(usage.get("cacheRead") or 0),
                            "cache_creation_input_tokens": int(usage.get("cacheWrite") or 0),
                        },
                    },
                }
            ]
        elif kind == "tool_execution_end":
            text = _text_of((event.get("result") or {}).get("content"))
            self.empty_search |= (
                event.get("toolName") == "tool_search" and "No matching tools" in text
            )
            failed = bool(event.get("isError"))
            block = {
                "type": "tool_result",
                "tool_use_id": str(event.get("toolCallId")),
                "is_error": failed,
                "content": text[:300] if failed else "",
            }
            return [{"type": "user", "message": {"content": [block]}}]
        elif kind == "agent_settled":
            self.settled = True
        elif kind == "auto_retry_start":
            retry = {"error": event.get("errorMessage"), "attempt": event.get("attempt")}
            return [
                {
                    "type": "system",
                    "subtype": "api_retry",
                    **retry,
                    "retry_delay_ms": event.get("delayMs"),
                }
            ]
        return []

    def finish(
        self, capped: bool, wall_ms: float, returncode: int | None = 0, stopped: bool = False
    ) -> list[dict[str, Any]]:
        """The init and result events, written when pi has stopped (``stopped``: the host ended it on purpose, at a
        timeout or an abort). None when pi produced no turn, exited without settling, or exited nonzero without a
        provider error, so the row is classified from stderr as a harness failure (retried, never scored) rather
        than scored as an agent result. A provider error keeps its message, so a 429 or 5xx is classified as such."""
        if not self.turns or not (capped or stopped or self.settled):
            return []
        is_error = self.last_stop == "error"
        if not (capped or stopped or is_error) and returncode not in (0, None):
            return []
        status = next(
            (c for c in (429, 401, 403, 500, 502, 503, 504, 529) if str(c) in self.error), None
        )
        # the server is "failed" only when tool_search found nothing and no cua tool ever loaded; unused is "ready"
        mcp = "connected" if self.mcp_loaded else ("failed" if self.empty_search else "ready")
        init = {
            "type": "system",
            "subtype": "init",
            "model": MODEL,
            "tools": [claude_name(t) for t in dict.fromkeys(self.declared)],
            "mcp_servers": [{"name": "cua", "status": mcp}],
            "skills": re.findall(r"<name>(.*?)</name>", str(self.sections.get("skills") or "")),
            "pi_prompt_sections": sorted(self.sections),
        }
        result = {
            "type": "result",
            "subtype": "error_max_turns"
            if capped
            else ("error_during_execution" if is_error else "success"),
            "is_error": is_error or capped,
            "api_error_status": status,
            "num_turns": self.turns,
            "duration_ms": round(wall_ms),
            "result": self.error[:2000] if is_error else self.final_text,
            "usage": {
                "input_tokens": self.usage["input"],
                "output_tokens": self.usage["output"],
                "cache_read_input_tokens": self.usage["cacheRead"],
                "cache_creation_input_tokens": self.usage["cacheWrite"],
            },
            "total_cost_usd": round(self.cost, 6),
            "pi_models": sorted(self.models),
        }
        return [init, result]


def _strip(value: Any, limit: int = 4000) -> Any:
    if isinstance(value, str):
        return value if len(value) <= limit else f"{value[:120]}...<{len(value)} chars stripped>"
    if isinstance(value, list):
        return [_strip(v, limit) for v in value]
    if isinstance(value, dict):
        return {k: _strip(v, limit) for k, v in value.items()}
    return value


def run_pi(
    cmd: list[str],
    env: dict[str, str],
    cwd: Path,
    prompt: str,
    timeout_s: float,
    max_turns: int,
    raw_path: Path,
    stream_path: Path,
    stderr_path: Path,
    abort: Any = None,
) -> dict[str, Any]:
    """One trial: pi in its own process group with the prompt on stdin, stopped at exit, the wall-time limit, the
    turn cap or ``abort``. pi's events (bulk stripped, streaming deltas dropped) go to ``raw_path``; the translated
    events go to ``stream_path`` as they arrive (both ``<epoch_ms>\\t<json>``), so the GUI-only guard reads them live."""
    started = time.monotonic()
    translator = Translator(max_turns)
    lock = threading.Lock()
    with (
        raw_path.open("w", encoding="utf-8") as raw,
        stream_path.open("w", encoding="utf-8") as stream,
        stderr_path.open("wb") as err,
    ):
        proc = subprocess.Popen(
            cmd,
            cwd=str(cwd),
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=err,
            start_new_session=True,
        )
        record_or_reap(proc, "pi")

        def write(obj: dict[str, Any]) -> None:
            stream.write(f"{time.time() * 1000:.1f}\t{json.dumps(obj)}\n")
            stream.flush()

        def pump() -> None:
            assert proc.stdout is not None
            for line in proc.stdout:  # binary: records split on LF only (pi's JSONL framing)
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                if not isinstance(event, dict):
                    continue
                with lock:
                    if event.get("type") != "message_update":
                        raw.write(f"{time.time() * 1000:.1f}\t{json.dumps(_strip(event))}\n")
                        raw.flush()
                    for out in translator.feed(event):
                        write(out)

        reader = threading.Thread(target=pump, daemon=True)
        reader.start()
        try:
            assert proc.stdin is not None
            proc.stdin.write(prompt.encode())
            proc.stdin.close()
        except OSError:
            pass
        timed_out = aborted = False
        while proc.poll() is None and not translator.capped.is_set():
            if time.monotonic() - started > timeout_s:
                timed_out = True
                break
            if abort is not None and abort.is_set():
                aborted = True
                break
            time.sleep(0.25)
        stop_group(proc.pid)
        reader.join(timeout=5)
        # read after the reader has drained pi's output: pi may print the over-cap turn_start and exit at once
        capped = translator.capped.is_set() and not timed_out and not aborted
        try:
            os.killpg(proc.pid, signal.SIGKILL)  # the MCP server child must not outlive the trial
        except OSError:
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        with lock:
            wall_ms = (time.monotonic() - started) * 1000
            for out in translator.finish(capped, wall_ms, proc.returncode, timed_out or aborted):
                write(out)
    return {
        "returncode": proc.returncode,
        "timed_out": timed_out,
        "aborted": aborted,
        "capped": capped,
        "result_seen": bool(translator.turns),
        "wall_s": round(time.monotonic() - started, 2),
        "elicitations": [],
        "turns": translator.turns,
        "models": sorted(translator.models),
    }


def observed(pi_bin: Path | None = None) -> dict[str, Any]:
    """The host's identity, recorded in the manifest and every row."""
    pi_bin = pi_bin or PI_BIN
    version = None
    sha = None
    if pi_bin.exists():
        env = {
            "PATH": f"{pi_bin.parent}:/usr/bin:/bin",
            "PI_OFFLINE": "1",
            "PI_SKIP_VERSION_CHECK": "1",
        }
        version = subprocess.run(
            [str(pi_bin), "--version"], capture_output=True, text=True, timeout=30, env=env
        ).stdout.strip()
        sha = hashlib.sha256(Path(os.path.realpath(pi_bin)).read_bytes()).hexdigest()
    store = AGENT_DIR / "models-store.json"
    return {
        "host": "pi --mode json",
        "pi_bin": str(pi_bin),
        "pi_version": version,
        "pi_cli_sha256": sha,
        "provider": PROVIDER,
        "model": MODEL,
        "thinking": THINKING,
        "tools": list(TOOLS),
        "mcp_exposure": "deferred",
        "gate_sha256": hashlib.sha256(GATE.read_bytes()).hexdigest(),
        "models_store_sha256": hashlib.sha256(store.read_bytes()).hexdigest()
        if store.is_file()
        else None,
    }


def init_check(
    arm: str, mcp_json: Path, cwd: Path, system_prompt: str, work: Path
) -> tuple[str, str]:
    """One trivial turn through the real argv: the declared tools, the skill and the model that answered."""
    import claude_events

    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    stream = work / "stream.tsv"
    root = new_root()
    try:
        env = render_agent_dir(root, mcp_json, skill_dir(cwd), 3)
        result = run_pi(
            argv(system_prompt, skill_dir(cwd)),
            env,
            cwd,
            "Reply OK. Do not call any tool.",
            180,
            3,
            work / "pi-events.tsv",
            stream,
            work / "stderr.txt",
        )
    finally:
        keep_config(root, work / "pi-agent")
    summary = claude_events.summarize(claude_events.read_events(stream))
    text = (summary.get("final_text") or "").strip()
    init = summary["init"]
    problems = []
    if not text.upper().startswith(("OK", "DONE")) or summary["tool_calls"]["total"]:
        problems.append(f"reply={text[:40]!r} tools={summary['tool_calls']['total']}")
    if init["tools_builtin"] != ["Read", "ToolSearch"] or init["mcp_tool_count"]:
        problems.append(f"declared tools {init['tools_builtin']} + {init['mcp_tool_count']} MCP")
    if init["skills"] != ["cua-driver"]:
        problems.append(f"skills {init['skills']}")
    if result["models"] != [f"{PROVIDER}/{MODEL}"]:
        problems.append(f"answered by {result['models']}")
    detail = f"{arm}: models={result['models']} skills={init['skills']} tokens={summary['tokens']}"
    return ("fail", "; ".join(problems) + " | " + detail) if problems else ("pass", detail)
