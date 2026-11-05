"""Checker tests for IR-06 on an untouched, a solved and near-miss states."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))
sys.path.insert(0, str(BUNDLE / "evaluator"))

from v2kit import common  # noqa: E402
import evaluate as ev  # noqa: E402

PROFILE = {"phone": "07700 900561", "emergency_name": "Wilma Osei-Brandt", "emergency_relation": "Partner",
           "emergency_phone": "07700 900287"}
PREFS = {"pace": "steady", "groups": ["sunday-hills", "thursday-evening"], "newsletter": "monthly"}
SAVES = [{"type": "page_loaded", "page": "profile"}, {"type": "profile_saved"},
         {"type": "page_loaded", "page": "preferences"}, {"type": "preferences_saved"}]


class IR06(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        self.initial = common.read_json(BUNDLE / "app/initial-state.json")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, profile: dict, prefs: dict, events: list) -> None:
        state = copy.deepcopy(self.initial)
        state["profile"].update(profile)
        state["preferences"].update(prefs)
        common.write_json(self.ws / "state/state.json", state)
        lines = [json.dumps({**e, "seq": i + 1}) for i, e in enumerate(events)]
        (self.ws / "state/events.jsonl").write_text("\n".join(lines) + ("\n" if lines else ""))

    def run_eval(self) -> dict:
        return common.evaluate_dir("IR-06", ev.evaluate, self.ws)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_untouched_after_load_fails_with_zero(self) -> None:
        self.write({}, {}, [{"type": "server_started"}, {"type": "page_loaded", "page": "profile"}])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_by_saving_first_passes(self) -> None:
        self.write(PROFILE, PREFS, SAVES)
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)
        self.assertFalse(r["diagnostics"]["interruption"]["shown"])
        self.assertIsNone(r["diagnostics"]["interruption"]["handled"])

    def test_solved_after_cancelling_dialog_passes(self) -> None:
        self.write(PROFILE, PREFS, [{"type": "page_loaded", "page": "profile"}, {"type": "leave_guard", "page": "profile"}] + SAVES[1:])
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["diagnostics"]["interruption"],
                         {"kind": "leave_site_dialog", "shown": True, "exercised": True, "handled": True, "completed": True})

    def test_leave_guard_activation_reported(self) -> None:
        guard = {"type": "leave_guard", "page": "profile", "activated": False}
        self.write(PROFILE, PREFS, [{"type": "page_loaded", "page": "profile"}, guard] + SAVES[1:])
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["diagnostics"]["leave_guard_activated"], [False])

    def test_left_unsaved_then_redone_fails(self) -> None:
        events = [{"type": "page_loaded", "page": "profile"}, {"type": "leave_guard", "page": "profile"},
                  {"type": "left_unsaved", "page": "profile", "changed": ["phone"]}] + SAVES[2:] + SAVES[:2]
        self.write(PROFILE, PREFS, events)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["no_edits_lost"]["passed"])
        self.assertFalse(r["diagnostics"]["interruption"]["handled"])
        self.assertTrue(r["diagnostics"]["interruption"]["completed"])

    def test_profile_lost_fails(self) -> None:
        events = [{"type": "page_loaded", "page": "profile"}, {"type": "leave_guard", "page": "profile"},
                  {"type": "left_unsaved", "page": "profile", "changed": ["phone"]}] + SAVES[2:]
        self.write({}, PREFS, events)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["phone_ok"]["passed"])

    def test_thursday_group_dropped_fails(self) -> None:
        self.write(PROFILE, {**PREFS, "groups": ["sunday-hills"]}, SAVES)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["groups_ok"]["passed"])

    def test_wrong_relation_fails(self) -> None:
        self.write({**PROFILE, "emergency_relation": "Brother"}, PREFS, SAVES)
        self.assertFalse(self.run_eval()["passed"])

    def test_leaderboard_switched_on_fails(self) -> None:
        self.write(PROFILE, {**PREFS, "leaderboard": True}, SAVES)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.5)

    def test_display_name_changed_fails(self) -> None:
        self.write({**PROFILE, "display_name": "Wilma Osei-Brandt"}, PREFS, SAVES)
        self.assertFalse(self.run_eval()["passed"])


if __name__ == "__main__":
    unittest.main()
