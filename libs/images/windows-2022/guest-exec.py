#!/usr/bin/env python3
"""Run one PowerShell script in the Windows build guest.

    guest-exec.py --url http://127.0.0.1:PORT [--timeout 600] [--fetch-base URL] SCRIPT.ps1 [ARG...]
    guest-exec.py --url ... --wait 1800        # wait until the guest answers

The base disk (the published Windows workspace) runs the legacy
computer-server on :8000 in the interactive session; the image build uses
its `run_command` only to provision the guest (build-image.sh). The script
is sent as `powershell -EncodedCommand` (UTF-16LE base64) so no quoting
survives cmd.exe. cmd.exe caps a command line at 8191 characters, so with
--fetch-base (the build's file server as the guest sees it, e.g.
http://10.0.2.2:PORT) only a short loader is sent and the guest downloads
SCRIPT from there. Prints the guest's stdout and stderr and exits with the
guest's exit code. Stdlib only.
"""

import argparse
import base64
import json
import re
import sys
import time
import urllib.error
import urllib.request

MAX_REPLY = 16 * 1024 * 1024


def post(url, body, timeout):
    req = urllib.request.Request(
        url.rstrip("/") + "/cmd",
        data=json.dumps(body).encode(),
        headers={"content-type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        text = resp.read(MAX_REPLY + 1)
    if len(text) > MAX_REPLY:
        raise RuntimeError("reply too large")
    text = text.decode("utf-8", "replace")
    for line in text.splitlines():
        if line.startswith("data:"):
            return json.loads(line[5:].strip())
    return json.loads(text)


def wait(url, budget):
    deadline = time.monotonic() + budget
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(url.rstrip("/") + "/status", timeout=5) as resp:
                if resp.status == 200:
                    return 0
        except (urllib.error.URLError, OSError, ValueError):
            pass
        time.sleep(5)
    print(f"the guest did not answer at {url} within {budget}s", file=sys.stderr)
    return 2


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--wait", type=int, default=0)
    ap.add_argument("--fetch-base", default="")
    ap.add_argument("script", nargs="?")
    # Everything after SCRIPT is the script's own (PowerShell `-Name value`).
    ap.add_argument("args", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    if a.wait:
        return wait(a.url, a.wait)
    if not a.script:
        ap.error("SCRIPT is required")
    # `-Name` stays bare so PowerShell binds it as a parameter name; values
    # are single-quoted literals.
    quoted = " ".join(
        x if re.fullmatch(r"-[A-Za-z][A-Za-z0-9]*", x) else "'" + x.replace("'", "''") + "'"
        for x in a.args
    )
    if a.fetch_base:
        name = a.script.replace("\\", "/").rsplit("/", 1)[-1]
        body = (
            "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue';"
            f"$p=Join-Path $env:TEMP 'cua-build-{name}';"
            f"Invoke-WebRequest -UseBasicParsing '{a.fetch_base.rstrip('/')}/{name}' -OutFile $p;"
            f"& $p {quoted}; exit $LASTEXITCODE"
        )
    else:
        with open(a.script, encoding="utf-8") as f:
            body = f.read()
        if a.args:
            ap.error("arguments need --fetch-base")
    encoded = base64.b64encode(body.encode("utf-16-le")).decode()
    line = f"powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {encoded}"
    if len(line) > 8000:
        ap.error(f"{a.script} is too long to send inline; pass --fetch-base")
    reply = post(a.url, {"command": "run_command", "params": {"command": line}}, a.timeout)
    sys.stdout.write(reply.get("stdout") or "")
    sys.stderr.write(reply.get("stderr") or "")
    if reply.get("success") is False and "return_code" not in reply:
        print(f"run_command failed: {reply.get('error')}", file=sys.stderr)
        return 1
    return int(reply.get("return_code") or 0)


if __name__ == "__main__":
    sys.exit(main())
