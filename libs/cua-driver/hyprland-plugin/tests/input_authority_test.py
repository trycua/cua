"""Runner checks only; these mocks do not compile or validate the compositor."""

import unittest

from input_authority_live import exercise, lane_state, validate_targets


class ScriptedPeer:
    def __init__(self, fault=None):
        self.fault = fault
        self.tokens = [None, None]
        self.leases = [False, False]
        self.deadlines = [0, 0]
        self.serial = 0
        self.revoked = False
        self.calls = []
        self.hellos = [{"epoch": "a" * 32, "challenge": char * 32} for char in ("b", "c")]

    def status(self):
        return {"experiment": {"protocol": 0, "test_only": True, "transport_ready": True,
                "lanes": [{"lane": i, "lease_active": self.leases[i], "dispatches": 0,
                           "held_button": 0, "held_keys": 0, "drag_active": False,
                           "pointer_focus": False, "keyboard_focus": False} for i in range(2)]}}

    def grants(self, stage, requests):
        grants = {}
        for row in requests:
            expiry = 1700000001000 if row["name"] == "active-0" else 1700000002000
            if stage == "after" and row["lane"] == 0:
                expiry = row["expires_before_unix_ms"] - 100
            grants[row["name"]] = {"epoch": row["epoch"],
                "packet": f"APPROVE {row['challenge']} {row['target']} {expiry} 1 {'d' * 128}"}
        if stage == "before" and self.fault == "invalid_renewal":
            grants["renewal-0"] = dict(grants["active-0"])
        return grants

    def send(self, connection, packet):
        lane = int(connection[-1])
        self.calls.append((connection, packet))
        fields = packet.split()
        if fields[0] == "TARGET":
            if self.tokens[lane] is None:
                self.serial += 1
                self.tokens[lane] = f"{self.serial:032x}"
            return {"ok": True, "target": self.tokens[lane], "revision": 1}
        if fields[0] in ("STOP", "CANCEL"):
            self.revoked = True
            affected = [0, 1] if fields[0] == "STOP" or self.fault == "cancel_sibling" else [lane]
            for index in affected:
                self.leases[index] = False
                if self.fault != "retain_highwater":
                    self.deadlines[index] = 0
                if self.fault != "retain_tokens":
                    self.tokens[index] = None
            return {"ok": True}
        if fields[0] == "APPROVE":
            stale = fields[2] != self.tokens[lane]
            if stale and not (self.revoked and ((self.fault == "accept_renewal" and lane == 0
                                                 and fields[3] == "1700000002000")
                                                or (self.fault == "accept_pending" and lane == 1))):
                return {"ok": False, "code": "stale_target", "detail": "stale_target"}
            if self.fault == "refuse_fresh" and self.revoked:
                return {"ok": False, "code": "invalid_grant", "detail": "invalid_grant"}
            if int(fields[3]) <= self.deadlines[lane]:
                return {"ok": False, "code": "invalid_grant", "detail": "invalid_grant"}
            self.deadlines[lane] = int(fields[3])
            self.leases[lane] = True
            return {"ok": True}
        raise AssertionError("authority probe must not send input")


class AuthorityProbeTest(unittest.TestCase):
    def probe(self, command="STOP", fault=None):
        peer = ScriptedPeer(fault)
        result = exercise(command, ["client0", "client1"], peer.hellos, ["operator0", "operator1"],
                          [{"pid": 10, "address": "abc"}, {"pid": 11, "address": "def"}],
                          peer.grants, peer.status, peer.send)
        return result, peer

    def test_stop_checks_unused_renewal_and_pending_other_lane(self):
        result, peer = self.probe()
        self.assertEqual(result["result"], "passed")
        self.assertEqual(result["pending_sibling"], "revoked")
        self.assertTrue(result["fresh_grants_accepted"])
        self.assertTrue(result["shorter_fresh_grant_accepted"])
        self.assertFalse(result["input_dispatched"])
        self.assertTrue(all(packet.split()[0] in ("TARGET", "APPROVE", "STOP") for _, packet in peer.calls))

    def test_cancel_preserves_the_pending_sibling_grant(self):
        result, _ = self.probe("CANCEL")
        self.assertEqual(result["pending_sibling"], "preserved")

    def test_regressions_cannot_report_pass(self):
        for fault in ("accept_renewal", "accept_pending", "retain_tokens", "refuse_fresh", "invalid_renewal", "retain_highwater"):
            with self.subTest(fault=fault), self.assertRaises(AssertionError):
                self.probe(fault=fault)
        with self.assertRaises(AssertionError):
            self.probe("CANCEL", "cancel_sibling")
        with self.assertRaises(AssertionError):
            self.probe("CANCEL", "retain_highwater")

    def test_status_requires_explicit_no_input_evidence(self):
        status = ScriptedPeer().status()
        for field, value in (("held_button", 272), ("held_keys", 1), ("drag_active", True),
                             ("pointer_focus", True), ("keyboard_focus", True), ("dispatches", "0")):
            with self.subTest(field=field):
                invalid = ScriptedPeer().status()
                invalid["experiment"]["lanes"][0][field] = value
                with self.assertRaises(AssertionError):
                    lane_state(invalid)
        del status["experiment"]["lanes"][0]["lease_active"]
        with self.assertRaises(AssertionError):
            lane_state(status)

    def test_plan_refuses_unbounded_or_shared_targets(self):
        valid = [{"pid": 10, "address": "abc"}, {"pid": 11, "address": "def"}]
        validate_targets(valid)
        for invalid in ([], [valid[0], valid[0]], [valid[0], {"pid": True, "address": "def"}],
                        [valid[0], {"pid": 11, "address": "0xdef"}],
                        [valid[0], {"pid": 11, "address": "0"}]):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                validate_targets(invalid)


if __name__ == "__main__":
    unittest.main()
