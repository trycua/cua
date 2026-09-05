r"""Transport unit checks and an opt-in disposable-guest input refusal suite.

Unit checks: python3 -m unittest input_transport_test
Live suite: python3 input_transport_test.py --socket PATH --pid PID \
    --address HEX --click-x X --click-y Y

The live suite prints a JSON grant request, keeps that client open, and reads
one already-signed grant JSON line from stdin. Sign its epoch/challenge/target
on the host with input_operator.py (click capability=1), then transfer only
the JSON grant through the trusted operator channel. No key or signing code
is loaded in the guest. The supplied coordinates must belong to a disposable
fixture: exactly one accepted synthetic click is attempted. Application
effects and primary-seat isolation require independent GUI fixture evidence.
"""

import argparse
import io
import json
import math
import re
import socket
import stat
import sys
import time
import unittest
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch

MAX_PACKET = 2048


def exchange(client, packet):
    encoded = packet.encode("ascii")
    if not encoded or len(encoded) > MAX_PACKET:
        raise ValueError("request packet is empty or too large")
    if client.send(encoded) != len(encoded):
        raise RuntimeError("partial packet send")
    raw, _ancillary, flags, _address = client.recvmsg(MAX_PACKET + 1)
    if flags & socket.MSG_TRUNC or not raw or len(raw) > MAX_PACKET:
        raise RuntimeError("response is empty or too large")
    value = json.loads(raw)
    if not isinstance(value, dict) or type(value.get("ok")) is not bool:
        raise ValueError("invalid response envelope")
    return value


def connect(path, *, claim=False):
    metadata = Path(path).stat()
    if not stat.S_ISSOCK(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) != 0o600:
        raise ValueError("expected a mode-0600 experiment socket")
    client = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    try:
        client.settimeout(5)
        client.connect(str(path))
        hello = exchange(client, "HELLO")
        if hello.get("ok") is not True or hello.get("protocol") != 0:
            raise ValueError("experiment HELLO failed")
        for name in ("epoch", "challenge"):
            if re.fullmatch(r"[0-9a-f]{32}", hello.get(name, "")) is None:
                raise ValueError("invalid HELLO token")
        if claim:
            claimed = exchange(client, "CLAIM")
            if claimed.get("ok") is not True or type(claimed.get("lane")) is not int or claimed["lane"] not in (0, 1):
                raise ValueError("input lane claim failed")
        return client, hello
    except BaseException:
        client.close()
        raise


def refused(response, *codes):
    if response.get("ok") is not False or response.get("code") not in codes or not isinstance(response.get("detail"), str):
        raise AssertionError(f"expected refusal {codes}, received {response}")


def accepted(response):
    if response != {"ok": True, "effect": "unverifiable", "route": "synthetic_events"}:
        raise AssertionError(f"expected synthetic dispatch acknowledgement, received {response}")


def approved_packet(grant, hello, target):
    """Reject misplaced fixture grants; signature verification stays server-side."""
    packet = grant.get("packet") if isinstance(grant, dict) else None
    if not isinstance(packet, str) or re.fullmatch(
        r"APPROVE [0-9a-f]{32} [0-9a-f]{32} [1-9][0-9]{0,19} [1-9][0-9]? [0-9a-f]{128}", packet
    ) is None:
        raise ValueError("expected a signed APPROVE packet")
    _, challenge, token, expiry, caps, _signature = packet.split(" ")
    if challenge != hello["challenge"] or token != target["target"]:
        raise ValueError("grant does not match pending connection and target")
    if grant.get("epoch") != hello["epoch"]:
        raise ValueError("grant does not match compositor epoch")
    if int(expiry) > (1 << 64) - 1 or not 1 <= int(caps) <= 15 or not int(caps) & 1:
        raise ValueError("grant must include click capability and bounded fields")
    return packet


def run_live(path, pid, address, x, y, grant_input=sys.stdin, output=sys.stdout):
    if pid <= 0 or pid > (1 << 31) - 1 or re.fullmatch(r"[0-9a-f]{1,16}", address) is None:
        raise ValueError("expected a positive PID and a lowercase hexadecimal address without 0x")
    if not math.isfinite(x) or not math.isfinite(y) or x < 0 or y < 0:
        raise ValueError("click coordinates must be finite and nonnegative")
    client, hello = connect(path, claim=True)
    with client:
        target = exchange(client, f"TARGET {pid} {address}")
        if target.get("ok") is not True or re.fullmatch(r"[0-9a-f]{32}", target.get("target", "")) is None:
            raise AssertionError(f"TARGET failed: {target}")
        revision = target["revision"]
        if type(revision) is not int or revision < 1 or not (x < target["width"] and y < target["height"]):
            raise ValueError("invalid target geometry or fixture coordinates")
        token = target["target"]
        refused(exchange(client, f"CLICK 1 {token} {revision} {x} {y} 272 1"), "pending_operator_approval")
        click = f"CLICK 7 {token} {revision} {x} {y} 272 1"
        # An operator connection has its own HELLO and cannot mint authority.
        operator_client, operator_hello = connect(path)
        with operator_client:
            if operator_hello["challenge"] == hello["challenge"] or operator_hello["epoch"] != hello["epoch"]:
                raise AssertionError("operator HELLO did not isolate the connection")
            refused(exchange(operator_client, "APPROVE malformed"), "invalid_request", "invalid_grant")
            expiry = time.time_ns() // 1_000_000 + 30_000
            forged = f"APPROVE {hello['challenge']} {token} {expiry} 1 {'0' * 128}"
            refused(exchange(operator_client, forged), "invalid_grant")
            print(json.dumps({"event": "grant_required", "epoch": hello["epoch"],
                              "challenge": hello["challenge"], "target": token,
                              "revision": revision, "width": target["width"], "height": target["height"],
                              "capabilities": 1, "max_lifetime_ms": 60_000}), file=output, flush=True)
            line = grant_input.readline(MAX_PACKET + 1)
            if not line or len(line) > MAX_PACKET:
                raise ValueError("signed grant JSON missing or too large")
            packet = approved_packet(json.loads(line), hello, target)
            approval = exchange(operator_client, packet)
            if approval.get("ok") is not True:
                raise AssertionError(f"operator approval failed: {approval}")
            try:
                refused(exchange(operator_client, packet), "invalid_grant", "lease_busy", "replay")
                for bad in (
                    f"CLICK 2 {token} {revision} nan {y} 272 1",
                    f"CLICK 3 {token} {revision} {x} inf 272 1",
                    f"CLICK 4 {token} {revision} -1 {y} 272 1",
                    f"CLICK 5 {token} {revision} {target['width']} {y} 272 1",
                    f"CLICK 6 {token} {revision} {x} {target['height']} 272 1",
                ):
                    refused(exchange(client, bad), "invalid_request")
                accepted(exchange(client, click))
                refused(exchange(client, click), "replay")
            finally:
                stopped = exchange(operator_client, "STOP")
                if stopped.get("ok") is not True:
                    raise AssertionError(f"STOP failed: {stopped}")
            refused(exchange(operator_client, packet), "invalid_grant")
            refused(exchange(client, f"CLICK 8 {token} {revision} {x} {y} 272 1"), "pending_operator_approval")
    print(json.dumps({"result": "passed", "missing_grant": "refused", "malformed_grant": "refused",
                      "forged_grant": "refused", "grant_replay": "refused", "grant_replay_after_stop": "refused", "sequence_replay": "refused",
                      "invalid_coordinates": "refused", "stop": "revoked", "accepted_clicks": 1,
                      "effect": "unverifiable", "route": "synthetic_events"}), file=output, flush=True)


class TransportTest(unittest.TestCase):
    def run_scripted_live(self, fail_click=False):
        hello = {"epoch": "a" * 32, "challenge": "b" * 32}
        target = {"ok": True, "target": "c" * 32, "revision": 1, "width": 100, "height": 100}
        grant = {"epoch": hello["epoch"],
                 "packet": f"APPROVE {hello['challenge']} {target['target']} 1700000000000 1 {'d' * 128}"}
        refusal = lambda code: {"ok": False, "code": code, "detail": "synthetic refusal"}
        responses = [target, refusal("pending_operator_approval"), refusal("invalid_request"),
                     refusal("invalid_grant"), {"ok": True}, refusal("lease_busy")]
        responses += [refusal("invalid_request")] * 5
        if fail_click:
            responses += [refusal("primary_target_busy"), {"ok": True}]
        else:
            responses += [{"ok": True, "effect": "unverifiable", "route": "synthetic_events"},
                          refusal("replay"), {"ok": True}, refusal("invalid_grant"), refusal("pending_operator_approval")]
        connections = [(MagicMock(), hello), (MagicMock(), {**hello, "challenge": "e" * 32})]
        output = io.StringIO()
        with patch(__name__ + ".connect", side_effect=connections), patch(__name__ + ".exchange", side_effect=responses) as send:
            if fail_click:
                with self.assertRaises(AssertionError):
                    run_live(Path("synthetic.sock"), 123, "abc", 5, 5, io.StringIO(json.dumps(grant) + "\n"), output)
                self.assertEqual(send.call_args.args[1], "STOP")
                self.assertNotIn('"result": "passed"', output.getvalue())
            else:
                run_live(Path("synthetic.sock"), 123, "abc", 5, 5, io.StringIO(json.dumps(grant) + "\n"), output)
                records = [json.loads(line) for line in output.getvalue().splitlines()]
                self.assertEqual(records[0]["event"], "grant_required")
                self.assertEqual(records[1]["accepted_clicks"], 1)
                self.assertEqual(send.call_count, len(responses))

    def test_live_sequence_with_already_signed_fixture(self):
        self.run_scripted_live()

    def test_live_failure_still_revokes_grant(self):
        self.run_scripted_live(fail_click=True)

    def test_packet_framing(self):
        client = Mock()
        client.send.return_value = 5
        client.recvmsg.return_value = (b'{"ok":true}', [], 0, None)
        self.assertEqual(exchange(client, "HELLO"), {"ok": True})
        client.send.assert_called_once_with(b"HELLO")

    def test_truncated_or_invalid_responses(self):
        for raw, flags in ((b"", 0), (b"x" * (MAX_PACKET + 1), 0),
                           (b'{"ok":true}', socket.MSG_TRUNC), (b"[]", 0),
                           (b'{"ok":1}', 0), (b"not-json", 0)):
            client = Mock()
            client.send.return_value = 5
            client.recvmsg.return_value = (raw, [], flags, None)
            with self.subTest(raw=raw[:30], flags=flags), self.assertRaises((ValueError, RuntimeError)):
                exchange(client, "HELLO")

    def test_partial_send_and_packet_limits(self):
        client = Mock()
        client.send.return_value = 1
        with self.assertRaises(RuntimeError):
            exchange(client, "HELLO")
        for packet in ("", "x" * (MAX_PACKET + 1), "é"):
            with self.assertRaises(ValueError):
                exchange(client, packet)

    def test_refusals_require_structured_expected_code(self):
        refused({"ok": False, "code": "replay", "detail": "old sequence"}, "replay")
        for response in ({"ok": True}, {"ok": False, "code": "replay"},
                         {"ok": False, "code": "invalid_request", "detail": "bad"}):
            with self.assertRaises(AssertionError):
                refused(response, "replay")

    def test_signed_fixture_matches_pending_connection(self):
        hello = {"epoch": "a" * 32, "challenge": "b" * 32}
        target = {"target": "c" * 32}
        packet = f"APPROVE {hello['challenge']} {target['target']} 1700000000000 1 {'d' * 128}"
        grant = {"epoch": hello["epoch"], "packet": packet}
        self.assertEqual(approved_packet(grant, hello, target), packet)
        for invalid in ({**grant, "epoch": "e" * 32}, {**grant, "packet": packet + " extra"},
                        {**grant, "packet": packet.replace("b" * 32, "e" * 32)},
                        {**grant, "packet": packet.replace("c" * 32, "e" * 32)},
                        {**grant, "packet": packet.replace(" 1 ", " 2 ")},
                        {**grant, "packet": packet.replace("1700000000000", str(1 << 64))}):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                approved_packet(invalid, hello, target)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--socket", type=Path, required=True)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--address", required=True)
    parser.add_argument("--click-x", type=float, required=True)
    parser.add_argument("--click-y", type=float, required=True)
    parser.add_argument("--grant-file", type=Path,
                        help="Read a public operator grant from a new file instead of stdin")
    args = parser.parse_args()
    class GrantFile:
        def readline(self, maximum):
            deadline = time.monotonic() + 45
            while not args.grant_file.exists():
                if time.monotonic() >= deadline:
                    raise TimeoutError("external operator grant did not arrive")
                time.sleep(0.1)
            with args.grant_file.open() as stream:
                return stream.readline(maximum)
    if args.grant_file and args.grant_file.exists():
        raise ValueError("grant file must not exist before the pending challenge")
    run_live(args.socket, args.pid, args.address, args.click_x, args.click_y,
             GrantFile() if args.grant_file else sys.stdin)


if __name__ == "__main__":
    main()
