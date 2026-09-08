"""Portable helper tests; these do not exercise or certify native Hyprland."""

import json
import os
from pathlib import Path
import socket
import stat
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from production_handshake_probe import (
    CAPACITY, CLOSED, ENDPOINTS, NativeRuntime, ProbeFailure, preflight,
    probe_endpoint, run, validate_path, validate_reply,
)


REFUSAL = {"ok": False, "code": "invalid_request"}
HELLO = {"ok": True, "protocol": 3, "epoch": "a" * 32}


class FakeClient:
    def __init__(self, opened, path):
        self.opened = opened
        self.path = path
        self.closed = False
        self.packets = []

    def close(self):
        self.closed = True


class FakeRuntime:
    def __init__(self, expiry=5.05, refusal=REFUSAL, hello=HELLO, recovery_limit=8):
        self.now = 10
        self.expiry = expiry
        self.refusal = refusal
        self.hello = hello
        self.recovery_limit = recovery_limit
        self.clients = []
        self.hellos = 0
        self.health_checks = 0

    def clock(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds

    def health(self):
        self.health_checks += 1

    def connect(self, path):
        if self.clients and self.clients[-1].path != path:
            assert all(client.closed for client in self.clients)
        client = FakeClient(self.now, path)
        self.clients.append(client)
        return client

    def closed(self, client):
        return self.now - client.opened >= self.expiry

    def exchange(self, client, packet):
        client.packets.append(packet)
        if packet == b"CLAIM":
            return CLOSED if self.closed(client) else self.refusal
        assert packet == b"HELLO" and client.packets == [b"HELLO"]
        self.hellos += 1
        return self.hello if self.hellos <= self.recovery_limit else CLOSED


class DeadlineTests(unittest.TestCase):
    def attempt(self, runtime, failure=None):
        evidence = {}
        if failure:
            with self.assertRaisesRegex(ProbeFailure, failure):
                probe_endpoint(Path(ENDPOINTS[0]), runtime, evidence)
        else:
            probe_endpoint(Path(ENDPOINTS[0]), runtime, evidence)
        self.assertTrue(evidence["cleanup_complete"])
        self.assertTrue(all(client.closed for client in runtime.clients))
        self.assertNotIn("opened", json.dumps(evidence).replace("opened_seconds", ""))
        return evidence

    def test_fixed_deadline_despite_traffic_and_full_capacity_recovery(self):
        runtime = FakeRuntime()
        evidence = self.attempt(runtime)
        self.assertEqual(evidence["result"], "passed")
        self.assertEqual(evidence["recovery_hello_count"], CAPACITY)
        self.assertEqual(len(runtime.clients), 2 * CAPACITY)
        for row in evidence["clients"]:
            self.assertGreaterEqual(len(row["refusal_seconds"]), 5)
            self.assertGreaterEqual(row["refusal_seconds"][-1], 4)
            self.assertLess(row["closed_seconds"], 5.1)
        self.assertGreaterEqual(runtime.health_checks, 7)
        self.assertTrue(all(set(client.packets) == {b"CLAIM"}
                            for client in runtime.clients[:CAPACITY]))
        self.assertTrue(all(client.packets == [b"HELLO"]
                            for client in runtime.clients[CAPACITY:]))

    def test_old_activity_based_timeout_fails_boundedly(self):
        runtime = FakeRuntime(expiry=float("inf"))
        evidence = self.attempt(runtime, "hard_hello_deadline_exceeded")
        self.assertLess(evidence["duration_seconds"], 6.6)
        self.assertEqual(evidence["recovery_hello_count"], 0)

    def test_early_close_cannot_pass_as_deadline_expiry(self):
        self.attempt(FakeRuntime(expiry=2), "closure_outside_deadline_window")

    def test_initial_capacity_unavailable_fails(self):
        self.attempt(FakeRuntime(expiry=0), "initial_connection_refused")

    def test_wrong_refusal_does_not_pass(self):
        for reply in ({"ok": True}, {"ok": False, "code": "lane_busy"}, None, []):
            with self.subTest(reply=reply):
                self.attempt(FakeRuntime(refusal=reply), "reply_not_object|invalid_prehello_refusal")

    def test_wrong_hello_protocol_and_epoch_fail(self):
        for reply in ({**HELLO, "protocol": 0}, {**HELLO, "protocol": True},
                      {**HELLO, "epoch": "bad"}, {**HELLO, "ok": 1}, None):
            with self.subTest(reply=reply):
                self.attempt(FakeRuntime(hello=reply), "reply_not_object|invalid_hello_reply")

    def test_partial_capacity_recovery_fails_and_closes_every_client(self):
        evidence = self.attempt(FakeRuntime(recovery_limit=7), "capacity_recovery_failed")
        self.assertEqual(evidence["recovery_hello_count"], 7)

    def test_changed_epoch_during_recovery_fails(self):
        runtime = FakeRuntime()
        exchange = runtime.exchange

        def changing(client, packet):
            reply = exchange(client, packet)
            return {**reply, "epoch": "b" * 32} if runtime.hellos == 2 else reply

        runtime.exchange = changing
        self.attempt(runtime, "endpoint_epoch_changed")

    def test_unresponsive_compositor_fails_and_cleans_up(self):
        runtime = FakeRuntime()
        runtime.health = Mock(side_effect=[None, ProbeFailure("compositor_unresponsive")])
        self.attempt(runtime, "compositor_unresponsive")

    def test_exchange_timeout_is_not_reported_as_closure(self):
        runtime = FakeRuntime()
        runtime.exchange = Mock(side_effect=ProbeFailure("reply_timeout"))
        evidence = self.attempt(runtime, "reply_timeout")
        self.assertNotIn("closed_seconds", evidence["clients"][0])

    def test_both_endpoints_run_sequentially_with_cleanup(self):
        runtime = FakeRuntime(recovery_limit=16)
        paths = [Path(name) for name in ENDPOINTS]
        with patch("production_handshake_probe.preflight", return_value=paths), \
                patch("production_handshake_probe.NativeRuntime", return_value=runtime):
            result = run(SimpleNamespace(compositor_pid=123))
        self.assertEqual(result["result"], "passed")
        self.assertEqual([row["endpoint"] for row in result["endpoints"]], list(ENDPOINTS))
        self.assertTrue(all(row["cleanup_complete"] for row in result["endpoints"]))
        self.assertFalse(result["post_hello_idle_tested"])

    def test_cleanup_failure_does_not_skip_remaining_sockets_or_report_pass(self):
        runtime = FakeRuntime()
        connect = runtime.connect

        def bad_close(path):
            client = connect(path)
            if len(runtime.clients) == 1:
                client.close = Mock(side_effect=OSError("synthetic close failure"))
            return client

        runtime.connect = bad_close
        evidence = {}
        with self.assertRaisesRegex(ProbeFailure, "socket_cleanup_failed"):
            probe_endpoint(Path(ENDPOINTS[0]), runtime, evidence)
        self.assertEqual(evidence["result"], "failed")
        self.assertFalse(evidence["cleanup_complete"])
        self.assertTrue(all(client.closed for client in runtime.clients[1:]))


class ValidationTests(unittest.TestCase):
    def test_explicit_linux_clean_session_required(self):
        with patch("production_handshake_probe.sys.platform", "linux"):
            with self.assertRaisesRegex(ProbeFailure, "clean_linux_session_required"):
                preflight(SimpleNamespace(clean_session=False))

    def test_private_canonical_directory_and_socket_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory).resolve()
            path.chmod(0o700)
            validate_path(path, directory=True)
            path.chmod(0o755)
            with self.assertRaisesRegex(ProbeFailure, "nonprivate_directory"):
                validate_path(path, directory=True)
            validate_path(path, directory=True, private=False)
            path.chmod(0o775)
            with self.assertRaisesRegex(ProbeFailure, "nonprivate_directory"):
                validate_path(path, directory=True, private=False)
            path.chmod(0o700)
            with self.assertRaisesRegex(ProbeFailure, "invalid_socket_type_or_mode"):
                validate_path(path)
            metadata = SimpleNamespace(st_uid=os.getuid() + 1, st_mode=stat.S_IFSOCK | 0o600)
            with patch.object(Path, "lstat", return_value=metadata):
                with self.assertRaisesRegex(ProbeFailure, "wrong_path_owner"):
                    validate_path(path)

    def test_socket_permissions_and_symlinks_fail_closed(self):
        path = Path("/synthetic/input.sock")
        for mode in (stat.S_IFREG | 0o600, stat.S_IFSOCK | 0o666, stat.S_IFLNK | 0o600):
            with self.subTest(mode=mode), \
                    patch.object(Path, "lstat", return_value=SimpleNamespace(st_uid=os.getuid(), st_mode=mode)), \
                    patch.object(Path, "resolve", return_value=path):
                with self.assertRaisesRegex(ProbeFailure, "invalid_socket_type_or_mode"):
                    validate_path(path)
        with patch.object(Path, "lstat", return_value=SimpleNamespace(st_uid=os.getuid(), st_mode=stat.S_IFSOCK | 0o600)), \
                patch.object(Path, "resolve", return_value=Path("/synthetic/other.sock")):
            with self.assertRaisesRegex(ProbeFailure, "noncanonical_path"):
                validate_path(path)

    def test_exact_peer_pid_and_uid_required_and_failed_connect_is_closed(self):
        for pid, uid in ((456, os.getuid()), (123, os.getuid() + 1)):
            peer = Mock()
            peer.getsockopt.return_value = struct.pack("3i", pid, uid, 1)
            with patch("production_handshake_probe.validate_path"), \
                    patch("production_handshake_probe.socket.socket", return_value=peer), \
                    patch.object(socket, "SO_PEERCRED", 17, create=True):
                with self.assertRaisesRegex(ProbeFailure, "wrong_socket_peer"):
                    NativeRuntime(123).connect(Path("/synthetic/input.sock"))
            peer.close.assert_called_once()

    def test_exchange_eof_reset_timeout_and_invalid_json_are_distinct(self):
        runtime = NativeRuntime(123)
        for response, error in ((b"", None), (ConnectionResetError(), None),
                                (TimeoutError(), "reply_timeout"), (b"garbage", "malformed_reply"),
                                (b"x" * 2049, "oversized_reply")):
            with self.subTest(response=type(response).__name__):
                peer = Mock()
                peer.send.return_value = 5
                if isinstance(response, Exception):
                    peer.recv.side_effect = response
                else:
                    peer.recv.return_value = response
                if error:
                    with self.assertRaisesRegex(ProbeFailure, error):
                        runtime.exchange(peer, b"CLAIM")
                else:
                    self.assertIs(runtime.exchange(peer, b"CLAIM"), CLOSED)
        peer = Mock()
        peer.send.return_value = 5
        peer.recv.return_value = b"null"
        self.assertIsNone(runtime.exchange(peer, b"CLAIM"))
        with self.assertRaisesRegex(ProbeFailure, "reply_not_object"):
            validate_reply(None)


if __name__ == "__main__":
    unittest.main()
