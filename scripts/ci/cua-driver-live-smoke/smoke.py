#!/usr/bin/env python3
"""Live smoke for the cua-driver agent loop on a real desktop.

Starts a cua-driver daemon, talks MCP to it over `cua-driver mcp`, opens the
fixture window (GTK3 on Linux, WinForms on Windows) and checks:

  * get_window_state's default (lean) output and `full_output: true`,
  * the act-by-element_token loop (snapshot, click a token, fresh snapshot),
  * `since` (a diff against an earlier snapshot, and `no change`),
  * run_actions (one batch with an end-of-batch observation),
  * set_value on a drop-down combo box and on an editable combo box.

Also checked: an element_token alone (no pid) acts and a stale one is
refused as `stale_element_token`. Those are required: each is `pass` or
`fail`. set_value on combo boxes (including delivery_mode foreground and an
unknown option) is required on the platforms in COMBO_HARD and best effort
elsewhere, where it is `pass`, `not_possible` (the driver refused with an
explicit error) or `bug` (the driver claimed success, or failed internally
after acting, and the app's change handler never saw the value).

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

# Row roles, matched against the role words before the quoted name. AT-SPI
# names a GtkEntry `text` and a label `label`; UIA calls a text box `Edit`
# and a label `Text`.
BUTTON = r"(push )?button"
ENTRY = r"edit" if WINDOWS else r"text|entry"
COMBO = r"combo ?box"
# Platforms where set_value on a combo box must reach the app's change
# handler: a failure there fails the job instead of being recorded.
COMBO_HARD = WINDOWS


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


def first(*values):
    return next((v for v in values if v is not None), None)


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
        lean = smoke.read()
        if find_row(tree(lean), BUTTON, "Increment") is not None:
            break
        if time.monotonic() >= deadline:
            raise RuntimeError("fixture tree never showed the Increment button:\n" + lean.text[:3000])
        time.sleep(1)
    print("---- lean tree ----\n" + tree(lean) + "\n-------------------", flush=True)

    # 1. Lean default.
    s = lean.structured
    problems = []
    if lean.is_error:
        problems.append("call failed: " + lean.first_line())
    if s.get("tree_format") != "markdown":
        problems.append(f"tree_format={s.get('tree_format')!r}")
    if "elements" in s:
        problems.append("structured `elements` present")
    if not s.get("snapshot_id"):
        problems.append("no snapshot_id")
    if "element_token for row [N]" not in lean.text:
        problems.append("no element_token hint in the text block")
    smoke.record(
        "get_window_state default is lean",
        "fail" if problems else "pass",
        "; ".join(problems)
        or f"tree_format=markdown, no `elements`, snapshot_id={s.get('snapshot_id')}, "
        f"{lean.size} chars",
    )

    # 2. full_output: true.
    full = smoke.read(full_output=True)
    f = full.structured
    elements = f.get("elements") or []
    problems = []
    if full.is_error:
        problems.append("call failed: " + full.first_line())
    if not elements:
        problems.append("no `elements`")
    elif not any(e.get("element_token") for e in elements):
        problems.append("`elements` carry no element_token")
    if not f.get("tree_markdown"):
        problems.append("no tree_markdown")
    if full.size <= lean.size:
        problems.append(f"full ({full.size} chars) not larger than lean ({lean.size})")
    smoke.record(
        "get_window_state full_output:true returns the full tree",
        "fail" if problems else "pass",
        "; ".join(problems)
        or f"{len(elements)} elements + tree_markdown, {full.size} chars vs lean {lean.size} "
        f"({full.size / max(lean.size, 1):.1f}x)",
    )

    # 3. Act by element_token, verify with a fresh snapshot.
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

    # 4. since: diff against the pre-click snapshot, then `no change`.
    since_id = before.structured["snapshot_id"]
    diff = smoke.read(since=since_id)
    d = diff.structured
    diff_text = d.get("tree_diff") or ""
    problems = []
    if diff.is_error:
        problems.append("call failed: " + diff.first_line())
    if d.get("since_status") != "diff":
        problems.append(f"since_status={d.get('since_status')!r}")
    if "Count: 1" not in diff_text:
        problems.append("diff does not mention 'Count: 1'")
    if "tree_markdown" in d:
        problems.append("diff read still carries the full tree_markdown")
    still = smoke.read(since=d.get("snapshot_id") or "")
    if still.structured.get("since_status") != "no_change":
        problems.append(
            f"second read since={d.get('snapshot_id')} gave "
            f"since_status={still.structured.get('since_status')!r}, expected 'no_change'"
        )
    smoke.record(
        "since returns a diff, then no_change",
        "fail" if problems else "pass",
        "; ".join(problems)
        or f"since={since_id}: diff_counts={d.get('diff_counts')}, {diff.size} chars vs "
        f"full read {after.size}; repeat read: no_change",
    )

    # 5. run_actions: set a field, click Apply, click Increment, observe once.
    state = smoke.read()
    md = tree(state)
    targets = {
        "entry": first(find_row(md, ENTRY, "Name field"), find_row(md, ENTRY)),
        "apply": find_row(md, BUTTON, "Apply"),
        "increment": find_row(md, BUTTON, "Increment"),
    }
    if None in targets.values():
        smoke.record("run_actions batch", "fail", f"fixture rows not found: {targets}")
    else:
        batch_args = {
            "steps": [
                {"tool": "set_value", "args": {"pid": smoke.pid, "element_token": token(state, targets["entry"]), "value": "Ada"}},
                {"tool": "click", "args": {"pid": smoke.pid, "element_token": token(state, targets["apply"])}},
                {"tool": "click", "args": {"pid": smoke.pid, "element_token": token(state, targets["increment"])}},
            ],
            "observe": {"pid": smoke.pid, "window_id": smoke.window_id},
        }
        batch = smoke.call("run_actions", batch_args)
        b = batch.structured
        observed = ((b.get("observation") or {}).get("state") or {}).get("tree_markdown") or ""
        problems = []
        if batch.is_error or not b.get("ok"):
            problems.append(f"batch failed: {json.dumps(b.get('steps'))[:600]} {batch.first_line()}")
        if b.get("executed") != 3:
            problems.append(f"executed={b.get('executed')}")
        if not (b.get("observation") or {}).get("ok"):
            problems.append("no observation")
        lagged = [n for n in ("Applied: Ada", "Count: 2") if n not in observed]
        if lagged and not problems:
            fresh = smoke.wait_for_text("Count: 2")
            missing = [n for n in lagged if n not in tree(fresh)]
            if missing:
                problems.append(f"neither the observation nor a fresh read shows {missing}")
        smoke.record(
            "run_actions batch (set_value + 2 clicks + observe)",
            "fail" if problems else "pass",
            "; ".join(problems)
            or "3/3 steps ok; "
            + ("observation shows 'Applied: Ada' and 'Count: 2'" if not lagged
               else f"observation lagged on {lagged}, fresh read confirms"),
        )

    # 6. set_value on combo boxes.
    combo_check(smoke, "Color", "Blue", editable=False)
    combo_check(smoke, "Size", "Large", editable=True)
    unknown_option_check(smoke)

    # 7. element_token alone, no pid: the token names its pid.
    token_without_pid_check(smoke)

    # 8. The drop-down combo with delivery_mode foreground (last because a
    # broken route may leave a popup open), then a click by token must still
    # land, which it does not while a popup holds the pointer grab.
    state = smoke.read()
    combo = find_row(tree(state), COMBO, "Color")
    status, evidence = set_value_attempt(
        smoke, state, combo, "Color", "Green", delivery_mode="foreground"
    )
    if status == "pass":
        landed, detail = click_still_lands(smoke)
        if not landed:
            status = "bug"
        evidence += "; " + detail
    smoke.record(
        "set_value on the drop-down combo box with delivery_mode foreground",
        hard(status),
        evidence,
    )


def hard(status):
    """A combo-box status under COMBO_HARD: anything but pass fails."""
    return "fail" if COMBO_HARD and status != "pass" else status


def count_value(markdown):
    match = re.search(r"Count: (\d+)", markdown)
    return int(match.group(1)) if match else None


def click_still_lands(smoke):
    """Click Increment by token and check the count moves."""
    state = smoke.read()
    before = count_value(tree(state))
    increment = find_row(tree(state), BUTTON, "Increment")
    if before is None or increment is None:
        return False, "no Increment button or Count label to probe with"
    smoke.call("click", {"pid": smoke.pid, "element_token": token(state, increment)})
    after = smoke.wait_for_text(f"Count: {before + 1}")
    if f"Count: {before + 1}" in tree(after):
        return True, f"a following click by token still lands (Count: {before + 1})"
    return False, f"a following click by token did not land (Count stayed {before})"


def token_without_pid_check(smoke):
    check = "act by element_token without pid; a stale token is refused"
    state = smoke.read()
    before = count_value(tree(state))
    increment = find_row(tree(state), BUTTON, "Increment")
    args = {"element_token": token(state, increment)}
    click = smoke.call("click", args)
    after = smoke.wait_for_text(f"Count: {(before or 0) + 1}")
    problems = []
    if click.is_error:
        problems.append(f"click {json.dumps(args)} -> {click.first_line()}")
    elif f"Count: {(before or 0) + 1}" not in tree(after):
        problems.append(f"click ok but the count did not move from {before}")
    stale_args = {"element_token": "s0fffffff:0", "value": "x"}
    stale = smoke.call("set_value", stale_args)
    code = (stale.structured.get("refusal") or {}).get("code")
    if not stale.is_error or code != "stale_element_token":
        problems.append(
            f"set_value {json.dumps(stale_args)} -> is_error={stale.is_error}, "
            f"refusal code {code!r}: {stale.first_line()}"
        )
    smoke.record(
        check,
        "fail" if problems else "pass",
        "; ".join(problems)
        or f"click {json.dumps(args)} -> Count: {(before or 0) + 1}; a stale token -> "
        f"stale_element_token ({stale.first_line()})",
    )


def unknown_option_check(smoke):
    """set_value with an option the drop-down does not have is refused and
    changes nothing."""
    check = "set_value on a drop-down combo box refuses an unknown option"
    state = smoke.read()
    combo = find_row(tree(state), COMBO, "Color")
    shown = re.search(r"Color: (\w+)", tree(state))
    args = {"pid": smoke.pid, "element_token": token(state, combo), "value": "Purple"}
    result = smoke.call("set_value", args)
    fresh = smoke.read()
    code = result.structured.get("code")
    if not result.is_error:
        status, evidence = "bug", f"set_value 'Purple' reported success: {result.first_line()}"
    elif shown and shown.group(0) not in tree(fresh):
        status, evidence = "bug", f"refused ({result.first_line()}) but the label changed"
    elif code != "option_not_found":
        status, evidence = "not_possible", f"refused with code {code!r}: {result.first_line()}"
    else:
        status, evidence = "pass", f"option_not_found: {result.first_line()}"
    smoke.record(check, hard(status), evidence)


def row_text(markdown, index):
    return next((text for i, text in rows(markdown) if i == index), "")


def set_value_attempt(smoke, state, index, name, value, **extra):
    """set_value on row `index`, then a fresh read for '<name>: <value>'.
    Returns (status, evidence): pass, not_possible (explicit refusal), or
    bug (the call claimed success, or failed internally after acting, and the
    app's change handler did not see the new value)."""
    args = {"pid": smoke.pid, "element_token": token(state, index), "value": value}
    args.update(extra)
    result = smoke.call("set_value", args)
    expected = f"{name}: {value}"
    fresh = smoke.wait_for_text(expected, timeout=5 if result.is_error else 10)
    if expected in tree(fresh):
        return "pass", f"set_value on row [{index}] -> fresh snapshot shows '{expected}'"
    control = row_text(tree(fresh), index)
    if result.is_error and result.structured.get("code") != "action_outcome_mismatch":
        return "not_possible", f"[{index}] {result.first_line()}"
    claim = ("failed internally: " if result.is_error else "reported success: ") + result.first_line()
    return "bug", (
        f"set_value{' ' + json.dumps(extra) if extra else ''} on row [{index}] {claim}; "
        f"the control now reads `{control}` but the app's change handler never ran "
        f"(no '{expected}')"
    )


def combo_check(smoke, name, value, editable):
    """set_value on a combo box: required under COMBO_HARD, otherwise best
    effort (pass, not_possible or bug, never failing the job)."""
    kind = "an editable combo box" if editable else "a drop-down combo box"
    check = f"set_value on {kind} ({name} -> {value})"
    state = smoke.read()
    md = tree(state)
    combo = find_row(md, COMBO, name)
    if combo is None:
        smoke.record(check, "fail", f"no combo row named {name!r} in the tree")
        return
    candidates = [combo]
    if editable:
        # GTK exposes the entry as a child row; UIA exposes an Edit child.
        child = find_row(md, ENTRY, after=combo)
        if child is not None:
            candidates.append(child)
    outcomes = []
    for index in candidates:
        status, evidence = set_value_attempt(smoke, state, index, name, value)
        outcomes.append((status, evidence))
        if status == "pass":
            break
        state = smoke.read()
    status = next((s for s in ("pass", "bug", "not_possible") if any(o[0] == s for o in outcomes)))
    smoke.record(check, hard(status), "; ".join(e for st, e in outcomes if st == status))


def write_reports(smoke, out, platform_name):
    out.mkdir(parents=True, exist_ok=True)
    (out / "results.json").write_text(json.dumps(smoke.results, indent=2), encoding="utf-8")
    (out / "calls.json").write_text(json.dumps(smoke.calls, indent=2, default=str), encoding="utf-8")
    lines = [
        f"## cua-driver live smoke: {platform_name}",
        "",
        "`fail` fails the job. `bug` (a best-effort check where the driver claimed "
        "success, or failed internally after acting, without the app seeing the "
        "change) and `not_possible` (an explicit refusal) are recorded only. "
        f"Combo-box checks are {'required' if COMBO_HARD else 'best effort'} here.",
        "",
        "| Check | Result | Evidence |",
        "| --- | --- | --- |",
    ]
    for r in smoke.results:
        evidence = r["evidence"].replace("|", "\\|").replace("\n", " ")
        lines.append(f"| {r['check']} | {r['status']} | {evidence} |")
        if r["status"] == "bug" and os.environ.get("GITHUB_ACTIONS"):
            print(f"::warning title=cua-driver bug: {r['check']}::{evidence}")
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
