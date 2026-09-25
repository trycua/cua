"""Opt-in native v3 HELLO-deadline probe; no application input is sent.

Run in a clean disposable Linux Hyprland session with the production plugin
enabled, no Driver/trace/operator clients, and both input endpoints available:

    python3 production_handshake_probe.py --clean-session \
        --compositor-pid PID --input-directory "$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE"

The controller must verify candidate/artifact provenance and capture stdout as
private evidence. This focused probe does not certify the desktop matrix or
the post-HELLO idle timeout. Portable unit tests are not native evidence.
"""

import argparse
import json
import os
from pathlib import Path
import re
import select
import socket
import stat
import struct
import subprocess
import sys
import time


ENDPOINTS = ("cua-input-v3.sock", "cua-input-v3-2.sock")
CAPACITY = 8
CLOSE_BOUND = 6.5
IO_TIMEOUT = 0.5
CLOSED = object()


class ProbeFailure(Exception):
    """Only fixed, path-free reason codes are exposed in evidence."""


def require(condition, reason):
    if not condition:
        raise ProbeFailure(reason)


def validate_path(path, *, directory=False, private=True):
    metadata = path.lstat()
    require(path.is_absolute() and path == path.resolve(strict=True), "noncanonical_path")
    require(metadata.st_uid == os.getuid(), "wrong_path_owner")
    if directory:
        # The private runtime directory confines descendants. Hyprland may
        # create readable child directories; they must not be writable by peers.
        require(stat.S_ISDIR(metadata.st_mode) and not metadata.st_mode & (0o077 if private else 0o022),
                "nonprivate_directory")
    else:
        require(stat.S_ISSOCK(metadata.st_mode) and stat.S_IMODE(metadata.st_mode) == 0o600,
                "invalid_socket_type_or_mode")


def ctl(*arguments):
    try:
        return json.loads(subprocess.check_output(
            ["hyprctl", "-j", *arguments], text=True, stderr=subprocess.DEVNULL,
            timeout=IO_TIMEOUT))
    except (OSError, subprocess.SubprocessError, ValueError):
        raise ProbeFailure("compositor_unresponsive") from None


def preflight(args):
    require(sys.platform == "linux" and args.clean_session, "clean_linux_session_required")
    require(args.compositor_pid > 0, "invalid_compositor_pid")
    instance = os.environ.get("HYPRLAND_INSTANCE_SIGNATURE", "")
    require(bool(re.fullmatch(r"[a-zA-Z0-9_.-]+", instance)) and instance not in (".", ".."),
            "invalid_instance_signature")
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", ""))
    expected = runtime / "hypr" / instance
    require(args.input_directory == expected, "wrong_instance_directory")
    for directory in (runtime, runtime / "hypr", expected):
        validate_path(directory, directory=True, private=directory == runtime)
    instances = ctl("instances")
    require(isinstance(instances, list), "invalid_instances_reply")
    matches = [row for row in instances if isinstance(row, dict) and row.get("instance") == instance]
    require(len(matches) == 1 and matches[0].get("pid") == args.compositor_pid,
            "wrong_compositor_instance")
    process = Path(f"/proc/{args.compositor_pid}")
    require(process.stat().st_uid == os.getuid()
            and (process / "exe").resolve(strict=True).name == "Hyprland",
            "wrong_compositor_process")
    paths = [expected / name for name in ENDPOINTS]
    for path in paths:
        validate_path(path)
    return paths


def validate_reply(value, *, hello=False):
    require(isinstance(value, dict), "reply_not_object")
    if hello:
        require(value.get("ok") is True and type(value.get("protocol")) is int
                and value["protocol"] == 3 and isinstance(value.get("epoch"), str)
                and bool(re.fullmatch(r"[0-9a-f]{32}", value["epoch"])), "invalid_hello_reply")
    else:
        require(value.get("ok") is False and value.get("code") == "invalid_request",
                "invalid_prehello_refusal")


class NativeRuntime:
    clock = staticmethod(time.monotonic)
    sleep = staticmethod(time.sleep)

    def __init__(self, pid):
        self.pid = pid

    def health(self):
        require(isinstance(ctl("version"), dict), "invalid_version_reply")

    def connect(self, path):
        validate_path(path)
        client = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        try:
            client.settimeout(IO_TIMEOUT)
            client.connect(str(path))
            pid, uid, _ = struct.unpack("3i", client.getsockopt(
                socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i")))
            require(pid == self.pid and uid == os.getuid(), "wrong_socket_peer")
            return client
        except BaseException:
            client.close()
            raise

    def exchange(self, client, packet):
        try:
            require(client.send(packet) == len(packet), "short_packet_send")
            reply = client.recv(2049)
        except (BrokenPipeError, ConnectionResetError):
            return CLOSED
        except TimeoutError:
            raise ProbeFailure("reply_timeout") from None
        if not reply:
            return CLOSED
        require(len(reply) <= 2048, "oversized_reply")
        try:
            return json.loads(reply)
        except (ValueError, UnicodeError):
            raise ProbeFailure("malformed_reply") from None

    def closed(self, client):
        if not select.select([client], [], [], 0)[0]:
            return False
        try:
            packet = client.recv(2049)
        except ConnectionResetError:
            return True
        require(not packet, "unsolicited_reply")
        return True


def probe_endpoint(path, runtime, evidence):
    """Fill every slot, observe hard expiry, then retain eight fresh HELLOs.

    runtime is injectable for portable failure-path tests. Times are observed
    from local connect, not a claim to know the server's precise accept time.
    All sockets are closed in finally, including failures and interrupts.
    """
    sockets = []
    started = runtime.clock()
    rows = evidence.setdefault("clients", [])
    evidence.update(result="failed", capacity=CAPACITY, close_bound_seconds=CLOSE_BOUND,
                    health_checks=[], recovery_hello_count=0, cleanup_complete=False)

    def health():
        before = runtime.clock()
        runtime.health()
        evidence["health_checks"].append({"at_seconds": round(before - started, 3),
                                         "duration_seconds": round(runtime.clock() - before, 3)})

    def refusal(client, row):
        value = runtime.exchange(client, b"CLAIM")
        if value is CLOSED:
            return False
        validate_reply(value)
        row["refusal_seconds"].append(round(runtime.clock() - row["opened"], 3))
        return True

    def mark_closed(row):
        elapsed = runtime.clock() - row["opened"]
        row["closed_seconds"] = round(elapsed, 3)
        require(4.75 <= elapsed <= CLOSE_BOUND, "closure_outside_deadline_window")
        require(len(row["refusal_seconds"]) >= 4 and row["refusal_seconds"][-1] >= 3,
                "insufficient_continued_prehello_traffic")

    try:
        health()
        for index in range(CAPACITY):
            opened = runtime.clock()
            client = runtime.connect(path)
            sockets.append(client)
            row = {"slot": index, "opened": opened, "refusal_seconds": []}
            rows.append(row)
            require(refusal(client, row), "initial_connection_refused")
        require(runtime.clock() - rows[0]["opened"] <= 1, "slow_initial_admission")
        pending = list(zip(sockets, rows))
        next_health = runtime.clock()
        while pending:
            if runtime.clock() >= next_health:
                health()
                next_health = runtime.clock() + 1
            for client, row in list(pending):
                elapsed = runtime.clock() - row["opened"]
                require(elapsed <= CLOSE_BOUND, "hard_hello_deadline_exceeded")
                if runtime.closed(client):
                    mark_closed(row)
                    pending.remove((client, row))
                elif elapsed >= len(row["refusal_seconds"]):
                    if not refusal(client, row):
                        mark_closed(row)
                        pending.remove((client, row))
            if pending:
                runtime.sleep(0.025)
        health()
        epochs = set()
        for _ in range(CAPACITY):
            client = runtime.connect(path)
            sockets.append(client)
            reply = runtime.exchange(client, b"HELLO")
            require(reply is not CLOSED, "capacity_recovery_failed")
            validate_reply(reply, hello=True)
            epochs.add(reply["epoch"])
            evidence["recovery_hello_count"] += 1
        require(len(epochs) == 1, "endpoint_epoch_changed")
        health()
        evidence["result"] = "passed"
    finally:
        cleanup_failed = False
        for client in sockets:
            try:
                client.close()
            except OSError:
                cleanup_failed = True
        for row in rows:
            row["opened_seconds"] = round(row.pop("opened") - started, 3)
        evidence["cleanup_complete"] = not cleanup_failed
        evidence["duration_seconds"] = round(runtime.clock() - started, 3)
        if cleanup_failed:
            evidence["result"] = "failed"
            raise ProbeFailure("socket_cleanup_failed")


def run(args):
    result = {"result": "failed", "native_run": True, "full_desktop_matrix": False,
              "post_hello_idle_tested": False, "endpoints": []}
    try:
        paths = preflight(args)
        runtime = NativeRuntime(args.compositor_pid)
        for path in paths:
            evidence = {"endpoint": path.name}
            result["endpoints"].append(evidence)
            probe_endpoint(path, runtime, evidence)
        result["result"] = "passed"
    except ProbeFailure as error:
        result["failure"] = str(error)
    except (OSError, ValueError, RuntimeError):
        result["failure"] = "native_io_failure"
    except KeyboardInterrupt:
        result["failure"] = "interrupted"
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--clean-session", action="store_true",
                        help="confirm a disposable session without other input endpoint clients")
    parser.add_argument("--compositor-pid", type=int, required=True)
    parser.add_argument("--input-directory", type=Path, required=True)
    result = run(parser.parse_args())
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["result"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
