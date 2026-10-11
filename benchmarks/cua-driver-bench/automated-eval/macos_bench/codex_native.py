"""Amendment 20 (CUA-1347): Codex Computer-Use (Sky) driven by OpenAI's own agent and model.

Arm ``cx-codex-cu`` runs the Codex agent that ships inside the ChatGPT app (``codex-cli/CodexCLI.app``,
OpenAI-signed) with ``gpt-6-astra`` at the model's catalog default effort. The harness talks to it through
``codex app-server``, the JSON-RPC protocol the Codex app itself uses, because that is the only Codex host route
that forwards MCP elicitations to the client: ``codex exec`` declines them silently, and Codex Computer-Use asks
the host to approve every app it touches (``Allow Computer Use to use "<App>"?``). The host answers exactly as for
arm B (``claude_driver.answer_elicitation``): accept for the apps the task names, decline everything else. Every
other approval request (commands, file changes, permissions, user input) gets a JSON-RPC error and is recorded.

Its computer-use layer is the same ``cua_repl`` launcher as arm B (``cc-codex-cu``): the ``unified-computer-use``
plugin's shipped entry, unchanged except ``CUA_REPL_ENABLED_SURFACES=computer``, registered under the name the
plugin uses (``cua_repl``) with the plugin's model-facing tools (``js``, ``js_reset``). So arm B and this arm share
the tool layer and differ in the agent harness and the model that drives it.

GUI-only (A14.2) for this arm:

* The shell and every other non-GUI route this Codex build can switch off is off (``DISABLED_FEATURES``); the
  sandbox is read-only.
* What cannot be switched off is watched instead: ``apply_patch`` (a file edit) and the multi-agent collaboration
  tools. App-server reports each tool call as a thread item; ``Translator`` turns the items into the
  Claude-shaped events the GUI-only guard and the summaries already read, mapping a command execution to ``Bash``,
  a file change to ``Edit``, a web search to ``WebSearch``, a sub-agent call to ``Agent`` and any other non-GUI
  item to ``Task`` (all forbidden built-ins, so each ends the trial as a violation). MCP calls keep their name
  (``mcp__cua_repl__js``) and input, so the same code-pattern scan as arm B applies.
* Code mode: ``gpt-6-astra`` calls tools from a JavaScript ``exec`` cell (a V8 isolate with no file system, process
  or network access). Nested MCP calls still arrive as separate ``mcpToolCall`` items.

Turns are the model's sampling requests (one ``thread/tokenUsage/updated`` notification each), the counterpart of
Claude Code's assistant turns; the 45-turn cap stops the turn when the cap-th response was not the final one. No
list price is pinned for ``gpt-6-astra`` through the subscription gateway, so cost is not reported for this arm
(tokens are).

Nothing here reads credentials. The model is reached through the LiteLLM gateway at ``CDB_CODEX_GATEWAY``
(default ``http://127.0.0.1:4000/v1``), which in the bench VM is a local port forwarded to the owner's gateway for
the run only; no key is placed in the VM.
"""

from __future__ import annotations

import json
import os
import signal
import subprocess
import threading
import time
from pathlib import Path
from typing import Any, Callable

ARM = "cx-codex-cu"
ARMS = (ARM,)
SERVER = "cua_repl"  # the server name of the unified-computer-use plugin
ENABLED_TOOLS = (
    "js",
    "js_reset",
)  # the plugin's tools the model sees (turn_ended is a host hook, not a model tool)
CODEX_CLI = Path(
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"
)
MODEL = "gpt-6-astra"
EFFORT = "medium"  # gpt-6-astra's default_reasoning_level in the Codex model catalog; Claude arms use their default
GATEWAY_ENV = "CDB_CODEX_GATEWAY"
DEFAULT_GATEWAY = "http://127.0.0.1:4000/v1"
CLOSED_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"
LABEL = "Codex Computer-Use (Sky), driven by Codex CLI + GPT-6 Astra"

# Features of this Codex build that would give the agent a non-GUI route or a second agent. Off for every trial.
DISABLED_FEATURES = (
    "shell_tool",
    "unified_exec",
    "apps",
    "browser_use",
    "browser_use_external",
    "in_app_browser",
    "image_generation",
    "multi_agent",
    "multi_agent_v2",
    "goals",
    "view_image",
    "plugins",
    "tool_suggest",
    "skill_search",
)
# App-server thread item types that are not a computer-use call. Each maps to a forbidden built-in of A14.2.
SAFE_ITEMS = {
    "userMessage",
    "hookPrompt",
    "agentMessage",
    "plan",
    "reasoning",
    "mcpToolCall",
    "sleep",
    "contextCompaction",
    "functionCallOutput",
}
ITEM_AS_BUILTIN = {
    "commandExecution": "Bash",
    "fileChange": "Edit",
    "webSearch": "WebSearch",
    "collabAgentToolCall": "Agent",
    "subAgentActivity": "Agent",
}  # anything else outside SAFE_ITEMS (imageView, imageGeneration, dynamicToolCall, ...) maps to "Task"


def _q(value: str) -> str:
    return json.dumps(value, ensure_ascii=True)


def gateway() -> str:
    return os.environ.get(GATEWAY_ENV) or DEFAULT_GATEWAY


def server_entry(mcp_json: Path) -> dict[str, Any]:
    """The single server of arm B's MCP config (written by tools/make_codex_cu_mcp.py from the plugin)."""
    data = json.loads(mcp_json.read_text("utf-8"))
    servers = data.get("mcpServers") or {}
    if len(servers) != 1:
        raise ValueError(f"{mcp_json}: expected exactly one MCP server, found {sorted(servers)}")
    entry = next(iter(servers.values()))
    env = dict(entry.get("env") or {})
    if env.get("CUA_REPL_ENABLED_SURFACES") != "computer":
        raise ValueError(
            f"{mcp_json}: CUA_REPL_ENABLED_SURFACES must be 'computer', got {env.get('CUA_REPL_ENABLED_SURFACES')!r}"
        )
    return {"command": entry["command"], "args": list(entry.get("args") or []), "env": env}


def config_toml(
    entry: dict[str, Any], model: str, effort: str, instructions: str, codex_home: Path
) -> str:
    env = dict(
        entry["env"]
    )  # unchanged from arm B: Sky's service and pipe live under the user's own home
    lines = [
        f"model = {_q(model)}",
        'model_provider = "litellm"',
        f"model_reasoning_effort = {_q(effort)}",
        'approval_policy = "on-request"',  # elicitations reach the host only with on-request
        'sandbox_mode = "read-only"',
        'web_search = "disabled"',
        "mcp_optional_startup_grace_ms = 30000",
        f"developer_instructions = {_q(instructions)}",
        "",
        "[features]",
        *[f"{name} = false" for name in DISABLED_FEATURES],
        "",
        "[model_providers.litellm]",
        'name = "LiteLLM"',
        f"base_url = {_q(gateway())}",
        'wire_api = "responses"',
        "requires_openai_auth = false",
        "request_max_retries = 2",
        "stream_max_retries = 2",
        "stream_idle_timeout_ms = 120000",
        "",
        f"[mcp_servers.{SERVER}]",
        f"command = {_q(entry['command'])}",
        f"args = [{', '.join(_q(a) for a in entry['args'])}]",
        "startup_timeout_sec = 120",
        'default_tools_approval_mode = "approve"',
        f"enabled_tools = [{', '.join(_q(t) for t in ENABLED_TOOLS)}]",
        "",
        f"[mcp_servers.{SERVER}.env]",
        *[f"{key} = {_q(str(value))}" for key, value in sorted(env.items())],
    ]
    return "\n".join(lines) + "\n"


def render_home(
    home: Path, mcp_json: Path, model: str, effort: str, instructions: str
) -> dict[str, str]:
    """Write a fresh CODEX_HOME for one trial; return the closed environment for `codex exec`."""
    codex_home = home / ".codex"
    codex_home.mkdir(parents=True, exist_ok=True)
    (home / "tmp").mkdir(exist_ok=True)
    (codex_home / "config.toml").write_text(
        config_toml(server_entry(mcp_json), model, effort, instructions, codex_home), "utf-8"
    )
    return {
        # HOME stays the user's, as for the Claude-driven arms (Sky's native pipe is found from it); only Codex's
        # own config, sessions and caches move to the trial's CODEX_HOME.
        "HOME": os.environ.get("HOME", str(home)),
        "CODEX_HOME": str(codex_home),
        "PATH": CLOSED_PATH,
        "TMPDIR": str(home / "tmp"),
        "LANG": "en_US.UTF-8",
    }


def argv(cli: Path = CODEX_CLI) -> list[str]:
    return [str(cli), "app-server"]


def cli_version(cli: Path = CODEX_CLI) -> str | None:
    if not cli.is_file():
        return None
    out = subprocess.run([str(cli), "--version"], capture_output=True, text=True, timeout=30)
    return out.stdout.strip() or None


# ------------------------------------------------------------------ stream translation


class Translator:
    """App-server notifications -> Claude-shaped stream-json events (assistant tool_use / user tool_result /
    result), so `bench_v2.GuiOnlyGuard`, `bench_v2.scan_events` and `claude_events.summarize` read this arm the
    same way as the Claude arms."""

    def __init__(self) -> None:
        self.final_text = ""
        self.errors: list[str] = []
        self.usage = {
            "inputTokens": 0,
            "cachedInputTokens": 0,
            "outputTokens": 0,
            "reasoningOutputTokens": 0,
        }
        self.turns = 0
        self.turn_completed = False
        self.turn_status: str | None = None
        self.started: set[str] = set()
        self.non_gui: list[str] = []

    @staticmethod
    def _use(item_id: str, name: str, tool_input: Any) -> dict[str, Any]:
        return {
            "type": "assistant",
            "message": {
                "id": f"cx-{item_id}",
                "content": [{"type": "tool_use", "id": item_id, "name": name, "input": tool_input}],
            },
        }

    def feed(self, method: str, params: dict[str, Any]) -> list[dict[str, Any]]:
        out: list[dict[str, Any]] = []
        if method == "thread/started":
            out.append(
                {
                    "type": "system",
                    "subtype": "init",
                    "codex_thread": (params.get("thread") or {}).get("id"),
                    "tools": [f"mcp__{SERVER}__{t}" for t in ENABLED_TOOLS],
                    "mcp_servers": [{"name": SERVER, "status": "connected"}],
                }
            )
        elif method == "thread/tokenUsage/updated":
            self.turns += 1
            total = (params.get("tokenUsage") or {}).get("total") or {}
            for key in self.usage:
                self.usage[key] = int(total.get(key) or 0)
        elif method == "turn/completed":
            turn = params.get("turn") or {}
            self.turn_completed = True
            self.turn_status = turn.get("status")
            if turn.get("error"):
                self.errors.append(json.dumps(turn.get("error"))[:400])
        elif method == "error":
            self.errors.append(json.dumps(params)[:400])
        elif method in ("item/started", "item/completed"):
            item = params.get("item") or {}
            itype = str(item.get("type"))
            item_id = str(item.get("id"))
            if itype == "agentMessage" and method == "item/completed":
                self.final_text = str(item.get("text") or "")
            elif itype == "mcpToolCall":
                name = f"mcp__{item.get('server')}__{item.get('tool')}"
                if item_id not in self.started:
                    self.started.add(item_id)
                    out.append(self._use(item_id, name, item.get("arguments") or {}))
                if method == "item/completed":
                    failed = item.get("status") != "completed" or item.get("error") is not None
                    text = json.dumps(item.get("error"))[:300] if failed else ""
                    out.append(
                        {
                            "type": "user",
                            "message": {
                                "content": [
                                    {
                                        "type": "tool_result",
                                        "tool_use_id": item_id,
                                        "is_error": failed,
                                        "content": text,
                                    }
                                ]
                            },
                        }
                    )
            elif itype not in SAFE_ITEMS and item_id not in self.started:
                self.started.add(item_id)
                self.non_gui.append(itype)
                detail = {
                    k: v
                    for k, v in item.items()
                    if k in ("command", "changes", "query", "tool", "type", "path")
                }
                out.append(self._use(item_id, ITEM_AS_BUILTIN.get(itype, "Task"), detail))
        return out

    def result(self, timed_out: bool, capped: bool, wall_ms: float) -> dict[str, Any]:
        text = " ".join(self.errors)
        status = next(
            (code for code in (429, 401, 403, 500, 502, 503, 504, 529) if str(code) in text), None
        )
        is_error = bool(self.errors) and self.turn_status != "completed"
        subtype = (
            "error_max_turns" if capped else ("error_during_execution" if is_error else "success")
        )
        return {
            "type": "result",
            "subtype": subtype,
            "is_error": is_error or capped,
            "api_error_status": status,
            "num_turns": self.turns,
            "duration_ms": round(wall_ms),
            "result": self.final_text if not is_error else text[:2000],
            "usage": {
                "input_tokens": self.usage["inputTokens"] - self.usage["cachedInputTokens"],
                "output_tokens": self.usage["outputTokens"],
                "cache_read_input_tokens": self.usage["cachedInputTokens"],
                "cache_creation_input_tokens": 0,
            },
            "codex_usage": dict(self.usage),
            "timed_out": timed_out,
        }


def _kill_group(pid: int, grace: float = 1.0) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pid, sig)
        except OSError:
            return
        time.sleep(grace)


def run_codex(
    cmd: list[str],
    env: dict[str, str],
    cwd: Path,
    prompt: str,
    timeout_s: float,
    max_turns: int,
    raw_path: Path,
    stream_path: Path,
    stderr_path: Path,
    allowed_apps: set[str] | None = None,
    elicitation_path: Path | None = None,
    abort: Any = None,
    answer: Callable[[str, set[str]], dict[str, Any]] | None = None,
) -> dict[str, Any]:
    """Run one trial through `codex app-server` in its own process group: initialize, start a thread in ``cwd``,
    start one turn with ``prompt``, answer server requests, and stop at turn/completed, the wall-time limit, the
    turn cap or ``abort``. Raw JSON-RPC lines go to ``raw_path``; the translated Claude-shaped events go to
    ``stream_path`` (both ``<epoch_ms>\\t<json>``) as they arrive, so the GUI-only guard reads them live."""
    if answer is None:
        import claude_driver

        answer = claude_driver.answer_elicitation
    allowed = {a.strip().lower() for a in (allowed_apps or set())}
    started = time.monotonic()
    translator = Translator()
    done = threading.Event()
    responses: dict[int, dict[str, Any]] = {}
    got = threading.Condition()
    elicitations: list[dict[str, Any]] = []
    elog = elicitation_path.open("w", encoding="utf-8") if elicitation_path else None
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
            text=True,
            bufsize=1,
            start_new_session=True,
        )
        lock = threading.Lock()
        next_id = [0]

        def send(obj: dict[str, Any]) -> None:
            with lock:
                try:
                    assert proc.stdin is not None
                    proc.stdin.write(json.dumps(obj) + "\n")
                    proc.stdin.flush()
                except (OSError, ValueError):
                    pass

        def request(method: str, params: dict[str, Any], wait_s: float = 60.0) -> dict[str, Any]:
            with lock:
                next_id[0] += 1
                rid = next_id[0]
            send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
            deadline = time.monotonic() + wait_s
            with got:
                while rid not in responses and time.monotonic() < deadline and proc.poll() is None:
                    got.wait(0.5)
            reply = responses.get(rid) or {"error": {"message": f"no response to {method}"}}
            if "error" in reply:
                raise RuntimeError(f"{method}: {json.dumps(reply['error'])[:300]}")
            return reply.get("result") or {}

        def write(obj: dict[str, Any]) -> None:
            stream.write(f"{time.time() * 1000:.1f}\t{json.dumps(obj)}\n")
            stream.flush()

        def pump() -> None:
            assert proc.stdout is not None
            for line in proc.stdout:
                line = line.rstrip("\n")
                if not line.strip():
                    continue
                raw.write(f"{time.time() * 1000:.1f}\t{line}\n")
                raw.flush()
                try:
                    msg = json.loads(line)
                except json.JSONDecodeError:
                    continue
                method = msg.get("method")
                if method and "id" in msg:  # a server request
                    params = msg.get("params") or {}
                    if method == "mcpServer/elicitation/request":
                        reply = answer(str(params.get("message") or ""), allowed)
                        record = {
                            "ts": time.time(),
                            "message": params.get("message", ""),
                            "answer": reply["action"],
                        }
                        send({"jsonrpc": "2.0", "id": msg["id"], "result": reply})
                    else:
                        record = {"ts": time.time(), "unhandled": method}
                        send(
                            {
                                "jsonrpc": "2.0",
                                "id": msg["id"],
                                "error": {
                                    "code": -32601,
                                    "message": "not handled by the benchmark host",
                                },
                            }
                        )
                    elicitations.append(record)
                    if elog:
                        elog.write(json.dumps(record) + "\n")
                        elog.flush()
                elif method:
                    for out in translator.feed(method, msg.get("params") or {}):
                        write(out)
                    if method == "turn/completed":
                        done.set()
                elif "id" in msg:
                    with got:
                        responses[msg["id"]] = msg
                        got.notify_all()

        reader = threading.Thread(target=pump, daemon=True)
        reader.start()
        timed_out = capped = aborted = False
        startup_error = ""
        try:
            request("initialize", {"clientInfo": {"name": "cdb-bench", "version": "1"}})
            send({"jsonrpc": "2.0", "method": "initialized"})
            thread = request("thread/start", {"cwd": str(cwd)}, wait_s=180.0)
            thread_id = (thread.get("thread") or {}).get("id")
            request(
                "turn/start", {"threadId": thread_id, "input": [{"type": "text", "text": prompt}]}
            )
        except Exception as error:  # noqa: BLE001 - recorded; the row classifies it
            startup_error = f"{type(error).__name__}: {error}"
            translator.errors.append(startup_error)
        while not startup_error and proc.poll() is None and not done.is_set():
            if time.monotonic() - started > timeout_s:
                timed_out = True
                break
            if abort is not None and abort.is_set():
                aborted = True
                break
            if translator.turns >= max_turns and not done.wait(3.0):
                capped = (
                    True  # the cap-th response was not the last one: the next would be past the cap
                )
                break
            done.wait(0.25)
        _kill_group(proc.pid)
        reader.join(timeout=5)
        try:
            os.killpg(
                proc.pid, signal.SIGKILL
            )  # MCP children (cua_repl, node_repl) must not outlive the trial
        except OSError:
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        with lock:
            write(translator.result(timed_out, capped, (time.monotonic() - started) * 1000))
    # app-server is a server: the harness ends it after the turn, so a completed turn counts as a clean exit
    returncode = (
        0
        if translator.turn_status == "completed" and not (timed_out or capped or aborted)
        else proc.returncode
    )
    if elog:
        elog.close()
    return {
        "returncode": returncode,
        "timed_out": timed_out,
        "aborted": aborted,
        "capped": capped,
        "result_seen": translator.turn_completed,
        "wall_s": round(time.monotonic() - started, 2),
        "elicitations": elicitations,
        "turns": translator.turns,
        "non_gui_items": translator.non_gui,
        "codex_usage": dict(translator.usage),
        "startup_error": startup_error,
    }


def observed(cli: Path = CODEX_CLI) -> dict[str, Any]:
    import hashlib

    sha = None
    if cli.is_file():
        digest = hashlib.sha256()
        with cli.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1 << 20), b""):
                digest.update(chunk)
        sha = digest.hexdigest()
    return {
        "codex_cli": str(cli),
        "codex_cli_version": cli_version(cli),
        "codex_cli_sha256": sha,
        "model": MODEL,
        "effort": EFFORT,
        "gateway": gateway(),
        "host": "codex app-server",
    }
