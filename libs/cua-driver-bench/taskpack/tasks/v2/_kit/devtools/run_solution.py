#!/usr/bin/env python3
"""Development-only end-to-end check of a v2 web task on a headless Chrome (never in trials).

    python3 run_solution.py TASK_DIR [--steps FILE] [--expect pass|fail]

Resets a temporary workspace with the task's own reset, starts the task's service apps from its
macOS launch descriptor, opens the descriptor's start URL in a headless Chrome with its own temporary
profile, replays the step list with real input events (cdp_solve.mjs), stops everything by PID and runs
the task's evaluator. Exit 0 when the verdict matches --expect.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"


def sub(text: str, values: dict[str, str]) -> str:
    return re.sub(r"\$\{(\w+)\}", lambda m: values.get(m.group(1), m.group(0)), text)


def wait_http(url: str, timeout: float = 15) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=2) as r:
                if r.status == 200:
                    return
        except OSError:
            time.sleep(0.2)
    raise RuntimeError(f"not ready: {url}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("task", type=Path)
    ap.add_argument("--steps", type=Path)
    ap.add_argument("--expect", choices=("pass", "fail"), default="pass")
    ap.add_argument("--keep", action="store_true")
    args = ap.parse_args()
    bundle = args.task.resolve()
    steps = args.steps or bundle / "tests" / "solution.json"
    desc = json.loads((bundle / "platform" / "launch.macos.json").read_text())
    tmp = Path(tempfile.mkdtemp(prefix="v2sol-"))
    ws = tmp / "ws"
    values = {"python": sys.executable, "workspace": str(ws), "bundle": str(bundle), "HOME": str(tmp),
              "workspace_uri": ws.as_uri(), "artifacts": str(tmp / "art"), "result": str(tmp / "result.json"),
              "agent_exit_code": "0"}
    (tmp / "art").mkdir()
    run = lambda cmd: subprocess.run([sub(c, values) for c in cmd], cwd=bundle, check=True, capture_output=True, text=True)
    run(desc["semantics"]["reset"]["setup"])
    run(desc["semantics"]["reset"]["verify"])
    procs: list[subprocess.Popen] = []
    url = None
    try:
        for app in desc["apps"]:
            if app["kind"] == "service":
                env = {**os.environ, **{k: sub(v, values) for k, v in (app.get("env") or {}).items()}}
                procs.append(subprocess.Popen([sub(c, values) for c in app["command"]], cwd=sub(app.get("cwd", str(bundle)), values), env=env, start_new_session=True))
                ready = (app.get("ready") or {}).get("http")
                if ready:
                    wait_http(sub(ready, values))
            elif app["kind"] == "browser":
                url = sub(app["command"][-1], values)
        if url is None:
            raise RuntimeError("no browser app in the descriptor")
        profile = tmp / "chrome"
        chrome = subprocess.Popen([CHROME, "--headless=new", "--no-first-run", "--no-default-browser-check",
                                   f"--user-data-dir={profile}", "--remote-debugging-port=0", "--window-size=1280,900",
                                   "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        procs.append(chrome)
        port_file = profile / "DevToolsActivePort"
        for _ in range(100):
            if port_file.exists() and port_file.read_text().strip():
                break
            time.sleep(0.1)
        port = port_file.read_text().split()[0]
        req = urllib.request.Request(f"http://127.0.0.1:{port}/json/new?{url}", method="PUT")
        with urllib.request.urlopen(req) as r:
            page = json.load(r)
        time.sleep(1.0)
        done = subprocess.run(["node", str(HERE / "cdp_solve.mjs"), page["webSocketDebuggerUrl"], str(steps)],
                              capture_output=True, text=True, timeout=180)
        print("solver:", (done.stdout or done.stderr).strip()[-600:])
        if done.returncode != 0:
            return 2
    finally:
        for p in reversed(procs):
            try:
                os.killpg(p.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        time.sleep(0.5)
    run(desc["semantics"]["evaluate"])
    result = json.loads((tmp / "result.json").read_text())
    failed = {k: v["detail"] for k, v in result["checks"].items() if not v["passed"]}
    print(json.dumps({"passed": result["passed"], "score": result["score"], "failed": failed}, indent=1)[:2000])
    verdict = "pass" if result["passed"] else "fail"
    if not args.keep:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)
    else:
        print("kept", tmp)
    return 0 if verdict == args.expect else 1


if __name__ == "__main__":
    raise SystemExit(main())
