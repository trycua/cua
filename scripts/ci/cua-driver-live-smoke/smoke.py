#!/usr/bin/env python3
"""Live smoke for the cua-driver 0.34.x agent loop on a real desktop.

Starts a cua-driver daemon, talks MCP to it over `cua-driver mcp`, opens the
fixture window (GTK3 on Linux, WinForms on Windows) and checks:

  * the act-by-element_token loop (snapshot, click a token, fresh snapshot),
  * display_only previews keep the agent's element tokens valid (8 preview
    reads, then a click with the token from before the previews).

This is the 0.34 release line's copy of main's smoke
(scripts/ci/cua-driver-live-smoke on main, #4769 and #4881), cut down to the
0.34 surface: the lean read, `since`, run_actions, combo-box set_value and
pid-less element_token checks exercise 0.35 changes and are not here, nor is
the "plain screenshot-only read keeps tokens" check (that 0.35 change is not
backported).

Writes results.json, summary.md and the raw responses to --out, and appends
summary.md to $GITHUB_STEP_SUMMARY when set. Exits 1 when any check fails.
"""

import argparse
import json
import os
import queue
import re
import subprocess
import sys
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
TITLE = "Cua Live Smoke"
WINDOWS = sys.platform == "win32"

# Row roles, matched against the role words before the quoted name.
BUTTON = r"(push )?button"


class Mcp:
    """Line-delimited JSON-RPC over the stdio of `cua-driver mcp`."""

    def __init__(self, driver, socket, env, log):
        self.proc = subprocess.Popen(
            [driver, "mcp", "--socket", socket],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=log,
            env=env,
            text=True,
            encoding="utf-8",
        )
        self.lines = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        self.next_id = 1

    def _read(self):
        for line in self.proc.stdout:
            self.lines.put(line)

    def request(self, method, params, timeout=120):
        rid = self.next_id
        self.next_id += 1
        self.proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
            + "\n"
        )
        self.proc.stdin.flush()
        deadline = time.monotonic() + timeout
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                raise TimeoutError(f"{method} timed out after {timeout}s")
            try:
                line = self.lines.get(timeout=left)
            except queue.Empty:
                continue
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if message.get("id") == rid:
                return message

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()


class Call:
    def __init__(self, tool, args, message):
        self.tool = tool
        self.args = args
        result = message.get("result") or {}
        error = message.get("error")
        self.is_error = bool(error) or bool(result.get("isError"))
        self.structured = result.get("structuredContent") or {}
        self.text = "\n".join(
            part.get("text", "")
            for part in result.get("content") or []
            if part.get("type") == "text"
        )
        if error:
            self.text = json.dumps(error)
        self.size = len(self.text) + len(json.dumps(self.structured))

    def first_line(self):
        return (self.text.strip().splitlines() or [""])[0][:300]


class Smoke:
    def __init__(self, mcp, out):
        self.mcp = mcp
        self.out = out
        self.results = []
        self.calls = []
        self.pid = None
        self.window_id = None

    def call(self, tool, args, timeout=120):
        message = self.mcp.request(
            "tools/call", {"name": tool, "arguments": args}, timeout=timeout
        )
        result = Call(tool, args, message)
        self.calls.append(
            {
                "tool": tool,
                "args": args,
                "is_error": result.is_error,
                "text": result.text[:4000],
                "structured": result.structured,
            }
        )
        return result

    def record(self, check, status, evidence):
        self.results.append({"check": check, "status": status, "evidence": evidence})
        print(f"[{status.upper()}] {check}: {evidence}", flush=True)

    def read(self, **extra):
        args = {"pid": self.pid, "window_id": self.window_id}
        args.update(extra)
        return self.call("get_window_state", args)

    def wait_for_text(self, needle, timeout=10):
        """Fresh full reads until `needle` shows up; returns the last read."""
        deadline = time.monotonic() + timeout
        while True:
            state = self.read()
            if needle in tree(state):
                return state
            if time.monotonic() >= deadline:
                return state
            time.sleep(0.5)


def tree(state):
    return state.structured.get("tree_markdown") or state.text


def rows(markdown):
    for line in markdown.splitlines():
        match = re.match(r"\s*-\s*\[(\d+)\]\s*(.*)", line)
        if match:
            yield int(match.group(1)), match.group(2)


def find_row(markdown, role, label=None, after=None):
    """Index of the first row whose role fully matches `role` (regex) and
    whose text contains `label`, optionally only rows after index `after`."""
    for index, text in rows(markdown):
        if after is not None and index <= after:
            continue
        row_role = re.split(r'["\[]', text, maxsplit=1)[0].strip()
        if not re.fullmatch(role, row_role, re.IGNORECASE):
            continue
        if label is None or label.lower() in text.lower():
            return index
    return None



def token(state, index):
    return f"{state.structured['snapshot_id']}:{index}"


def start_daemon(driver, socket, env, log):
    daemon = subprocess.Popen(
        [driver, "serve", "--socket", socket, "--no-permissions-gate", "--no-overlay"],
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=log,
        env=env,
    )
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        if daemon.poll() is not None:
            raise RuntimeError(f"daemon exited with {daemon.returncode} during startup")
        probe = subprocess.run(
            [driver, "--socket", socket, "call", "get_config", "{}"],
            capture_output=True,
            env=env,
        )
        if probe.returncode == 0:
            return daemon
        time.sleep(1)
    raise RuntimeError("daemon did not answer get_config within 90 s")


def start_fixture(log):
    if WINDOWS:
        command = [
            "powershell.exe", "-NoProfile", "-STA", "-ExecutionPolicy", "Bypass",
            "-File", str(HERE / "fixture_winforms.ps1"),
        ]
        return subprocess.Popen(
            command, stdout=log, stderr=log, creationflags=0x08000000  # CREATE_NO_WINDOW
        )
    return subprocess.Popen(
        ["/usr/bin/python3", str(HERE / "fixture_gtk3.py")], stdout=log, stderr=log
    )


def find_window(smoke, pid):
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        listed = smoke.call("list_windows", {"pid": pid})
        for window in listed.structured.get("windows") or []:
            if window.get("pid") == pid and TITLE in (window.get("title") or ""):
                return window["window_id"]
        time.sleep(1)
    raise RuntimeError(f"no '{TITLE}' window for pid {pid} within 60 s")


def run_checks(smoke):
    # Wait until the accessibility tree is populated.
    deadline = time.monotonic() + 60
    while True:
        first_read = smoke.read()
        if find_row(tree(first_read), BUTTON, "Increment") is not None:
            break
        if time.monotonic() >= deadline:
            raise RuntimeError(
                "fixture tree never showed the Increment button:\n" + first_read.text[:3000]
            )
        time.sleep(1)
    print("---- tree ----\n" + tree(first_read) + "\n--------------", flush=True)

    # 1. Act by element_token, verify with a fresh snapshot.
    before = smoke.read()
    increment = find_row(tree(before), BUTTON, "Increment")
    click_args = {"pid": smoke.pid, "element_token": token(before, increment)}
    click = smoke.call("click", click_args)
    after = smoke.wait_for_text("Count: 1")
    if click.is_error:
        smoke.record("act by element_token (click) + fresh snapshot", "fail",
                     f"click {json.dumps(click_args)} -> {click.first_line()}")
    elif "Count: 1" not in tree(after):
        smoke.record("act by element_token (click) + fresh snapshot", "fail",
                     f"click ok ({click.first_line()}) but fresh snapshot lacks 'Count: 1'")
    else:
        smoke.record("act by element_token (click) + fresh snapshot", "pass",
                     f"click element_token={click_args['element_token']} -> fresh snapshot "
                     f"{after.structured.get('snapshot_id')} shows 'Count: 1'")

    # 2. display_only previews leave the agent's snapshot alone.
    display_only_check(smoke)


def count_value(markdown):
    match = re.search(r"Count: (\d+)", markdown)
    return int(match.group(1)) if match else None


DISPLAY_ONLY_READS = 8


def display_only_check(smoke):
    """A preview polling display_only reads (T3 Code's picture-in-picture)
    must not stale the agent's element tokens."""
    check = f"tokens stay valid after {DISPLAY_ONLY_READS} display_only reads"
    problems = []
    listed = smoke.mcp.request("tools/list", {})
    tools = (listed.get("result") or {}).get("tools") or []
    schema = next(
        (t.get("inputSchema") or {} for t in tools if t.get("name") == "get_window_state"), {}
    )
    if "display_only" not in (schema.get("properties") or {}):
        problems.append("get_window_state does not advertise display_only")

    state = smoke.read()
    snapshot = state.structured.get("snapshot_id")
    before = count_value(tree(state))
    increment = find_row(tree(state), BUTTON, "Increment")
    preview = {"include_accessibility_tree": False, "include_screenshot": True,
               "max_dimension": 480, "display_only": True}
    for n in range(DISPLAY_ONLY_READS):
        frame = smoke.read(**preview)
        f = frame.structured
        bad = []
        if frame.is_error:
            bad.append(frame.first_line())
        if f.get("display_only") is not True or not f.get("frame_note"):
            bad.append("no display_only/frame_note")
        if not isinstance(f.get("screenshot_width"), int) or not isinstance(f.get("screenshot_height"), int):
            bad.append("no screenshot_width/height")
        for key in ("snapshot_id", "invalidated_snapshot_ids", "capture_id"):
            if key in f:
                bad.append(f"carries {key}={f[key]!r}")
        if bad:
            problems.append(f"display_only read {n + 1}: " + "; ".join(bad))
            break
        time.sleep(0.25)
    refused = smoke.read(display_only=True)
    if not refused.is_error or "display_only requires include_accessibility_tree:false" not in refused.text:
        problems.append(f"display_only with a tree walk was not refused: {refused.first_line()}")

    args = {"pid": smoke.pid, "element_token": token(state, increment)}
    click = smoke.call("click", args)
    after = smoke.wait_for_text(f"Count: {(before or 0) + 1}")
    if click.is_error:
        problems.append(f"click with the pre-preview token {args['element_token']} -> {click.first_line()}")
    elif f"Count: {(before or 0) + 1}" not in tree(after):
        problems.append(f"click ok but the count did not move from {before}")
    smoke.record(
        check,
        "fail" if problems else "pass",
        "; ".join(problems)
        or f"schema advertises display_only; {DISPLAY_ONLY_READS} display_only reads of "
        f"snapshot {snapshot} carried no snapshot_id/capture_id; a tree walk with "
        f"display_only was refused; click {args['element_token']} -> Count: {(before or 0) + 1}",
    )


def write_reports(smoke, out, platform_name):
    out.mkdir(parents=True, exist_ok=True)
    (out / "results.json").write_text(json.dumps(smoke.results, indent=2), encoding="utf-8")
    (out / "calls.json").write_text(json.dumps(smoke.calls, indent=2, default=str), encoding="utf-8")
    lines = [
        f"## cua-driver live smoke: {platform_name}",
        "",
        "`fail` fails the job.",
        "",
        "| Check | Result | Evidence |",
        "| --- | --- | --- |",
    ]
    for r in smoke.results:
        evidence = r["evidence"].replace("|", "\\|").replace("\n", " ")
        lines.append(f"| {r['check']} | {r['status']} | {evidence} |")
    summary = "\n".join(lines) + "\n"
    (out / "summary.md").write_text(summary, encoding="utf-8")
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a", encoding="utf-8") as handle:
            handle.write(summary + "\n")
    print(summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--driver", required=True, help="path to the cua-driver binary")
    parser.add_argument("--out", required=True, help="artifact directory")
    options = parser.parse_args()
    # Tool text carries non-ASCII marks; a Windows console is cp1252.
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    out = Path(options.out)
    out.mkdir(parents=True, exist_ok=True)
    driver = str(Path(options.driver).resolve())
    socket = r"\\.\pipe\cua-driver-live-smoke" if WINDOWS else str(out / "d.sock")
    env = dict(os.environ)
    env.update(
        {
            "CUA_DRIVER_RS_TELEMETRY_ENABLED": "false",
            # Disposable CI desktop: no interactive approvals.
            "CUA_DRIVER_PERMISSION_MODE": "unrestricted",
            "CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS": "1",
        }
    )
    daemon_log = open(out / "daemon.log", "w", encoding="utf-8")
    mcp_log = open(out / "mcp.log", "w", encoding="utf-8")
    fixture_log = open(out / "fixture.log", "w", encoding="utf-8")

    daemon = start_daemon(driver, socket, env, daemon_log)
    fixture = None
    mcp = Mcp(driver, socket, env, mcp_log)
    smoke = Smoke(mcp, out)
    platform_name = "Windows (WinForms)" if WINDOWS else "Linux Xvfb (GTK3)"
    try:
        init = mcp.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "live-smoke", "version": "1"}})
        if "error" in init:
            raise RuntimeError(f"initialize failed: {init['error']}")
        mcp.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        mcp.proc.stdin.flush()
        version = smoke.call("get_config", {})
        print("driver config:", version.text[:800], flush=True)

        fixture = start_fixture(fixture_log)
        smoke.pid = fixture.pid
        smoke.window_id = find_window(smoke, fixture.pid)
        print(f"fixture pid={smoke.pid} window_id={smoke.window_id}", flush=True)
        run_checks(smoke)
    except Exception as error:  # noqa: BLE001 - every error becomes a failed row
        smoke.record("smoke harness", "fail", f"{type(error).__name__}: {error}")
    finally:
        write_reports(smoke, out, platform_name)
        mcp.close()
        for proc in (fixture, daemon):
            if proc is not None and proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill()

    return 1 if any(r["status"] == "fail" for r in smoke.results) else 0


if __name__ == "__main__":
    sys.exit(main())
