#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11,<3.14"
# dependencies = [
#   "cua-sandbox==0.7.0",
#   "httpx>=0.27,<1",
# ]
# ///
"""Run an OpenAI Agents API self-hosted session on Cua Cloud Fleet.

The application API key remains on the controller. Only the restricted
environment key is copied into the claimed VM, and its temporary file is
removed as soon as the detached executor starts.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import os
from pathlib import Path
import secrets
import shlex
from typing import Any, AsyncIterator, Callable

import httpx
from cua_sandbox import Image, Pool


AGENTS_BASE_URL = "https://api.openai.com/v1/agents/"
AGENTS_BETA_HEADER = "agents=v1"
IMAGE = (
    "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04"
    "@sha256:80fff8a40f217a460cef7a60161adb3899eabd02c3451f18926b84d1f81b8da2"
)
CODEX_VERSION = "0.155.0-alpha.3"
CUA_DRIVER_VERSION = "0.27.0"
ARTIFACT_PATH = "/workspace/outputs/openai-cua-fleet-e2e.txt"
ARTIFACT_CONTENT = "OPENAI AGENTS API ON CUA CLOUD FLEET PASSED\n"
RUNTIME_DIR = "/run/cua-agents"
EXECUTOR_ENV_PATH = f"{RUNTIME_DIR}/executor.env"
EXECUTOR_LAUNCHER_PATH = f"{RUNTIME_DIR}/launch-executor"
EXECUTOR_LOG_PATH = f"{RUNTIME_DIR}/executor.log"
EXECUTOR_PID_PATH = f"{RUNTIME_DIR}/executor.pid"
MCP_AUDIT_PROXY_PATH = f"{RUNTIME_DIR}/mcp-audit-proxy.py"
MCP_AUDIT_LOG_PATH = f"{RUNTIME_DIR}/mcp-tool-names.log"
E2E_TERMINAL_PATH = f"{RUNTIME_DIR}/e2e-terminal"
E2E_TARGET_PATH = f"{RUNTIME_DIR}/e2e-target"
TERMINAL_TURN_EVENTS = {
    "agent.session.turn.completed",
    "agent.session.turn.failed",
    "agent.session.turn.cancelled",
}


def required_env(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise RuntimeError(f"Set {name} before running this controller")
    return value


def pool_name() -> str:
    # This example deletes its pool. Never reconcile a caller's existing pool.
    return f"cua-openai-agents-{secrets.token_hex(16)}"


def root_turn(event: dict[str, Any]) -> bool:
    turn = event.get("turn")
    return not isinstance(turn, dict) or turn.get("subagent_id") is None


async def checked(sandbox: Any, command: str, timeout: int = 300) -> str:
    result = await sandbox.shell.run(command, timeout=timeout)
    if not result.success:
        detail = result.stderr.strip() or result.stdout.strip() or "command failed"
        raise RuntimeError(detail)
    return result.stdout.strip()


async def checked_read(
    sandbox: Any,
    command: str,
    *,
    timeout: int = 300,
    attempts: int = 3,
) -> str:
    """Retry a read-only shell probe after transient Fleet transport failures."""
    for attempt in range(1, attempts + 1):
        try:
            return await checked(sandbox, command, timeout=timeout)
        except Exception:
            if attempt == attempts:
                raise
            await asyncio.sleep(2)
    raise AssertionError("unreachable")


class AgentsClient:
    """Small async client for the public Agents API beta HTTP contract."""

    def __init__(self, api_key: str) -> None:
        self._client = httpx.AsyncClient(
            base_url=AGENTS_BASE_URL,
            headers={
                "Authorization": f"Bearer {api_key}",
                "OpenAI-Beta": AGENTS_BETA_HEADER,
            },
            timeout=httpx.Timeout(60, read=None),
            follow_redirects=False,
        )

    async def close(self) -> None:
        await self._client.aclose()

    async def request(
        self,
        method: str,
        path: str,
        *,
        payload: dict[str, Any] | None = None,
        params: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        attempts = 3 if method == "GET" else 1
        for attempt in range(1, attempts + 1):
            response = await self._client.request(
                method, path, json=payload, params=params
            )
            if response.status_code < 500 or attempt == attempts:
                response.raise_for_status()
                if not response.content:
                    return {}
                return response.json()
            await asyncio.sleep(2)
        raise AssertionError("unreachable")

    async def create_session(self) -> dict[str, Any]:
        return await self.request(
            "POST",
            "sessions",
            payload={
                "agent": {
                    "model": "gpt-6-astra",
                    "instructions": (
                        "Work carefully and report only behavior you observed. When a "
                        "desktop check is requested, you must use the named cua_driver MCP "
                        "tools for inspection and interaction; never substitute shell UI "
                        "automation."
                    ),
                    "tools": [
                        {
                            "type": "mcp",
                            "server_label": "cua_driver",
                            "transport": {
                                "type": "stdio",
                                "command": "/usr/bin/python3",
                                "args": [
                                    MCP_AUDIT_PROXY_PATH,
                                    "/root/.local/bin/cua-driver",
                                ],
                                "cwd": "/workspace",
                                "env_vars": [
                                    "DISPLAY",
                                    "CUA_DRIVER_PERMISSION_MODE",
                                    "CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS",
                                ],
                            },
                            "allowed_tools": [
                                "launch_app",
                                "get_window_state",
                                "click",
                                "verify_state",
                            ],
                            "required": True,
                        }
                    ],
                },
                "environment": {
                    "type": "self_hosted",
                    "workspace_directory": "/workspace",
                },
            },
        )

    async def delete_session(self, session_id: str) -> None:
        for attempt in range(1, 11):
            response = await self._client.delete(f"sessions/{session_id}")
            if response.status_code in {200, 202, 204, 404}:
                return
            if response.status_code == 409 and attempt < 10:
                await asyncio.sleep(2)
                continue
            response.raise_for_status()
        raise AssertionError("unreachable")

    async def retrieve_session(self, session_id: str) -> dict[str, Any]:
        return await self.request("GET", f"sessions/{session_id}")

    async def send_message(self, session_id: str, text: str) -> None:
        await self.request(
            "POST",
            f"sessions/{session_id}/events",
            payload={
                "events": [
                    {
                        "type": "agent.session.input.message",
                        "input": [
                            {
                                "role": "user",
                                "content": [{"type": "input_text", "text": text}],
                            }
                        ],
                    }
                ]
            },
        )

    async def list_items(self, session_id: str) -> dict[str, Any]:
        items: list[dict[str, Any]] = []
        after: str | None = None
        for _ in range(20):
            params: dict[str, Any] = {"order": "asc", "limit": 100}
            if after is not None:
                params["after"] = after
            page = await self.request(
                "GET",
                f"sessions/{session_id}/items",
                params=params,
            )
            page_items = page.get("data", [])
            items.extend(page_items)
            if not page.get("has_more"):
                return {"data": items}
            after = page.get("last_id") or (
                page_items[-1].get("id") if page_items else None
            )
            if not after:
                raise RuntimeError("Items page was truncated without a continuation cursor")
        raise RuntimeError("Items pagination exceeded 20 pages")

    @contextlib.asynccontextmanager
    async def event_lines(self, session_id: str) -> AsyncIterator[AsyncIterator[str]]:
        async with self._client.stream(
            "GET",
            f"sessions/{session_id}/events",
            params={"stream": "true"},
            headers={"Accept": "text/event-stream"},
        ) as response:
            response.raise_for_status()
            yield response.aiter_lines()


class EventMonitor:
    def __init__(self, lines: AsyncIterator[str]) -> None:
        self._lines = lines
        self._events: asyncio.Queue[dict[str, Any] | BaseException] = asyncio.Queue()
        self.observed: list[dict[str, Any]] = []
        self._task = asyncio.create_task(self._consume())

    async def _consume(self) -> None:
        data: list[str] = []
        try:
            async for line in self._lines:
                if line == "":
                    if data:
                        raw = "\n".join(data)
                        data.clear()
                        if raw == "[DONE]":
                            continue
                        event = json.loads(raw)
                        self.observed.append(event)
                        await self._events.put(event)
                    continue
                if line.startswith("data:"):
                    data.append(line[5:].lstrip())
        except BaseException as error:
            await self._events.put(error)

    async def wait_for(
        self,
        predicate: Callable[[dict[str, Any]], bool],
        *,
        timeout: float,
    ) -> dict[str, Any]:
        async with asyncio.timeout(timeout):
            while True:
                item = await self._events.get()
                if isinstance(item, BaseException):
                    raise item
                if predicate(item):
                    return item

    async def wait_for_type(self, event_type: str, *, timeout: float = 180) -> dict[str, Any]:
        return await self.wait_for(lambda event: event.get("type") == event_type, timeout=timeout)

    async def wait_for_root_turn(self, *, timeout: float = 900) -> dict[str, Any]:
        return await self.wait_for(
            lambda event: event.get("type") in TERMINAL_TURN_EVENTS and root_turn(event),
            timeout=timeout,
        )

    async def close(self) -> None:
        self._task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await self._task


def mcp_audit_proxy() -> str:
    return f'''#!/usr/bin/python3
import json
import subprocess
import sys
import threading

child = subprocess.Popen(
    [sys.argv[1], "mcp"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=sys.stderr.buffer,
)


def relay_stdout():
    while chunk := child.stdout.read1(65536):
        sys.stdout.buffer.write(chunk)
        sys.stdout.buffer.flush()


threading.Thread(target=relay_stdout, daemon=True).start()
for line in sys.stdin.buffer:
    try:
        message = json.loads(line)
        if message.get("method") == "tools/call":
            name = message.get("params", {{}}).get("name")
            if isinstance(name, str):
                with open({MCP_AUDIT_LOG_PATH!r}, "a", encoding="utf-8") as audit:
                    audit.write(name + "\\n")
    except (json.JSONDecodeError, AttributeError):
        pass
    child.stdin.write(line)
    child.stdin.flush()

child.stdin.close()
raise SystemExit(child.wait())
'''


def e2e_terminal_launcher() -> str:
    """Launch a synthetic GTK terminal and publish its proven PID/XID pair."""
    return f'''#!/bin/bash
set -euo pipefail
rm -f {shlex.quote(E2E_TARGET_PATH)}
xfce4-terminal --disable-server --title='OPENAI CUA FLEET E2E' --hold \\
  --command="bash -lc 'printf \\\"OPENAI CUA FLEET E2E\\\\n\\\"; sleep 3600'" &
terminal_pid=$!
window_id=''
for _attempt in $(seq 1 50); do
  window_id=$(xwininfo -root -tree 2>/dev/null | awk '/"OPENAI CUA FLEET E2E"/ && /xfce4-terminal/ {{print $1; exit}}')
  test -n "$window_id" && break
  sleep 0.1
done
test -n "$window_id"
window_pid=$(xprop -id "$window_id" _NET_WM_PID | awk '{{print $3}}')
test "$window_pid" -gt 0
printf '%s %s\n' "$window_pid" "$((window_id))" > {shlex.quote(E2E_TARGET_PATH)}
wait "$terminal_pid"
'''


async def install_runtime(sandbox: Any) -> str:
    await checked(sandbox, "mkdir -p /workspace /workspace/outputs /run/cua-agents")
    await checked(sandbox, "chmod 700 /run/cua-agents")
    await checked(
        sandbox,
        "curl -fsSL https://cua.ai/driver/install.sh -o /tmp/cua-driver-install.sh",
    )
    await checked(
        sandbox,
        f"CUA_DRIVER_RS_VERSION={CUA_DRIVER_VERSION} "
        "bash /tmp/cua-driver-install.sh --no-modify-path",
        timeout=600,
    )
    await checked(
        sandbox,
        f"npm install --global @openai/codex@{CODEX_VERSION}",
        timeout=600,
    )
    await checked(sandbox, "/root/.local/bin/cua-driver --version")
    await sandbox.files.write_bytes(MCP_AUDIT_PROXY_PATH, mcp_audit_proxy().encode())
    await sandbox.files.write_bytes(
        E2E_TERMINAL_PATH,
        e2e_terminal_launcher().encode(),
    )
    await checked(sandbox, f"chmod 700 {shlex.quote(MCP_AUDIT_PROXY_PATH)}")
    await checked(sandbox, f"chmod 700 {shlex.quote(E2E_TERMINAL_PATH)}")
    codex_path = await checked(sandbox, "command -v codex")
    if not codex_path.startswith("/"):
        raise RuntimeError("Codex CLI did not resolve to an absolute path")
    return codex_path


def executor_launcher(
    *, codex_path: str, remote_url: str, environment_id: str, display: str
) -> str:
    return f"""#!/bin/bash
set -euo pipefail
umask 077
set -a
. {shlex.quote(EXECUTOR_ENV_PATH)}
set +a
rm -f {shlex.quote(EXECUTOR_ENV_PATH)}
echo $$ > {shlex.quote(EXECUTOR_PID_PATH)}
export DISPLAY={shlex.quote(display)}
export PATH=/root/.local/bin:/usr/local/bin:/usr/bin:/bin
export CUA_DRIVER_PERMISSION_MODE=unrestricted
export CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS=1
exec {shlex.quote(codex_path)} exec-server \\
  --remote {shlex.quote(remote_url)} \\
  --environment-id {shlex.quote(environment_id)}
"""


async def start_executor(
    sandbox: Any,
    *,
    environment_key: str,
    launcher: str,
) -> int:
    # The file API keeps the key out of shell arguments and controller logs.
    await sandbox.files.write_bytes(
        EXECUTOR_ENV_PATH,
        f"CODEX_API_KEY={shlex.quote(environment_key)}\n".encode(),
    )
    await sandbox.files.write_bytes(EXECUTOR_LAUNCHER_PATH, launcher.encode())
    await checked(
        sandbox,
        f"chmod 600 {shlex.quote(EXECUTOR_ENV_PATH)} && "
        f"chmod 700 {shlex.quote(EXECUTOR_LAUNCHER_PATH)}",
    )
    launch_result = await sandbox.shell.run(
        f"exec /bin/bash {shlex.quote(EXECUTOR_LAUNCHER_PATH)} "
        f"> {shlex.quote(EXECUTOR_LOG_PATH)} 2>&1",
        background=True,
    )
    if not launch_result.success:
        raise RuntimeError(launch_result.stderr.strip() or "executor launch failed")
    status_command = (
        f"pid=$(cat {shlex.quote(EXECUTOR_PID_PATH)} 2>/dev/null || true); "
        "if test -z \"$pid\"; then echo NO_PID; "
        "elif ! kill -0 \"$pid\" 2>/dev/null; then echo EXITED; "
        f"elif test -e {shlex.quote(EXECUTOR_ENV_PATH)}; then echo STARTING; "
        "else echo READY:$pid; fi"
    )
    last_status = "NO_RESPONSE"
    ready_checks = 0
    for _ in range(30):
        try:
            last_status = await checked_read(
                sandbox,
                status_command,
                timeout=10,
                attempts=2,
            )
        except Exception:
            last_status = "TRANSPORT_ERROR"
        if last_status.startswith("READY:"):
            raw_pid = last_status.removeprefix("READY:")
            if not raw_pid.isdigit():
                raise RuntimeError("Executor did not publish a numeric process ID")
            ready_checks += 1
            if ready_checks >= 3:
                return int(raw_pid)
        else:
            ready_checks = 0
        if last_status == "EXITED":
            break
        await asyncio.sleep(1)

    log_tail = await checked_read(
        sandbox,
        f"tail -n 60 {shlex.quote(EXECUTOR_LOG_PATH)} 2>/dev/null | "
        "sed -E 's/sk-[A-Za-z0-9_-]+/[REDACTED]/g'",
        timeout=10,
    )
    raise RuntimeError(
        f"Executor did not become ready (status {last_status}); "
        f"sanitized log tail:\n{log_tail or '(empty)'}"
    )


async def stop_executor(sandbox: Any) -> None:
    await checked(
        sandbox,
        f"if test -f {shlex.quote(EXECUTOR_PID_PATH)}; then "
        f"pid=$(cat {shlex.quote(EXECUTOR_PID_PATH)}); "
        "kill \"$pid\" 2>/dev/null || true; "
        "for attempt in $(seq 1 30); do "
        "kill -0 \"$pid\" 2>/dev/null || exit 0; sleep 1; done; "
        "kill -KILL \"$pid\" 2>/dev/null || true; fi",
        timeout=45,
    )


async def verify_desktop_evidence(sandbox: Any) -> None:
    audit_names = (
        (await sandbox.files.read_bytes(MCP_AUDIT_LOG_PATH)).decode().splitlines()
    )
    missing = [
        name for name in ("launch_app", "click") if name not in audit_names
    ]
    if audit_names.count("get_window_state") < 2:
        missing.append("two get_window_state calls")
    if audit_names.count("click") < 2:
        missing.append("two click calls")
    if missing:
        raise RuntimeError(
            "MCP audit lacks required Cua Driver evidence: " + ", ".join(missing)
        )
    terminal_state = await checked_read(
        sandbox,
        f"read -r pid window_id < {shlex.quote(E2E_TARGET_PATH)}; "
        "for attempt in $(seq 1 30); do "
        "kill -0 \"$pid\" 2>/dev/null || { echo CLOSED; exit 0; }; "
        "sleep 0.2; done; echo OPEN",
    )
    if terminal_state != "CLOSED":
        raise RuntimeError("Cua Driver clicks did not close the terminal")


async def cleanup_resources(
    agents: AgentsClient,
    *,
    session: dict[str, Any] | None,
    pool: Pool | None,
) -> list[Exception]:
    errors: list[Exception] = []
    if session is not None:
        try:
            await agents.delete_session(session["id"])
        except Exception as error:
            errors.append(error)
    try:
        await agents.close()
    except Exception as error:
        errors.append(error)
    if pool is not None:
        try:
            await pool.delete()
        except Exception as error:
            errors.append(error)
    return errors


async def main() -> None:
    application_key = required_env("OPENAI_API_KEY")
    environment_key = required_env("CODEX_API_KEY")
    selected_pool_name = pool_name()
    output_path = Path(
        os.environ.get("CUA_ARTIFACT_DESTINATION", "openai-cua-fleet-e2e.txt")
    ).resolve()
    agents = AgentsClient(application_key)
    pool: Pool | None = None
    session: dict[str, Any] | None = None
    sandbox: Any = None
    executor_running = False
    run_error: Exception | None = None

    try:
        pool = await Pool.apply(
            Image.from_registry(IMAGE, os_type="linux", kind="vm"),
            name=selected_pool_name,
            replicas=1,
            cpu=4,
            memory_mb=8192,
            services={"server": 8000},
            ttl_seconds_after_created=7200,
        )
        async with pool.claim(
            name=f"session-{secrets.token_hex(4)}",
            service="server",
            time_to_start=1800,
            ttl_seconds_after_created=3600,
        ) as sandbox:
            print(f"Cua pool: {sandbox.pool_name}")
            print(f"Cua claim: {sandbox.claim_name}")
            codex_path = await install_runtime(sandbox)
            display = (await checked(sandbox, "printf %s \"${DISPLAY:-:1}\"")).strip() or ":1"

            session = await agents.create_session()
            session_id = session["id"]
            environment = session["environment"]
            launcher = executor_launcher(
                codex_path=codex_path,
                remote_url=environment["remote_url"],
                environment_id=environment["id"],
                display=display,
            )

            async with agents.event_lines(session_id) as lines:
                monitor = EventMonitor(lines)
                try:
                    await start_executor(
                        sandbox,
                        environment_key=environment_key,
                        launcher=launcher,
                    )
                    executor_running = True
                    await monitor.wait_for_type("agent.session.environment.connected")
                    print("Executor connection: PASS")

                    await agents.send_message(
                        session_id,
                        f"Write exactly {ARTIFACT_CONTENT.strip()!r} followed by a newline "
                        f"to {ARTIFACT_PATH}. Read the file back and report the exact "
                        "contents. Then perform this desktop check with the cua_driver MCP "
                        f"tools. (1) Call `launch_app` with name `/bin/bash` and "
                        f"additional_arguments [`{E2E_TERMINAL_PATH}`]. (2) Use the shell to "
                        f"wait up to 10 seconds for `{E2E_TARGET_PATH}` to become non-empty, "
                        "then read its two integers; they are the terminal's owner PID and "
                        "window ID. This read is discovery only, not UI automation. (3) Call "
                        "`get_window_state` for that exact PID and window ID. (4) Call `click` "
                        "on the `File` menu using the element token from that snapshot, not "
                        "coordinates. (5) Call `get_window_state` again, then call `click` on "
                        "`Close Terminal` or `Close Window` using the fresh element token from "
                        "the second snapshot. (6) Use the shell only to verify that the "
                        "published terminal PID is no longer running. Do not report success "
                        "unless every step completed.",
                    )
                    first_turn = await monitor.wait_for_root_turn()
                    if first_turn["type"] != "agent.session.turn.completed":
                        raise RuntimeError(
                            "First turn did not complete: "
                            + json.dumps(first_turn, sort_keys=True)
                        )
                    initial_artifact = await sandbox.files.read_bytes(ARTIFACT_PATH)
                    if initial_artifact != ARTIFACT_CONTENT.encode():
                        raise RuntimeError(
                            "Agent-created Fleet artifact content did not match"
                        )
                    await verify_desktop_evidence(sandbox)
                    print("Cua Driver MCP evidence: PASS")

                    await stop_executor(sandbox)
                    executor_running = False
                    await monitor.wait_for_type("agent.session.environment.disconnected")
                    print("Executor disconnect observation: PASS")

                    second_submission = asyncio.create_task(
                        agents.send_message(
                            session_id,
                            f"Read {ARTIFACT_PATH} and report its exact contents, including "
                            "whether it ends with a newline.",
                        )
                    )
                    try:
                        await monitor.wait_for_type(
                            "agent.session.requires_action", timeout=60
                        )
                        pending_session = await agents.retrieve_session(session_id)
                        required_actions = pending_session.get("required_actions", [])
                        if not any(
                            action.get("type") == "environment_connection"
                            and action.get("environment_id") == environment["id"]
                            for action in required_actions
                            if isinstance(action, dict)
                        ):
                            raise RuntimeError(
                                "Session did not request the original environment connection"
                            )
                        await start_executor(
                            sandbox,
                            environment_key=environment_key,
                            launcher=launcher,
                        )
                        executor_running = True
                        await monitor.wait_for_type("agent.session.environment.connected")
                        print("Same-environment reconnect: PASS")
                        await second_submission
                    finally:
                        if not second_submission.done():
                            second_submission.cancel()
                            with contextlib.suppress(asyncio.CancelledError):
                                await second_submission

                    second_turn = await monitor.wait_for_root_turn()
                    if second_turn["type"] != "agent.session.turn.completed":
                        raise RuntimeError(
                            "Second turn did not complete: "
                            + json.dumps(second_turn, sort_keys=True)
                        )

                    await agents.list_items(session_id)
                    artifact = await sandbox.files.read_bytes(ARTIFACT_PATH)
                    if artifact != ARTIFACT_CONTENT.encode():
                        raise RuntimeError("Retrieved Fleet artifact content did not match")
                    output_path.write_bytes(artifact)
                    print(f"Artifact retrieval: PASS ({output_path})")
                finally:
                    await monitor.close()
                    if executor_running:
                        await stop_executor(sandbox)
                        executor_running = False
    except Exception as error:
        run_error = error
    finally:
        cleanup_errors = await cleanup_resources(agents, session=session, pool=pool)

    if run_error is not None:
        if cleanup_errors:
            raise ExceptionGroup(
                "Controller run and cleanup both failed",
                [run_error, *cleanup_errors],
            )
        raise run_error
    if cleanup_errors:
        raise ExceptionGroup("Resource cleanup failed", cleanup_errors)

    print("OpenAI session, Fleet claim, and Fleet pool cleanup: PASS")


if __name__ == "__main__":
    asyncio.run(main())
