#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""mcp-lazy-app — hot-load an app-backed MCP server.

Some MCP servers are only half a server: the process you spawn is a thin client
for an endpoint that lives *inside a GUI application*. blender-mcp is the
canonical case — its tools talk to 127.0.0.1:9876, which is served by an addon
running on Blender's modal event loop. `blender --background` does NOT serve it
(measured), so "the Blender MCP is available" used to be implemented as "keep
Blender running", and every Space launched Blender at boot whether or not the
task had anything to do with 3D. A user asking about a spreadsheet got Blender
on screen.

This shim breaks that coupling. It sits in front of such a server and speaks
plain MCP stdio in both directions:

  * `initialize` / `tools/list` are answered WITHOUT the application running, so
    the capability is advertised and selectable on a clean desktop. (Verified
    for blender-mcp: its tool list is static; it only opens the socket inside a
    tool handler.)
  * `tools/call` is the first genuine need. Before the call is forwarded the
    shim ENSURES the app: launch it if the readiness probe fails, then wait for
    the probe to pass.
  * The wait is bounded. On timeout the caller gets a clear tool error naming
    the app and the probe, not a connection-refused from somewhere underneath.

Everything app- and OS-specific lives in the JSON spec, not here: the launch
argv, the probe, the timeouts. The same file runs unchanged on Linux — a port
only has to write a spec whose `launch` is its own launcher (`setsid blender
--python ...`, `uwsm app -- blender`, ...).

Contract notes worth keeping:

  * READINESS IS THE SOCKET, NOT THE PROCESS. A previous guard used
    `pgrep -x Blender`, which passed for a Blender that macOS's Resume had
    launched without the addon's start_server operator ever firing: the process
    was there and 9876 was closed. The probe here is the endpoint answering.
  * IDEMPOTENCE ACROSS PROCESSES. Two agents (or two shims) calling at once must
    not launch two Blenders. An flock around launch+wait serialises them, and
    the second one re-probes inside the lock and finds the app already up.
    flock is released by the kernel when the holder dies, so a crashed launcher
    cannot deadlock the Space — unlike a lockfile with a pid in it.
  * FIRST-CALL LATENCY IS REAL. Cold Blender is seconds. The right behaviour is
    to WAIT, not to fail; the tool descriptions are annotated so the agent knows
    the first call may take a moment and does not treat it as a hang.

Usage:  mcp-lazy-app.py <spec.json>

Spec:
  {
    "app": "Blender",                       # for messages
    "ready": {"type": "tcp", "host": "127.0.0.1", "port": 9876},
    "launch": ["/usr/bin/open", "-n", "-a", "/Applications/Blender.app",
               "--args", "-noaudio", "--python", "/Users/lume/.cua/blender-mcp-start.py"],
    "ready_timeout_sec": 120,               # bound on the cold-start wait
    "lock_timeout_sec": 180,                # bound on waiting for another launcher
    "state_dir": "/Users/lume/.cua/lazy-apps",
    "upstream": {"command": "uvx", "args": ["blender-mcp"], "env": {}},
    "hint": "sentence appended to every tool description"
  }

Dependency-free (stdlib only) on purpose: it runs on the system python3 with no
virtualenv and cannot break because a resolver moved.
"""
import errno
import json
import os
import socket
import subprocess
import sys
import threading
import time

LOG_LOCK = threading.Lock()


def load_spec(path):
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


class Shim:
    def __init__(self, spec, spec_path):
        self.spec = spec
        self.app = spec.get("app") or "the application"
        self.ready_spec = spec.get("ready") or {}
        self.launch_argv = spec.get("launch") or []
        self.ready_timeout = float(spec.get("ready_timeout_sec", 120))
        self.lock_timeout = float(spec.get("lock_timeout_sec", 180))
        # How long to wait, on client EOF, for replies the upstream still owes.
        self.drain_timeout = float(spec.get("drain_timeout_sec", 10))
        self.hint = spec.get("hint") or (
            "First use in a Space starts %s on demand and may take a few "
            "seconds; later calls are immediate." % self.app
        )
        state_dir = spec.get("state_dir") or os.path.join(
            os.path.dirname(os.path.abspath(spec_path)), "state"
        )
        try:
            os.makedirs(state_dir, exist_ok=True)
        except OSError as exc:
            # Cannot use self.log() yet -- log_path is derived below from
            # state_dir, which is exactly what just failed. Go straight to
            # stderr, which is the MCP client's log. Falling back silently left
            # someone hunting for lock files in a directory we had given up on.
            print(
                "mcp-lazy-app: state dir %s unusable (%s); falling back to /tmp"
                % (state_dir, exc),
                file=sys.stderr,
            )
            state_dir = "/tmp"
        base = os.path.splitext(os.path.basename(spec_path))[0]
        self.lock_path = os.path.join(state_dir, base + ".lock")
        self.log_path = os.path.join(state_dir, base + ".log")
        # Once the app has been seen ready, skip the probe on the hot path only
        # after re-checking cheaply -- the probe IS cheap, so just always probe.
        self.child = None
        self.child_lock = threading.Lock()
        # Request ids forwarded upstream and not yet answered. _drain() waits on
        # these before closing the child's stdin: closing it is what makes a
        # well-behaved MCP server exit, and doing that with a request still in
        # flight kills the answer. blender-mcp does exactly this -- its
        # tools/list handler probes Blender first, so on a cold desktop the
        # handler is still running when a one-shot client reaches EOF, and the
        # server shuts down mid-request. That made the clean-desktop CI gate
        # NONDETERMINISTIC: same image, sometimes 28 tools, sometimes none.
        self.pending = set()
        self.pending_lock = threading.Lock()
        self.pending_done = threading.Condition(self.pending_lock)

    # -- logging ------------------------------------------------------------
    def error(self, msg):
        """Log at error severity.

        A thin wrapper over log() rather than a second mechanism: it marks the
        lines a human debugging a Space actually wants to grep for, and it lets
        static analysis see that an except branch reports rather than swallows.
        """
        self.log("ERROR " + msg)

    def log(self, msg):
        line = "%s %s\n" % (time.strftime("%Y-%m-%dT%H:%M:%S"), msg)
        with LOG_LOCK:
            try:
                with open(self.log_path, "a", encoding="utf-8") as fh:
                    fh.write(line)
            # This IS the logger. Reporting a logging failure through the
            # logger would recurse, and the line is not lost: the stderr write
            # below is unconditional, so the message still reaches the MCP
            # client's log even when the log FILE cannot be written.
            # lint-ignore: swallowed-exception
            except OSError:
                pass
            # stderr is the MCP client's log; stdout is the protocol and must
            # never carry anything but JSON-RPC.
            sys.stderr.write(line)
            sys.stderr.flush()

    # -- readiness ----------------------------------------------------------
    def probe(self):
        """True when the app's endpoint is actually answering."""
        kind = self.ready_spec.get("type", "tcp")
        if kind == "tcp":
            host = self.ready_spec.get("host", "127.0.0.1")
            port = int(self.ready_spec.get("port", 0))
            try:
                with socket.create_connection((host, port), timeout=2):
                    return True
            # A refused connection is this function's expected NEGATIVE RESULT,
            # not an error: "the app is not up yet" is precisely what the caller
            # asked. ensure() polls this once a second for up to ready_timeout,
            # so reporting here would put ~120 identical lines in the log for
            # every normal cold start and bury the one line that matters. The
            # real failure -- the probe never passing -- is reported by ensure().
            # lint-ignore: swallowed-exception
            except OSError:
                return False
        if kind == "command":
            try:
                return subprocess.call(
                    self.ready_spec["argv"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=self.ready_spec.get("timeout_sec", 10),
                ) == 0
            # Same contract as the tcp probe above: a non-zero exit, a missing
            # binary or a timeout all mean "not ready yet", which is the answer
            # the caller wants, on a once-a-second polling path.
            # lint-ignore: swallowed-exception
            except Exception:
                return False
        if kind == "none":
            return True
        raise ValueError("unknown readiness probe type %r" % kind)

    def probe_description(self):
        kind = self.ready_spec.get("type", "tcp")
        if kind == "tcp":
            return "%s:%s" % (
                self.ready_spec.get("host", "127.0.0.1"),
                self.ready_spec.get("port"),
            )
        if kind == "command":
            return " ".join(self.ready_spec.get("argv", []))
        return kind

    def ensure(self):
        """Make the app's endpoint ready. Returns None on success, else an error string."""
        if self.probe():
            return None
        if not self.launch_argv:
            return (
                "%s is not running and this Space has no launch command for it "
                "(%s is not answering)." % (self.app, self.probe_description())
            )

        lock_fd = None
        try:
            lock_fd = self._acquire_lock()
        except TimeoutError:
            # Somebody else has been launching for longer than we are willing to
            # wait. Do NOT steal the lock and launch a second copy; report.
            if self.probe():
                return None
            self.error(
                "gave up after %.0fs waiting for another launcher of %s"
                % (self.lock_timeout, self.app)
            )
            return (
                "Timed out after %.0fs waiting for another request to finish "
                "starting %s. Retry in a moment." % (self.lock_timeout, self.app)
            )
        except OSError as exc:
            # A lock we cannot even create must not make the Space unusable.
            self.error("lock unavailable (%s); proceeding unlocked" % exc)

        try:
            # Re-probe INSIDE the lock: the holder we just waited on has very
            # likely started the app already, and launching a second Blender is
            # exactly the bug this lock exists to prevent.
            if self.probe():
                return None
            self.log("launching %s: %s" % (self.app, " ".join(self.launch_argv)))
            try:
                subprocess.Popen(
                    self.launch_argv,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    start_new_session=True,  # survive this shim; portable
                )
            except OSError as exc:
                self.error("could not start %s: %s" % (self.app, exc))
                return "Could not start %s: %s" % (self.app, exc)

            deadline = time.time() + self.ready_timeout
            while time.time() < deadline:
                if self.probe():
                    self.log("%s ready at %s" % (self.app, self.probe_description()))
                    return None
                time.sleep(1.0)
            return (
                "%s was started but %s did not begin answering within %.0fs. "
                "The application may still be loading, or it may have come up "
                "without its MCP endpoint. Retry once; if it fails again the "
                "Space needs attention."
                % (self.app, self.probe_description(), self.ready_timeout)
            )
        finally:
            if lock_fd is not None:
                try:
                    os.close(lock_fd)  # closing releases the flock
                # Best-effort teardown of a descriptor we own. The flock is
                # released by the kernel when this process exits regardless, so
                # a failed close cannot strand the lock or affect the caller.
                # lint-ignore: swallowed-exception
                except OSError:
                    pass

    def _acquire_lock(self):
        import fcntl  # POSIX; the Windows port would swap this for msvcrt

        fd = os.open(self.lock_path, os.O_CREAT | os.O_RDWR, 0o644)
        deadline = time.time() + self.lock_timeout
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return fd
            except OSError as exc:
                if exc.errno not in (errno.EACCES, errno.EAGAIN):
                    os.close(fd)
                    raise
            # While waiting, the other holder may have finished successfully.
            if self.probe():
                os.close(fd)
                raise _AlreadyReady()
            if time.time() >= deadline:
                os.close(fd)
                raise TimeoutError()
            time.sleep(0.5)

    # -- upstream child -----------------------------------------------------
    def start_child(self):
        up = self.spec.get("upstream") or {}
        argv = [up["command"]] + list(up.get("args") or [])
        env = dict(os.environ)
        env.update(up.get("env") or {})
        self.child = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,  # let the upstream's own logging reach the client's log
            env=env,
            text=True,
            bufsize=1,
        )

    def pump_child_to_client(self):
        """Upstream -> client, annotating tools/list so the latency is documented."""
        for line in self.child.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except ValueError:
                # Upstream stdout is the MCP protocol channel and must carry
                # nothing else. Dropping a malformed line silently would hide a
                # server printing a banner or a traceback over the protocol.
                self.error("ignoring non-JSON line from upstream: %.200r" % line)
                continue
            if msg.get("id") is not None and "method" not in msg:
                with self.pending_done:
                    self.pending.discard(msg["id"])
                    self.pending_done.notify_all()
            result = msg.get("result")
            if isinstance(result, dict) and isinstance(result.get("tools"), list):
                for tool in result["tools"]:
                    desc = tool.get("description") or ""
                    if self.hint not in desc:
                        tool["description"] = (desc.rstrip() + "\n\n" + self.hint).strip()
            self.write_client(msg)
        # Upstream died: nothing more will ever arrive. Exiting lets the client
        # see a closed server rather than hanging on every request.
        self.log("upstream exited")
        os._exit(0)

    def write_client(self, msg):
        with LOG_LOCK:
            sys.stdout.write(json.dumps(msg) + "\n")
            sys.stdout.flush()

    def write_child(self, msg):
        if msg.get("id") is not None and msg.get("method"):
            with self.pending_done:
                self.pending.add(msg["id"])
        self.child.stdin.write(json.dumps(msg) + "\n")
        self.child.stdin.flush()

    # -- main loop ----------------------------------------------------------
    def serve(self):
        self.start_child()
        pump = threading.Thread(target=self.pump_child_to_client, daemon=True)
        pump.start()
        try:
            self._read_client(pump)
        finally:
            pass

    def _drain(self, pump):
        """Let in-flight upstream replies reach the client before we exit.

        On stdin EOF the read loop returns immediately, main() terminates the
        child, and the pump -- a daemon thread -- is killed with the process.
        Any reply the upstream had not yet written was silently DROPPED. A
        long-lived MCP client never hits this because it holds stdin open, but
        anything that feeds a fixed script and closes (a CI smoke test, a
        one-shot probe) loses every response and looks like a server that
        answered nothing. Closing the child's stdin makes a well-behaved
        upstream finish and close its stdout, which ends the pump normally.
        """
        # Wait for anything still in flight FIRST. Closing stdin below is a
        # shutdown signal to the upstream, so sending it early is what loses the
        # reply we are trying to preserve.
        deadline = time.time() + self.drain_timeout
        with self.pending_done:
            while self.pending and time.time() < deadline:
                self.pending_done.wait(timeout=0.2)
            still = sorted(self.pending)
        if still:
            self.error(
                "client closed with %d request(s) unanswered after %.0fs: %s"
                % (len(still), self.drain_timeout, still)
            )
        try:
            if self.child and self.child.stdin and not self.child.stdin.closed:
                self.child.stdin.close()
        # Shutdown path: the child is on its way out and an already-closed or
        # broken stdin is the normal race here, not a fault. The join below
        # still bounds how long we wait for its remaining output.
        # lint-ignore: swallowed-exception
        except (OSError, ValueError):
            pass
        pump.join(timeout=self.drain_timeout)

    def _read_client(self, pump):
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except ValueError:
                # Same reasoning as the upstream pump: a client that sends us
                # something unparseable is a bug worth seeing, not noise.
                self.error("ignoring non-JSON line from client: %.200r" % line)
                continue
            if msg.get("method") == "tools/call" and msg.get("id") is not None:
                # THE hot-load point. Everything else -- initialize, tools/list,
                # notifications -- passes straight through with the app still
                # closed, which is what keeps the desktop clean and the
                # capability visible at the same time.
                self._handle_call(msg)
                continue
            try:
                self.write_child(msg)
            except (BrokenPipeError, ValueError):
                self.error("upstream pipe closed while forwarding a request")
                self._drain(pump)
                return
        # stdin EOF: the client is done sending, but the upstream may still owe
        # us replies. Drain before returning, or they are lost.
        self._drain(pump)

    def _handle_call(self, msg):
        try:
            err = self.ensure()
        # _AlreadyReady is control flow, not a failure: it is raised out of the
        # lock wait precisely BECAUSE the app came up while we waited, which is
        # the success case. err = None is the correct, complete handling.
        # lint-ignore: swallowed-exception
        except _AlreadyReady:
            err = None
        # The exception is formatted into `err`, which the `if err:` branch
        # immediately below logs via self.error() and returns to the agent as a
        # tool error. Reporting here too would duplicate every such line.
        # lint-ignore: swallowed-exception
        except Exception as exc:  # never take the server down over a probe
            err = "Could not ensure %s is running: %s" % (self.app, exc)
        if err:
            self.error("ensure failed: %s" % err)
            self.write_client({
                "jsonrpc": "2.0",
                "id": msg.get("id"),
                "result": {"content": [{"type": "text", "text": err}], "isError": True},
            })
            return
        try:
            self.write_child(msg)
        except (BrokenPipeError, ValueError):
            self.error("upstream exited before it could answer a tools/call")
            self.write_client({
                "jsonrpc": "2.0",
                "id": msg.get("id"),
                "result": {
                    "content": [{"type": "text", "text": "the %s MCP server exited" % self.app}],
                    "isError": True,
                },
            })


class _AlreadyReady(Exception):
    """Raised out of the lock wait when the app came up while we waited."""


def main():
    if len(sys.argv) != 2:
        sys.stderr.write("usage: mcp-lazy-app.py <spec.json>\n")
        return 2
    spec_path = sys.argv[1]
    shim = Shim(load_spec(spec_path), spec_path)
    try:
        shim.serve()
    # Ctrl-C / SIGINT is an ordinary way for a client to stop this server. The
    # finally below still terminates the child; there is no error to report.
    # lint-ignore: swallowed-exception
    except KeyboardInterrupt:
        pass
    finally:
        if shim.child and shim.child.poll() is None:
            shim.child.terminate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
