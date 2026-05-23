"""Overnight cua-driver test harness.

Drives cua-computer-server (running in cuademo's Session 9 on the Windows VM)
to test cua-driver against a matrix of apps × gestures. Captures structured
results to a JSON log so the night's findings are journal-friendly.

Run from the Mac side. ccs is exposed via SSH local-forward, OR we can hit
it directly if winvm:8000 is reachable.

Usage:
    python3 overnight-harness.py [--via-ssh] [--app=foo] [--gesture=bar]
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any, Callable, Optional


# ---------------------------------------------------------------------------
# Transport
# ---------------------------------------------------------------------------

CCS_HOST = "127.0.0.1"
CCS_PORT = 8000

# We talk to ccs via SSH-tunnelled HTTP. The simplest way is to invoke curl
# inside an `ssh winvm` command, which keeps everything in-band of the existing
# winvm SSH setup. requests/httpx aren't installed on the Mac side by default.

def ccs_cmd(command: str, params: Optional[dict] = None, timeout: int = 60) -> dict:
    """POST /cmd via a pre-staged PowerShell helper on the VM.

    Transport: ssh stdin → ccs-post.ps1 (uses Invoke-RestMethod). Avoids
    the curl-vs-Invoke-WebRequest alias problem AND avoids the JSON-quote
    mangling that happens when passing JSON through ssh+Windows-shell.
    The helper at C:\\Users\\fbonacci\\ccs-post.ps1 reads body from stdin
    and POSTs it raw.
    """
    body = json.dumps({"command": command, "params": params or {}})
    remote = "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\Users\\fbonacci\\ccs-post.ps1"
    p = subprocess.run(
        ["ssh", "winvm", remote],
        input=body, capture_output=True, text=True, timeout=timeout + 30,
    )
    raw = p.stdout.strip()
    if not raw:
        return {"_transport_error": p.stderr or "empty response", "_stderr": p.stderr}
    # ccs streams responses as "data: {...}\n\n" SSE-style — parse the first data
    # line we find.
    for line in raw.splitlines():
        line = line.strip()
        if line.startswith("data:"):
            payload = line[5:].strip()
            try:
                return json.loads(payload)
            except json.JSONDecodeError as e:
                return {"_parse_error": str(e), "_raw": payload[:500]}
    # Fall back: maybe ccs returned a plain JSON (not SSE)
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return {"_raw": raw[:1000]}


def shell(cmd: str, timeout: int = 60) -> dict:
    """Run a shell command on the VM via ccs."""
    return ccs_cmd("shell", {"command": cmd}, timeout=timeout)


def ps(script: str, timeout: int = 90) -> dict:
    """Run a PowerShell script (b64-encoded) on the VM via ccs."""
    import base64
    encoded = base64.b64encode(script.encode("utf-16-le")).decode("ascii")
    return shell(f"powershell -NoProfile -EncodedCommand {encoded}", timeout=timeout)


# ---------------------------------------------------------------------------
# cua-driver wrapper helpers
# ---------------------------------------------------------------------------

CUA = r"C:\Users\cuademo\.cua-driver\packages\releases\0.0.0-local-release-x86_64-pc-windows-msvc\cua-driver.exe"


def cua_call(tool: str, args_json: str = "{}", timeout: int = 30) -> dict:
    """Invoke `cua-driver call <tool>` via stdin pipe and parse output.

    Captures stdout (typically JSON or success-text) AND stderr (typically
    error text). cua-driver writes tool errors to stderr with exit code 1 —
    those used to look like `{_empty: true}` in earlier harness versions.
    """
    args_escaped = args_json.replace("'", "''")
    # Write stdout and stderr to separate files so PowerShell doesn't mangle.
    script = f"""
$ErrorActionPreference = 'Continue'
$payload = '{args_escaped}'
$stdoutFile = "$env:TEMP\\cua-call-out.txt"
$stderrFile = "$env:TEMP\\cua-call-err.txt"
Remove-Item -Force $stdoutFile, $stderrFile -EA SilentlyContinue
$proc = Start-Process -FilePath '{CUA}' -ArgumentList 'call','{tool}' `
  -RedirectStandardInput - -RedirectStandardOutput $stdoutFile `
  -RedirectStandardError $stderrFile -NoNewWindow -PassThru -Wait
# Workaround: Start-Process doesn't accept stdin via pipe cleanly;
# fall back to direct invocation with Tee-Object to capture.
"""
    # Use a delimited 3-section format to avoid ConvertTo-Json's "{"value":...}"
    # wrapping behaviour on long Get-Content strings.
    script = f"""
$ErrorActionPreference = 'Continue'
$tmpIn = "$env:TEMP\\cua-call-in.txt"
$tmpOut = "$env:TEMP\\cua-call-out.txt"
$tmpErr = "$env:TEMP\\cua-call-err.txt"
[System.IO.File]::WriteAllText($tmpIn, '{args_escaped}', (New-Object System.Text.UTF8Encoding $false))
$proc = Start-Process -FilePath '{CUA}' -ArgumentList 'call','{tool}' `
  -RedirectStandardInput $tmpIn -RedirectStandardOutput $tmpOut `
  -RedirectStandardError $tmpErr -NoNewWindow -PassThru -Wait
$out = if (Test-Path $tmpOut) {{ [System.IO.File]::ReadAllText($tmpOut) }} else {{ '' }}
$err = if (Test-Path $tmpErr) {{ [System.IO.File]::ReadAllText($tmpErr) }} else {{ '' }}
Write-Output "===EXIT===$($proc.ExitCode)"
Write-Output "===STDOUT==="
Write-Output $out
Write-Output "===STDERR==="
Write-Output $err
Write-Output "===END==="
"""
    r = ps(script, timeout=timeout)
    if not r.get("success"):
        return {"_ps_error": r.get("stderr", "")[:300], "_stdout": r.get("stdout", "")[:200]}
    raw = r.get("stdout", "")

    # Parse delimited sections
    def section(after: str, before: str) -> str:
        try:
            a = raw.index(after) + len(after)
            b = raw.index(before, a)
            return raw[a:b]
        except ValueError:
            return ""

    exit_line = section("===EXIT===", "\n")
    try:
        exit_code = int(exit_line.strip())
    except ValueError:
        exit_code = None
    stdout = section("===STDOUT===", "===STDERR===").strip()
    stderr = section("===STDERR===", "===END===").strip()

    # Strip the PS5.1 CLIXML wrapper from stderr if present
    if stderr.startswith("#< CLIXML"):
        idx = stderr.find("</Objs>")
        if idx > 0:
            stderr = stderr[idx + len("</Objs>"):].strip()

    if exit_code and exit_code != 0:
        return {"_exit_code": exit_code, "_error_text": stderr or stdout, "_stdout": stdout[:300]}

    if not stdout:
        return {"_empty": True, "_stderr": stderr[:300] if stderr else ""}

    # Try to parse stdout as JSON
    try:
        return json.loads(stdout)
    except json.JSONDecodeError:
        if "{" in stdout:
            start = stdout.index("{")
            try:
                return json.loads(stdout[start:])
            except json.JSONDecodeError:
                pass
        # Plain text — preserve it
        return {"_text": stdout[:2000], "_stderr_tail": stderr[:300] if stderr else ""}


# ---------------------------------------------------------------------------
# App management
# ---------------------------------------------------------------------------

APPS = {
    # name → {"launch": "cmd or AUMID", "kill_proc": "process name pattern", "title": "substring"}
    "notepad":      {"launch": "notepad.exe",                          "kill_proc": "notepad",          "title": "Notepad"},
    "calc":         {"launch": "calc.exe",                             "kill_proc": "CalculatorApp",    "title": "Calculator"},
    "paint":        {"launch": "mspaint.exe",                          "kill_proc": "mspaint",          "title": "Paint"},
    "edge":         {"launch": "msedge.exe https://example.com",       "kill_proc": "msedge",           "title": "Example"},
    "explorer":     {"launch": "explorer C:\\Users\\cuademo",          "kill_proc": "explorer",         "title": "cuademo"},
    "settings":     {"launch": "start ms-settings:",                   "kill_proc": "SystemSettings",   "title": "Settings"},
    "vscode":       {"launch": "code",                                 "kill_proc": "Code",             "title": "Visual Studio Code"},
    "notepad++":    {"launch": '"C:\\Program Files\\Notepad++\\notepad++.exe"', "kill_proc": "notepad++", "title": "Notepad++"},
    "libreoffice":  {"launch": '"C:\\Program Files\\LibreOffice\\program\\swriter.exe"', "kill_proc": "soffice", "title": "LibreOffice Writer"},
    "edge_youtube": {"launch": "msedge.exe https://youtube.com",       "kill_proc": "msedge",           "title": "YouTube"},
}


def launch_app(name: str) -> dict:
    spec = APPS.get(name)
    if not spec:
        return {"_error": f"unknown app {name}"}
    # `start "" cmd` with no -WindowStyle so the app uses its default visibility.
    # cmd's `start ""` returns immediately so we don't block.
    return shell(f'start "" /B cmd /c {spec["launch"]}', timeout=15)


def kill_app(name: str) -> dict:
    spec = APPS.get(name)
    if not spec:
        return {"_error": f"unknown app {name}"}
    return shell(f'taskkill /F /IM {spec["kill_proc"]}.exe /T 2>nul & exit /b 0')


def find_window(title_substr: str, timeout_s: int = 8) -> Optional[dict]:
    """Block until a window with title containing `title_substr` shows up."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        r = cua_call("list_windows")
        for w in (r.get("windows") or r.get("_legacy_windows") or []):
            if title_substr.lower() in (w.get("title") or "").lower():
                return w
        time.sleep(0.6)
    return None


# ---------------------------------------------------------------------------
# Test definitions
# ---------------------------------------------------------------------------

def t_launch_and_list(app: str) -> dict:
    """Launch an app, verify it shows in list_windows."""
    launch_app(app)
    w = find_window(APPS[app]["title"], timeout_s=15)
    return {
        "found": w is not None,
        "window": w,
    }


def t_get_window_state(app: str) -> dict:
    """get_window_state on the launched app's window."""
    w = find_window(APPS[app]["title"], timeout_s=5)
    if not w:
        return {"_error": "window not found"}
    payload = json.dumps({"pid": w["pid"], "window_id": w["window_id"]})
    r = cua_call("get_window_state", payload, timeout=45)
    if "_call_error" in r or "_text" in r:
        return {"_error": r}
    return {
        "element_count": r.get("element_count"),
        "tree_first_200": (r.get("tree_markdown") or "")[:200],
    }


def t_type_text(app: str, text: str = "hello cua") -> dict:
    """Focus the app, type text, screenshot to confirm."""
    w = find_window(APPS[app]["title"], timeout_s=5)
    if not w:
        return {"_error": "window not found"}
    payload = json.dumps({"pid": w["pid"], "window_id": w["window_id"], "text": text})
    r = cua_call("type_text", payload, timeout=20)
    return {"call_result": r}


def t_hotkey(app: str, key: str = "ctrl+s") -> dict:
    """Send a hotkey and report result."""
    w = find_window(APPS[app]["title"], timeout_s=5)
    if not w:
        return {"_error": "window not found"}
    payload = json.dumps({"pid": w["pid"], "key": key})
    return cua_call("hotkey", payload, timeout=20)


def t_screenshot(app: str) -> dict:
    """Take a screenshot scoped to the app's window."""
    w = find_window(APPS[app]["title"], timeout_s=5)
    if not w:
        return {"_error": "window not found"}
    payload = json.dumps({"pid": w["pid"], "window_id": w["window_id"]})
    r = cua_call("screenshot", payload, timeout=30)
    img_b64 = r.get("screenshot_png_b64") or ""
    return {
        "format": r.get("format"),
        "width": r.get("width"),
        "height": r.get("height"),
        "bytes_b64_len": len(img_b64),
    }


GESTURE_TESTS: dict[str, Callable] = {
    "launch_and_list":  t_launch_and_list,
    "get_window_state": t_get_window_state,
    "type_text":        t_type_text,
    "hotkey":           t_hotkey,
    "screenshot":       t_screenshot,
}


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

def run_matrix(apps: list[str], gestures: list[str], outdir: Path) -> dict:
    outdir.mkdir(parents=True, exist_ok=True)
    results: dict[str, dict] = {}
    for app in apps:
        results[app] = {}
        kill_app(app)
        time.sleep(1)
        for g in gestures:
            print(f"[{dt.datetime.now().strftime('%H:%M:%S')}] {app:<14}  {g}", flush=True)
            try:
                r = GESTURE_TESTS[g](app)
            except Exception as e:
                r = {"_runner_error": str(e)}
            results[app][g] = r
            time.sleep(0.5)
        # cleanup between apps
        kill_app(app)
        time.sleep(1)
    # Save
    out = outdir / f"matrix-{dt.datetime.now().strftime('%H%M%S')}.json"
    out.write_text(json.dumps(results, indent=2))
    print(f"\n→ saved {out}", flush=True)
    return results


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--app", action="append", default=[], help="app(s) to test; default = all")
    ap.add_argument("--gesture", action="append", default=[], help="gesture(s); default = all")
    ap.add_argument("--outdir", default="overnight-results", help="results directory")
    args = ap.parse_args()

    apps = args.app if args.app else list(APPS.keys())
    gestures = args.gesture if args.gesture else list(GESTURE_TESTS.keys())

    outdir = Path(__file__).parent / args.outdir
    run_matrix(apps, gestures, outdir)


if __name__ == "__main__":
    main()
