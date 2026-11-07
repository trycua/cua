#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Runner-agnostic fairness and health checks for Cua Driver Bench trials.

The rules are in SPEC.md next to this file. This module is the reference implementation: standard library
only, Python 3.9 or later, no import from any runner, so any runner (the bench's own `macos_bench`, an
AgentEnv provider, a hosted evaluation service) can import it or call its command line.

Every check returns a `Check` with `ok`, a short `reason` and `detail`. A failed check means a HARNESS ERROR:
the trial is re-run with the same seed and is never scored as an agent failure (see `classify` and
`scored_rows`).

Command line (one JSON object on stdout; exit 0 when healthy, 97 on a harness error, 2 on bad usage):

    cdb_fairness.py seed TASK RUN_INDEX
    cdb_fairness.py mcp MCP_JSON [--server NAME] [--timeout S] [--probe-tool NAME [--probe-args JSON]]
    cdb_fairness.py session-init STREAM_JSONL
    cdb_fairness.py appleevents APP [APP ...] [--timeout S] [--launch]
    cdb_fairness.py network [--url URL ...] [--require-lease]
    cdb_fairness.py toolchain [PROGRAM ...] [--descriptor DESCRIPTOR_JSON] [--path PATH]
    cdb_fairness.py summarize ROWS_JSONL
"""

from __future__ import annotations

import argparse
import hashlib
import ipaddress
import json
import os
import re
import select
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.request
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterable, Optional

EXIT_OK = 0
EXIT_HARNESS_ERROR = 97
SEED_MODULUS = 1_000_000

PASS, FAIL, HARNESS_ERROR = "pass", "fail", "harness_error"


@dataclass
class Check:
    name: str
    ok: bool
    reason: str = ""
    detail: dict[str, Any] = field(default_factory=dict)

    def to_json(self) -> dict[str, Any]:
        return asdict(self)


# ------------------------------------------------------------------ seeds


def trial_seed(task_id: str, run_index: int) -> int:
    """The seed of a task's run: the first 8 hex digits of sha256("<task>:<run_index>"), mod 10^6.

    `run_index` counts from 0 per task (trial k of a task, counting from 1, has run_index k-1). Every setup
    gets the same seed for the same task and run index, and a re-run of a harness-error trial keeps it."""
    if run_index < 0:
        raise ValueError("run_index counts from 0")
    digest = hashlib.sha256(f"{task_id}:{run_index}".encode()).hexdigest()
    return int(digest[:8], 16) % SEED_MODULUS


# ------------------------------------------------------------------ tool server (MCP) preflight


def _read_message(
    proc: subprocess.Popen, deadline: float, buffer: bytearray
) -> Optional[dict[str, Any]]:
    """One newline-delimited JSON-RPC message from the server's stdout, or None at the deadline."""
    assert proc.stdout is not None
    fd = proc.stdout.fileno()
    while True:
        newline = buffer.find(b"\n")
        if newline >= 0:
            line = bytes(buffer[:newline]).strip()
            del buffer[: newline + 1]
            if not line:
                continue
            try:
                return json.loads(line)
            except ValueError:
                continue  # a server may log non-JSON lines to stdout; skip them
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None
        ready, _, _ = select.select([fd], [], [], remaining)
        if not ready:
            return None
        chunk = os.read(fd, 65536)
        if not chunk:
            return None
        buffer.extend(chunk)


def _await_id(
    proc: subprocess.Popen, want: int, deadline: float, buffer: bytearray
) -> Optional[dict[str, Any]]:
    while True:
        message = _read_message(proc, deadline, buffer)
        if message is None or message.get("id") == want:
            return message


def mcp_preflight(
    command: list[str],
    env: Optional[dict[str, str]] = None,
    timeout: float = 60.0,
    probe_tool: Optional[str] = None,
    probe_args: Optional[dict[str, Any]] = None,
    name: str = "server",
) -> Check:
    """Start a stdio MCP server, `initialize`, list its tools and optionally call one probe tool.

    Healthy when the server answers `initialize` and `tools/list` with at least one tool within `timeout`,
    and the probe tool (when given) returns a result that is not an error. The agent must never start a trial
    without a working tool server: a failure here is a harness error, not an agent failure."""
    check = Check(f"mcp:{name}", False)
    started = time.monotonic()
    deadline = started + timeout
    try:
        proc = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env={**os.environ, **(env or {})},
        )
    except OSError as error:
        check.reason = f"cannot start the tool server: {error}"
        return check
    buffer = bytearray()

    def send(message: dict[str, Any]) -> None:
        assert proc.stdin is not None
        proc.stdin.write((json.dumps(message) + "\n").encode())
        proc.stdin.flush()

    try:
        send(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "cdb-fairness-preflight", "version": "1"},
                },
            }
        )
        init = _await_id(proc, 1, deadline, buffer)
        if init is None or "result" not in init:
            check.reason = (
                "no initialize result" if init is None else f"initialize error: {init.get('error')}"
            )
            return check
        send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
        listed = _await_id(proc, 2, deadline, buffer)
        tools = ((listed or {}).get("result") or {}).get("tools") or []
        check.detail["tools"] = len(tools)
        if not tools:
            check.reason = "tools/list returned no tools" if listed else "no tools/list result"
            return check
        if probe_tool:
            if probe_tool not in {t.get("name") for t in tools}:
                check.reason = f"probe tool {probe_tool!r} is not offered"
                return check
            send(
                {
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {"name": probe_tool, "arguments": probe_args or {}},
                }
            )
            called = _await_id(proc, 3, deadline, buffer)
            result = (called or {}).get("result")
            if result is None or result.get("isError"):
                check.reason = f"probe tool {probe_tool!r} failed"
                check.detail["probe"] = (called or {}).get("error") or result
                return check
        check.ok = True
        check.reason = "ready"
        return check
    except (BrokenPipeError, OSError) as error:
        check.reason = f"tool server exited: {error}"
        return check
    finally:
        check.detail["seconds"] = round(time.monotonic() - started, 2)
        try:
            proc.kill()
            proc.wait(timeout=5)
        except (OSError, subprocess.TimeoutExpired):
            pass
        for stream in (proc.stdin, proc.stdout):
            try:
                if stream is not None:
                    stream.close()
            except OSError:
                pass


def mcp_preflight_from_config(
    config_path: Path,
    server: Optional[str] = None,
    timeout: float = 60.0,
    probe_tool: Optional[str] = None,
    probe_args: Optional[dict[str, Any]] = None,
) -> list[Check]:
    """`mcp_preflight` for each stdio server of an `mcpServers` config (or only `server`)."""
    servers = json.loads(Path(config_path).read_text("utf-8")).get("mcpServers", {})
    if server is not None:
        servers = {server: servers[server]} if server in servers else {}
    if not servers:
        return [Check("mcp", False, "no MCP server in the config")]
    out = []
    for name, entry in servers.items():
        if "command" not in entry:
            out.append(Check(f"mcp:{name}", False, "not a stdio server (no command)"))
            continue
        out.append(
            mcp_preflight(
                [entry["command"], *entry.get("args", [])],
                env=entry.get("env"),
                timeout=timeout,
                probe_tool=probe_tool,
                probe_args=probe_args,
                name=name,
            )
        )
    return out


def session_servers_connected(init_event: dict[str, Any]) -> Check:
    """The agent session's own view of its tool servers, from a Claude Code `system`/`init` stream event
    (`mcp_servers: [{"name", "status"}]`). Every server must be `connected`; one that is `failed` or
    `pending` means the agent ran without its tools, so the trial is a harness error."""
    servers = init_event.get("mcp_servers") or []
    bad = [s.get("name", "?") for s in servers if s.get("status") != "connected"]
    if not servers:
        return Check("session_init", False, "the session reported no tool server")
    if bad:
        return Check(
            "session_init",
            False,
            "tool server not connected: " + ",".join(bad),
            {"servers": servers},
        )
    return Check("session_init", True, "connected", {"servers": servers})


def session_init_from_stream(lines: Iterable[str]) -> Check:
    """`session_servers_connected` on the first `system`/`init` event of a stream-json transcript."""
    for line in lines:
        try:
            event = json.loads(line)
        except ValueError:
            continue
        if event.get("type") == "system" and event.get("subtype") == "init":
            return session_servers_connected(event)
    return Check("session_init", False, "no init event in the stream")


# ------------------------------------------------------------------ AppleEvents / automation consent (macOS)

# osascript error numbers that mean the harness, not the agent, cannot automate the app
AE_HARNESS_ERRORS = {
    -1743: "not authorized to send Apple events (Automation consent missing)",
    -1744: "the user must consent first (Automation consent pending)",
    -10004: "privilege error (the sender is not entitled)",
    -1712: "Apple event timed out",
    -600: "the app is not running",
    -609: "connection to the app is invalid",
}


def appleevents_health(
    apps: Iterable[str],
    timeout: float = 30.0,
    osascript: str = "/usr/bin/osascript",
    launch: bool = False,
) -> list[Check]:
    """Send one harmless Apple event (`name`) to each app the task's reset or export automates.

    Run it after the reset, as the same user the reset and export run as. A task whose reset or export
    reads app data (Calendar, Reminders, Notes, Finder) cannot be graded when the event is refused, so a
    refusal is a harness error, never an agent failure.

    With `launch=False` (the default) an app that is not running is not sent anything, so the check never opens
    an app or a window that the task's start state does not have; run it after the task's apps have started."""
    out = []
    for app in apps:
        name = json.dumps(app)
        script = f"const a = Application({name}); a.running() ? a.name() : '__not_running__'"
        if launch:
            script = f"Application({name}).name()"
        try:
            done = subprocess.run(
                [osascript, "-l", "JavaScript", "-e", script],
                capture_output=True,
                text=True,
                timeout=timeout,
            )
        except FileNotFoundError:
            out.append(Check(f"appleevents:{app}", False, "osascript not found"))
            continue
        except subprocess.TimeoutExpired:
            out.append(Check(f"appleevents:{app}", False, AE_HARNESS_ERRORS[-1712]))
            continue
        if done.returncode == 0:
            if done.stdout.strip() == "__not_running__":
                out.append(
                    Check(
                        f"appleevents:{app}", True, "not running; not checked", {"checked": False}
                    )
                )
            else:
                out.append(Check(f"appleevents:{app}", True, "ok", {"checked": True}))
            continue
        match = re.search(r"\((-?\d+)\)\s*$", done.stderr.strip())
        code = int(match.group(1)) if match else None
        reason = AE_HARNESS_ERRORS.get(code or 0, "Apple event failed")
        out.append(
            Check(
                f"appleevents:{app}",
                False,
                reason,
                {"code": code, "stderr": done.stderr.strip()[-300:]},
            )
        )
    return out


# ------------------------------------------------------------------ network


def loopback_health() -> Check:
    """127.0.0.1 accepts a listener and a connection to it. Every pack task with a local web app serves it on
    loopback; if loopback is broken the app never becomes ready and the trial is a harness error."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as server:
            server.bind(("127.0.0.1", 0))
            server.listen(1)
            port = server.getsockname()[1]
            with socket.create_connection(("127.0.0.1", port), timeout=5):
                conn, _ = server.accept()
                conn.close()
        return Check("network:loopback", True, "ok", {"port": port})
    except OSError as error:
        return Check("network:loopback", False, f"loopback unusable: {error}")


def http_ready(url: str, timeout: float = 20.0) -> Check:
    """A task app's `ready.http` URL answers 200 within `timeout`."""
    deadline = time.monotonic() + timeout
    last = ""
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2) as response:
                if response.status == 200:
                    return Check(f"network:ready:{url}", True, "ok")
                last = f"HTTP {response.status}"
        except OSError as error:
            last = str(error)
        time.sleep(0.4)
    return Check(f"network:ready:{url}", False, f"not ready within {timeout:g} s: {last}")


def primary_ipv4() -> Optional[str]:
    """The source address the OS would use for an outside destination (no packet is sent)."""
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
            probe.connect(
                ("192.0.2.1", 9)
            )  # TEST-NET-1: never routed, nothing is sent for a UDP connect
            return probe.getsockname()[0]
    except OSError:
        return None


def lease_health() -> Check:
    """The environment has a usable IPv4 address (a DHCP lease or a static one), not none and not a
    169.254/16 self-assigned address. Only needed when something in the environment must reach the network
    (for example a tool server that talks to a host-side model gateway); tasks themselves use no internet."""
    address = primary_ipv4()
    if not address:
        return Check("network:lease", False, "no IPv4 route")
    ip = ipaddress.ip_address(address)
    if ip.is_link_local:
        return Check(
            "network:lease", False, "self-assigned address (no DHCP lease)", {"address": address}
        )
    if ip.is_loopback or ip.is_unspecified:
        return Check("network:lease", False, "no usable IPv4 address", {"address": address})
    return Check("network:lease", True, "ok", {"address": address})


def network_health(
    urls: Iterable[str] = (), require_lease: bool = False, timeout: float = 20.0
) -> list[Check]:
    checks = [loopback_health(), *(http_ready(u, timeout) for u in urls)]
    if require_lease:
        checks.append(lease_health())
    return checks


# ------------------------------------------------------------------ checker toolchain


DESCRIPTOR_PROGRAMS = {"${python}": "python3", "${evaluator_node}": "node"}


def programs_of_descriptor(
    descriptor: dict[str, Any], resolve: Optional[dict[str, str]] = None
) -> list[str]:
    """Programs the descriptor's reset, export and evaluate steps need: each step's first argument, and the
    interpreter variables used anywhere in its semantics. `resolve` maps `${python}` / `${evaluator_node}` to
    the executables the runner substitutes (default: `python3` and `node` on PATH)."""
    sem = descriptor.get("semantics", {})
    mapping = {**DESCRIPTOR_PROGRAMS, **{"${" + k + "}": v for k, v in (resolve or {}).items()}}
    found: list[str] = []
    steps = [*(sem.get("reset") or {}).values(), sem.get("export"), sem.get("evaluate")]
    for step in steps:
        if isinstance(step, list) and step:
            found.append(str(step[0]))
    text = json.dumps(sem)
    found += [v for v in DESCRIPTOR_PROGRAMS if v in text]
    out: list[str] = []
    for item in found:
        program = mapping.get(item, item)
        if "${" in program or program in out:
            continue
        out.append(program)
    return out


def toolchain_health(programs: Iterable[str], path: Optional[str] = None) -> list[Check]:
    """Every program the task's checker side needs is present and starts, on the PATH the reset, export and
    evaluator get. Pass the task's `health.evaluator_tools` from the task-set manifest, plus
    `programs_of_descriptor(...)`. A missing checker tool (for example `node` for a checker that starts the
    task's own web server) fails the trial for a reason that has nothing to do with the agent."""
    out = []
    for program in programs:
        found = program if os.path.isabs(program) else shutil.which(program, path=path)
        if not found or not os.access(found, os.X_OK):
            out.append(
                Check(f"toolchain:{program}", False, f"{program} not found on the checker's PATH")
            )
            continue
        if os.path.basename(found) in ("python3", "python", "node"):
            try:
                done = subprocess.run(
                    [found, "--version"], capture_output=True, text=True, timeout=20
                )
                ok = done.returncode == 0
                detail = {"path": found, "version": (done.stdout or done.stderr).strip()[:80]}
            except (OSError, subprocess.TimeoutExpired) as error:
                ok, detail = False, {"path": found, "error": str(error)}
            out.append(
                Check(f"toolchain:{program}", ok, "ok" if ok else f"{found} does not start", detail)
            )
        else:
            out.append(Check(f"toolchain:{program}", True, "ok", {"path": found}))
    return out


# ------------------------------------------------------------------ environment-lost watchdog


class EnvWatchdog:
    """Probe the environment during the agent phase; report it lost after `misses` failed probes in a row.

    `probe` is the runner's own liveness call (the VM answers, the tool server's process is alive, the
    desktop session still exists). A trial whose environment was lost mid-run is a harness error: the agent
    may have been acting on a desktop that no longer existed."""

    def __init__(self, probe: Callable[[], bool], interval: float = 10.0, misses: int = 3) -> None:
        self.probe, self.interval, self.misses = probe, interval, misses
        self.failures = 0
        self.lost_at: Optional[float] = None
        self.probes = 0
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, name="cdb-env-watchdog", daemon=True)
        self._started = 0.0

    def _run(self) -> None:
        while not self._stop.wait(self.interval):
            self.tick()
            if self.lost_at is not None:
                return

    def tick(self) -> None:
        """One probe (the thread calls it; tests call it directly)."""
        self.probes += 1
        try:
            alive = bool(self.probe())
        except Exception:  # noqa: BLE001 - a probe that raises is a failed probe
            alive = False
        self.failures = 0 if alive else self.failures + 1
        if self.failures >= self.misses and self.lost_at is None:
            self.lost_at = time.monotonic() - self._started

    def start(self) -> "EnvWatchdog":
        self._started = time.monotonic()
        self._thread.start()
        return self

    def stop(self) -> Check:
        self._stop.set()
        if self._thread.is_alive():
            self._thread.join(timeout=self.interval + 5)
        if self.lost_at is not None:
            return Check(
                "env_watchdog",
                False,
                "environment lost during the agent phase",
                {"lost_after_s": round(self.lost_at, 1), "probes": self.probes},
            )
        return Check("env_watchdog", True, "ok", {"probes": self.probes})


# ------------------------------------------------------------------ outcomes: harness error -> re-run, never scored


def classify(row: dict[str, Any]) -> str:
    """`pass`, `fail` or `harness_error` for one trial row.

    A row is a harness error when any health check failed (`health` list with an `ok: false` entry), when it
    carries `harness_error` (a reason string), or when its `status` is `harness_error`/`infra_error`. Only
    `pass` and `fail` are scored."""
    if row.get("harness_error") or row.get("status") in ("harness_error", "infra_error"):
        return HARNESS_ERROR
    if any(not c.get("ok", True) for c in row.get("health") or []):
        return HARNESS_ERROR
    return PASS if row.get("passed") else FAIL


def trial_key(row: dict[str, Any]) -> tuple[str, str, int, str]:
    return (
        str(row.get("arm") or row.get("setup") or ""),
        str(row["task"]),
        int(row["run_index"]),
        str(row.get("condition") or ""),
    )


def scored_rows(rows: Iterable[dict[str, Any]]) -> list[dict[str, Any]]:
    """The rows that count: for each (setup, task, run index, condition), its last pass/fail attempt.
    Harness-error attempts are dropped. A trial that only ever had harness errors is not in the result; it is
    reported by `rerun_plan`, never counted as a failure."""
    final: dict[tuple, dict[str, Any]] = {}
    for row in rows:
        if classify(row) != HARNESS_ERROR:
            final[trial_key(row)] = row
    return list(final.values())


def rerun_plan(
    rows: Iterable[dict[str, Any]], max_attempts: int = 3
) -> dict[str, list[dict[str, Any]]]:
    """Which trials to run again: a trial whose attempts so far are all harness errors is re-run with the
    same seed (`rerun`) until `max_attempts`, then reported as `excluded` (never scored)."""
    attempts: dict[tuple, list[dict[str, Any]]] = {}
    for row in rows:
        attempts.setdefault(trial_key(row), []).append(row)
    plan: dict[str, list[dict[str, Any]]] = {"rerun": [], "excluded": []}
    for key, tries in attempts.items():
        if any(classify(r) != HARNESS_ERROR for r in tries):
            continue
        setup, task, run_index, condition = key
        item = {
            "setup": setup,
            "task": task,
            "run_index": run_index,
            "condition": condition,
            "seed": trial_seed(task, run_index),
            "attempts": len(tries),
            "last_reason": tries[-1].get("harness_error") or tries[-1].get("status"),
        }
        plan["rerun" if len(tries) < max_attempts else "excluded"].append(item)
    return plan


def summarize(rows: list[dict[str, Any]], max_attempts: int = 3) -> dict[str, Any]:
    scored = scored_rows(rows)
    plan = rerun_plan(rows, max_attempts)
    return {
        "attempts": len(rows),
        "harness_error_attempts": sum(classify(r) == HARNESS_ERROR for r in rows),
        "scored": len(scored),
        "passed": sum(classify(r) == PASS for r in scored),
        "rerun": plan["rerun"],
        "excluded": plan["excluded"],
    }


# ------------------------------------------------------------------ command line


def _emit(checks: list[Check], extra: Optional[dict[str, Any]] = None) -> int:
    ok = all(c.ok for c in checks)
    print(
        json.dumps({"ok": ok, "checks": [c.to_json() for c in checks], **(extra or {})}, indent=1)
    )
    return EXIT_OK if ok else EXIT_HARNESS_ERROR


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(
        description="Cua Driver Bench fairness and health checks (see SPEC.md)"
    )
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("seed")
    p.add_argument("task")
    p.add_argument("run_index", type=int)
    p = sub.add_parser("mcp")
    p.add_argument("config", type=Path)
    p.add_argument("--server")
    p.add_argument("--timeout", type=float, default=60.0)
    p.add_argument("--probe-tool")
    p.add_argument("--probe-args", default="{}")
    p = sub.add_parser("session-init")
    p.add_argument("stream", type=Path)
    p = sub.add_parser("appleevents")
    p.add_argument("apps", nargs="+")
    p.add_argument("--timeout", type=float, default=30.0)
    p.add_argument(
        "--launch", action="store_true", help="also check apps that are not running (starts them)"
    )
    p = sub.add_parser("network")
    p.add_argument("--url", action="append", default=[])
    p.add_argument("--require-lease", action="store_true")
    p.add_argument("--timeout", type=float, default=20.0)
    p = sub.add_parser("toolchain")
    p.add_argument("programs", nargs="*", help="extra programs (the task's health.evaluator_tools)")
    p.add_argument("--descriptor", type=Path, help="also every program of this launch descriptor")
    p.add_argument(
        "--path", default=None, help="the PATH the checker gets (default: this process's)"
    )
    p.add_argument("--python")
    p.add_argument("--node")
    p = sub.add_parser("summarize")
    p.add_argument("rows", type=Path)
    p.add_argument("--max-attempts", type=int, default=3)
    args = parser.parse_args(argv)

    if args.command == "seed":
        print(
            json.dumps(
                {
                    "task": args.task,
                    "run_index": args.run_index,
                    "seed": trial_seed(args.task, args.run_index),
                }
            )
        )
        return EXIT_OK
    if args.command == "mcp":
        return _emit(
            mcp_preflight_from_config(
                args.config, args.server, args.timeout, args.probe_tool, json.loads(args.probe_args)
            )
        )
    if args.command == "session-init":
        return _emit(
            [session_init_from_stream(args.stream.read_text("utf-8", "replace").splitlines())]
        )
    if args.command == "appleevents":
        return _emit(appleevents_health(args.apps, args.timeout, launch=args.launch))
    if args.command == "network":
        return _emit(network_health(args.url, args.require_lease, args.timeout))
    if args.command == "toolchain":
        resolve = {k: v for k, v in (("python", args.python), ("evaluator_node", args.node)) if v}
        programs = list(args.programs)
        if args.descriptor:
            descriptor = json.loads(args.descriptor.read_text("utf-8"))
            programs += [
                p for p in programs_of_descriptor(descriptor, resolve) if p not in programs
            ]
        return _emit(toolchain_health(programs, args.path))
    if args.command == "summarize":
        rows = [
            json.loads(line) for line in args.rows.read_text("utf-8").splitlines() if line.strip()
        ]
        print(json.dumps(summarize(rows, args.max_attempts), indent=1))
        return EXIT_OK
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
