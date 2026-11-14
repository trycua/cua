#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Minimal stdlib MCP adapter for browser-use/macos-harness 0.2.1 (bench arm `cc-browser-use`).

macos-harness is a shell CLI whose work is done by a per-user helper daemon (started on the first call). The vendor
skill teaches a shell ("actions you are sure of, then one read, joined with &&"). Bench v2 gives the model no shell, so
this adapter exposes the vendor's own surface unchanged through ONE tool:

  macos_harness(command)   e.g.  macos-harness click --app "TextEdit" --element_index 12 && macos-harness get_app_state --app "TextEdit"
  view_screenshot(path)    stand-in for the Read tool the skill assumes ("open the file on the Screenshot line")

The command string is parsed here (shlex, no shell): only the vendor tools and `skill`, joined by `&&`, with one optional
trailing heredoc for paste/type_text/set_value. Pipes, `;`, `||`, redirects and `doctor`, `install`, `approve`, `helper`
and the like are refused. The server instructions are the output of `macos-harness skill` (the vendor SKILL.md), so the
text always matches the installed version. view_screenshot only opens files that macos-harness printed on a Screenshot line.

Environment:
  BU_BIN        path of the macos-harness executable (default: `macos-harness` on PATH)
  BU_SHOT       ondemand (default, vendor-faithful) | always (attach the screenshot to every get_app_state)
  BU_IMG_MAX    optional longest-side downscale in pixels (uses macOS `sips`); 0 = off
"""

import base64
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile

BIN = os.environ.get("BU_BIN") or "macos-harness"
SHOT = os.environ.get("BU_SHOT", "ondemand")
IMG_MAX = int(os.environ.get("BU_IMG_MAX", "0") or 0)
ALLOWED = {
    "list_apps",
    "get_app_state",
    "click",
    "perform_secondary_action",
    "set_value",
    "select_text",
    "scroll",
    "drag",
    "press_key",
    "type_text",
    "paste",
    "skill",
}
HEREDOC = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1[ \t]*\n(.*?)\n\2[ \t]*\Z", re.S)
SHOT_RE = re.compile(r"^Screenshot \(\d+x\d+ px\): (.+)$", re.M)


def parse(command):
    """-> list of (argv, stdin|None). Raises ValueError with a message the model can act on."""
    command = command.strip()
    stdin = None
    m = HEREDOC.search(command)
    if m:
        stdin, command = m.group(3), command[: m.start()].rstrip()
    lex = shlex.shlex(command, posix=True, punctuation_chars=True)
    lex.whitespace_split = True
    segs, cur = [], []
    for t in lex:
        if t == "&&":
            segs.append(cur)
            cur = []
        elif set(t) <= set("();<>|&"):
            raise ValueError(
                "only `macos-harness ...` commands joined by && are allowed (got %r)" % t
            )
        else:
            cur.append(t)
    segs.append(cur)
    out = []
    for i, s in enumerate(segs):
        if s[:1] == ["macos-harness"]:
            s = s[1:]
        if not s or s[0] not in ALLOWED:
            raise ValueError(
                "unknown or not allowed command %r; use only the commands on the skill page"
                % (s[:1] or "")
            )
        out.append((s, stdin if i == len(segs) - 1 else None))
    return out


class Backend:
    def __init__(self):
        self.seen = set()  # screenshot paths macos-harness printed

    def cli(self, argv, stdin):
        try:
            r = subprocess.run(
                [BIN, *argv],
                input=stdin.encode() if stdin is not None else None,
                capture_output=True,
                timeout=150,
            )
        except subprocess.TimeoutExpired:
            return 124, b"timed out"
        return r.returncode, r.stdout + r.stderr

    def cat(self, path):
        if path not in self.seen:
            return 1, b"only screenshots that macos-harness wrote can be opened"
        try:
            with open(path, "rb") as f:
                return 0, f.read()
        except OSError as e:
            return 1, str(e).encode()


def shrink(data):
    if not IMG_MAX:
        return data
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "s.jpeg")
        open(p, "wb").write(data)
        subprocess.run(["/usr/bin/sips", "-Z", str(IMG_MAX), p], capture_output=True)
        return open(p, "rb").read()


def img(data):
    return {
        "type": "image",
        "data": base64.b64encode(shrink(data)).decode(),
        "mimeType": "image/jpeg",
    }


def run_command(be, command):
    try:
        steps = parse(command)
    except ValueError as e:
        return [{"type": "text", "text": "usage error: %s" % e}], True
    texts, content, last_shot, err = [], [], None, False
    for argv, stdin in steps:
        rc, raw = be.cli(argv, stdin)
        t = raw.decode("utf-8", "replace")
        texts.append(t)
        m = SHOT_RE.search(t)
        if m:
            last_shot = m.group(1).strip()
            be.seen.add(last_shot)
        if rc != 0:
            err = True
            break  # && semantics
    content.append({"type": "text", "text": "\n".join(texts)})
    if SHOT == "always" and last_shot and not err and steps[-1][0][0] == "get_app_state":
        rc, raw = be.cat(last_shot)
        if rc == 0:
            content.append(img(raw))
    return content, err


def tools():
    t = [
        {
            "name": "macos_harness",
            "description": "Run macos-harness commands, exactly as the skill teaches (instructions). One call may chain commands with &&; it stops at the first failure. For paste/type_text/set_value with quotes or several lines, end the command with a heredoc (<<'EOF' ... EOF). No other shell syntax.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": 'e.g. macos-harness click --app "TextEdit" --element_index 12 && macos-harness get_app_state --app "TextEdit"',
                    }
                },
                "required": ["command"],
            },
        },
        {
            "name": "view_screenshot",
            "description": "Open a screenshot file named on a `Screenshot (WxH px): <path>` line (the skill's 'open the file'). Only files macos-harness wrote.",
            "inputSchema": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
            },
        },
    ]
    return t


def call(be, name, a):
    if name == "macos_harness":
        return run_command(be, a.get("command", ""))
    if name == "view_screenshot":
        rc, raw = be.cat(a.get("path", ""))
        return (
            ([img(raw)], False)
            if rc == 0
            else ([{"type": "text", "text": raw.decode("utf-8", "replace")}], True)
        )
    raise KeyError(name)


def serve():
    be = Backend()
    instr = be.cli(["skill"], None)[1].decode("utf-8", "replace")
    for line in sys.stdin:
        m = json.loads(line)
        mid, meth = m.get("id"), m.get("method")

        def send(r):
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": mid, **r}) + "\n")
            sys.stdout.flush()

        if meth == "initialize":
            send(
                {
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "browseruse", "version": "bench-adapter"},
                        "instructions": instr,
                    }
                }
            )
        elif meth == "tools/list":
            send({"result": {"tools": tools()}})
        elif meth == "tools/call":
            p = m["params"]
            try:
                c, e = call(be, p["name"], p.get("arguments") or {})
                send({"result": {"content": c, "isError": e}})
            except Exception as ex:
                send(
                    {
                        "result": {
                            "content": [{"type": "text", "text": "adapter error: %s" % ex}],
                            "isError": True,
                        }
                    }
                )
        elif mid is not None:
            send({"result": {}})


if __name__ == "__main__":
    serve()
