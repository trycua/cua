#!/usr/bin/env python3
"""Record a coding CLI running inside a real pseudo-terminal, as an asciinema v2 cast.

Why asciinema v2 and not a bespoke format
-----------------------------------------
The cast is a documented, line-oriented standard: a JSON header line followed by
one JSON array per chunk, ``[elapsed_seconds, "o"|"i", data]``.  That buys three
things this repo actually needs:

* it is inspectable with ``head``/``jq`` and diffable in review, which matters
  because these files are golden inputs;
* every event carries its own timestamp, so playback is a scan to a time rather
  than a guess about pacing;
* it is readable by tooling that has nothing to do with us (``asciinema play``,
  ``agg``, ``asciinema-player``), so a golden can be eyeballed by a human
  without running any Swift.

The one extension is the header's ``env``/custom keys, which the spec already
allows for: every cast this tool writes records ``cua_cli`` (which CLI) and
``cua_cli_version`` (its ``--version`` output).  Claude Code's rendering changes
between releases, so a recording that does not name its version is not evidence
about anything.

This recorder is deliberately CLI-agnostic: ``--cmd`` takes any argv.  Scripted
input is driven from a small step file so that recording ``codex`` is a matter
of writing another script, not another recorder.

Usage
-----
    python3 record.py --script scripts/basic-turn.json --out ../../libs/spaces-sdk-swift/Tests/.../basic-turn.cast
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import termios
import time

DEFAULT_COLS = 100
DEFAULT_ROWS = 34


# --------------------------------------------------------------------------
# Scrubbing
# --------------------------------------------------------------------------
# A recorded terminal session is a verbatim capture of whatever the CLI drew,
# which can include an API key echoed into a banner, a bearer token in an error,
# or the operator's home directory.  Everything committed to this repo goes
# through these rules.  They are applied to the *bytes* as recorded, before the
# cast is written, so the secret never reaches disk.
#
# Each rule replaces with a same-shaped placeholder where it can, because
# changing the width of a line changes how the terminal wraps it, and a golden
# whose wrapping differs from the real session is a golden that tests the
# scrubber rather than the CLI.

_SCRUB_RULES: list[tuple[re.Pattern[bytes], bytes]] = [
    # Anthropic-style keys.  Fixed prefix, variable tail.
    (re.compile(rb"sk-ant-[A-Za-z0-9_\-]{8,}"), b"sk-ant-REDACTED"),
    (re.compile(rb"sk-[A-Za-z0-9]{20,}"), b"sk-REDACTED"),
    # GitHub tokens.
    (re.compile(rb"gh[pousr]_[A-Za-z0-9]{16,}"), b"ghp_REDACTED"),
    # AWS access key ids.
    (re.compile(rb"AKIA[0-9A-Z]{16}"), b"AKIAREDACTEDREDACTED"),
    # Bearer tokens in any header-ish context.
    (re.compile(rb"(?i)(bearer\s+)[A-Za-z0-9._\-]{16,}"), rb"\1REDACTED"),
    # Claude Code's resume banner prints the session id on exit. Not a
    # credential, but it identifies a real transcript on the operator's
    # machine and is pure noise in a golden.
    (re.compile(rb"(?i)(--resume\s+)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"),
     rb"\g<1>00000000-0000-0000-0000-000000000000"),
    # JWTs.
    (re.compile(rb"eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}"),
     b"eyJREDACTED.REDACTED.REDACTED"),
]


# --------------------------------------------------------------------------
# Terminal query auto-responder
# --------------------------------------------------------------------------
# A bare pty has no terminal behind it, so nothing answers the capability
# queries a modern TUI sends on startup.  Claude Code probes for device
# attributes, the kitty keyboard protocol and the terminal name, and a session
# where those probes go unanswered does not render the same way as a session in
# a real terminal -- in practice it exits.  The recorder therefore answers them
# itself, minimally and honestly (it reports a vt220-class terminal with no
# kitty keyboard support), and records each reply as an ``i`` event so the cast
# shows exactly what the CLI was told.
_QUERY_REPLIES: list[tuple[re.Pattern[bytes], bytes]] = [
    (re.compile(rb"\x1b\[>0?q"), b"\x1bP>|cua-cast(1.0)\x1b\\"),   # XTVERSION
    (re.compile(rb"\x1b\[\?u"), b"\x1b[?0u"),                     # kitty keyboard
    (re.compile(rb"\x1b\[>0?c"), b"\x1b[>0;276;0c"),               # DA2
    (re.compile(rb"\x1b\[c"), b"\x1b[?62;1;2;6;9;15;22c"),       # DA1
    (re.compile(rb"\x1b\[5n"), b"\x1b[0n"),                       # DSR status
    (re.compile(rb"\x1b\]1[01];\?(\x07|\x1b\\)"), b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
]

_ANSI_RE = re.compile(
    rb"\x1b\][^\x07\x1b]*(\x07|\x1b\\)"       # OSC
    rb"|\x1bP[^\x1b]*\x1b\\"                   # DCS
    rb"|\x1b\[[0-9;?<>!$\"' ]*[@-~]"           # CSI
    rb"|\x1b[()][B0AU]"                       # charset
    rb"|\x1b[=>78MNOcDEHZ\\]"                  # short escapes
)


def flatten(data: bytes) -> bytes:
    """Strip escape sequences and whitespace.

    Used only by ``expect``.  Claude Code interleaves absolute column moves
    between individual *words* (``Enter\x1b[8Gto\x1b[11Gconfirm``), so a
    substring search against the raw stream finds nothing a human would call
    present.  Flattening both haystack and needle the same way makes the match
    mean "these characters were drawn, in this order", which is the weakest
    claim that is still useful and is never a claim about layout.
    """
    return re.sub(rb"\s+", b"", _ANSI_RE.sub(b"", data)).lower()


def _split_utf8(data: bytes) -> tuple[bytes, bytes]:
    """Split ``data`` into (complete UTF-8 prefix, incomplete trailing bytes)."""
    try:
        data.decode("utf-8")
        return data, b""
    except UnicodeDecodeError as exc:
        # Only a *truncated* sequence at the very end is held back.  A byte
        # that is simply not valid UTF-8 anywhere in the middle is passed
        # through and becomes U+FFFD, because holding it back would stall the
        # recording forever waiting for a continuation that never comes.
        if exc.end == len(data) and exc.end - exc.start < 4:
            return data[: exc.start], data[exc.start:]
        return data, b""


def scrub(data: bytes, home: bytes | None) -> bytes:
    """Apply every scrub rule to a chunk of recorded output."""
    for pattern, replacement in _SCRUB_RULES:
        data = pattern.sub(replacement, data)
    if home:
        # The operator's home path is not a secret in the cryptographic sense,
        # but it is their name, and it makes goldens machine-specific.
        data = data.replace(home, b"/Users/operator")
    return data


# --------------------------------------------------------------------------
# PTY plumbing
# --------------------------------------------------------------------------

def _set_winsize(fd: int, rows: int, cols: int) -> None:
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def _cli_version(argv: list[str]) -> str:
    """Best-effort ``--version``.  Recorded verbatim; never invented."""
    try:
        out = subprocess.run(argv[:1] + ["--version"], capture_output=True,
                             timeout=20, text=True)
        text = (out.stdout or out.stderr).strip().splitlines()
        return text[0] if text else "unknown"
    except Exception as exc:  # noqa: BLE001 - recording must not fail on this
        return f"unknown ({type(exc).__name__})"


# --------------------------------------------------------------------------
# The step language
# --------------------------------------------------------------------------
# A script is a JSON object: {"name":..., "description":..., "steps":[...]}.
# Steps are intentionally few, because anything cleverer is a test framework
# pretending to be a recorder.
#
#   {"wait": 2.5}                      sleep, letting the CLI draw
#   {"send": "hello"}                  write bytes (\n etc. honoured)
#   {"key": "enter"|"esc"|"ctrl-c"|"shift-tab"|"tab"|"up"|"down"}
#   {"expect": "substring", "timeout": 30}
#                                      wait until the substring appears in the
#                                      output so far, or the timeout elapses.
#                                      A timeout is recorded, not raised: an
#                                      interrupted or stuck session is exactly
#                                      the kind of golden we want.

_KEYS = {
    "enter": b"\r",
    "esc": b"\x1b",
    "ctrl-c": b"\x03",
    "ctrl-d": b"\x04",
    "tab": b"\t",
    "shift-tab": b"\x1b[Z",
    "up": b"\x1b[A",
    "down": b"\x1b[B",
    "backspace": b"\x7f",
}


class Recorder:
    def __init__(self, argv: list[str], cols: int, rows: int, idle_cap: float,
                 cwd: str | None = None):
        self.argv = argv
        self.cwd = cwd
        self.cols = cols
        self.rows = rows
        self.idle_cap = idle_cap
        self.events: list[tuple[float, str, str]] = []
        self.seen = bytearray()
        self.start = 0.0
        self.home = os.path.expanduser("~").encode()
        # A read() boundary lands wherever the kernel put it, which is
        # regularly in the middle of a UTF-8 sequence -- the box-drawing
        # characters Claude Code uses for every frame are three bytes each.
        # Splitting one across two cast events would make the cast invalid
        # UTF-8, so an incomplete trailing sequence is held back and prepended
        # to the next chunk.  Nothing is dropped and the ordering is unchanged;
        # only the chunk boundary moves.
        self._pending = b""

    # -- process lifetime ---------------------------------------------------

    def spawn(self) -> None:
        self.pid, self.fd = pty.fork()
        if self.pid == 0:  # child
            env = dict(os.environ)
            env["TERM"] = "xterm-256color"
            env["COLUMNS"] = str(self.cols)
            env["LINES"] = str(self.rows)
            # Keep the recording deterministic-ish and free of extra chrome.
            env["CI"] = ""
            # Strip the parent harness out of the child's environment.  When a
            # recording is driven from inside a Claude Code session, variables
            # like CLAUDECODE and CLAUDE_CODE_ENTRYPOINT make the child believe
            # it is nested and it exits immediately -- and
            # CLAUDE_CODE_MESSAGING_TOKEN is a live credential that has no
            # business being inherited by a process whose output we commit.
            for key in list(env):
                if key.startswith(("CLAUDE", "ANTHROPIC_LOG")) or key == "AI_AGENT":
                    env.pop(key, None)
            try:
                if self.cwd:
                    os.chdir(self.cwd)
                os.execvpe(self.argv[0], self.argv, env)
            except Exception:
                os._exit(127)
        _set_winsize(self.fd, self.rows, self.cols)
        self.start = time.time()

    def _drain(self, budget: float) -> None:
        """Read whatever is available for up to ``budget`` seconds."""
        deadline = time.time() + budget
        while True:
            remaining = deadline - time.time()
            if remaining <= 0:
                return
            try:
                ready, _, _ = select.select([self.fd], [], [], min(remaining, 0.1))
            except (OSError, ValueError):
                return
            if not ready:
                continue
            try:
                chunk = os.read(self.fd, 65536)
            except OSError as exc:
                if exc.errno in (errno.EIO, errno.EBADF):
                    self.fd = -1
                    return
                raise
            if not chunk:
                return
            self._record(chunk)

    def _record(self, chunk: bytes) -> None:
        cleaned = scrub(chunk, self.home)
        self.seen.extend(cleaned)
        self._answer_queries(chunk)
        elapsed = round(time.time() - self.start, 6)
        # Decode with surrogateescape so a split UTF-8 sequence at a chunk
        # boundary round-trips instead of being lost.  The player re-encodes
        # the same way; a byte that was never valid UTF-8 stays exactly as
        # recorded rather than becoming U+FFFD in the golden.
        self.events.append((elapsed, "o", cleaned.decode("utf-8", "replace")))

    def _answer_queries(self, chunk: bytes) -> None:
        for pattern, reply in _QUERY_REPLIES:
            if pattern.search(chunk):
                self._write(reply)

    def _write(self, data: bytes) -> None:
        if self.fd < 0:
            return
        try:
            os.write(self.fd, data)
        except OSError:
            # The child is gone.  That is a legitimate recording outcome (a
            # crashed or exited CLI), not a recorder failure.
            self.fd = -1
            return
        elapsed = round(time.time() - self.start, 6)
        # Scrub the input side too: a step script that types a token would
        # otherwise leave it in the cast even though the echoed output copy
        # was cleaned.
        cleaned = scrub(data, self.home)
        self.events.append((elapsed, "i", cleaned.decode("utf-8", "replace")))

    # -- steps --------------------------------------------------------------

    def run_steps(self, steps: list[dict]) -> list[str]:
        notes: list[str] = []
        for index, step in enumerate(steps):
            if self.fd < 0:
                notes.append(f"step {index}: pty closed, remaining steps skipped")
                break
            if "wait" in step:
                self._drain(float(step["wait"]))
            elif "send" in step:
                self._drain(0.2)
                self._write(step["send"].encode())
            elif "key" in step:
                self._drain(0.2)
                name = step["key"]
                if name not in _KEYS:
                    raise SystemExit(f"unknown key {name!r}")
                self._write(_KEYS[name])
            elif "expect" in step:
                needle = flatten(step["expect"].encode())
                timeout = float(step.get("timeout", 30))
                deadline = time.time() + timeout
                found = False
                while time.time() < deadline:
                    if needle in flatten(bytes(self.seen)):
                        found = True
                        break
                    self._drain(0.25)
                    if self.fd < 0:
                        break
                if not found:
                    notes.append(
                        f"step {index}: expect {step['expect']!r} not seen within {timeout}s"
                    )
            else:
                raise SystemExit(f"unrecognised step: {step!r}")
        # Let the last repaint land.
        self._drain(self.idle_cap)
        return notes

    def terminate(self) -> int | None:
        if self.fd >= 0:
            try:
                os.write(self.fd, b"\x03")
                self._drain(0.4)
                os.write(self.fd, b"\x04")
                self._drain(0.6)
            except OSError:
                pass
        try:
            os.kill(self.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        deadline = time.time() + 5
        while time.time() < deadline:
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                return os.waitstatus_to_exitcode(status)
            self._drain(0.1)
        try:
            os.kill(self.pid, signal.SIGKILL)
            _, status = os.waitpid(self.pid, 0)
            return os.waitstatus_to_exitcode(status)
        except (ProcessLookupError, ChildProcessError):
            return None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--script", required=True, help="JSON step script")
    ap.add_argument("--out", required=True, help="output .cast path")
    ap.add_argument("--cmd", default=None,
                    help="argv override, e.g. 'codex --model o3'")
    ap.add_argument("--cols", type=int, default=DEFAULT_COLS)
    ap.add_argument("--rows", type=int, default=DEFAULT_ROWS)
    ap.add_argument("--idle-cap", type=float, default=2.0)
    ap.add_argument("--cwd", default=None,
                    help="directory to run the CLI in (a scratch project, never "
                         "a live Space)")
    args = ap.parse_args()

    with open(args.script) as fh:
        script = json.load(fh)

    argv = (args.cmd.split() if args.cmd else script.get("cmd", ["claude"]))
    if shutil.which(argv[0]) is None:
        print(f"error: {argv[0]} not on PATH", file=sys.stderr)
        return 2

    version = _cli_version(argv)
    rec = Recorder(argv, args.cols, args.rows, args.idle_cap, args.cwd)
    rec.spawn()
    notes = rec.run_steps(script["steps"])
    exit_code = rec.terminate()

    header = {
        "version": 2,
        "width": args.cols,
        "height": args.rows,
        "timestamp": int(time.time()),
        "env": {"TERM": "xterm-256color", "SHELL": "/bin/zsh"},
        # Extensions.  Everything below is recorded fact, not inference.
        "cua_cli": argv[0],
        "cua_cli_argv": argv,
        "cua_cli_version": version,
        "cua_script": script.get("name", os.path.basename(args.script)),
        "cua_description": script.get("description", ""),
        "cua_exit_code": exit_code,
        "cua_scrubbed": [p.pattern.decode("utf-8", "replace") for p, _ in _SCRUB_RULES]
                        + ["$HOME -> /Users/operator"],
        "cua_notes": notes,
        "cua_cwd": args.cwd or os.getcwd(),
    }

    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as fh:
        fh.write(json.dumps(header) + "\n")
        for t, kind, data in rec.events:
            fh.write(json.dumps([t, kind, data], ensure_ascii=False) + "\n")

    size = os.path.getsize(args.out)
    print(f"wrote {args.out} ({size} bytes, {len(rec.events)} events, "
          f"{version}, exit={exit_code})")
    for note in notes:
        print(f"  note: {note}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
