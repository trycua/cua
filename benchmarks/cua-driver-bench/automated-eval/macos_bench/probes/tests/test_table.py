from __future__ import annotations

import unittest

import synth
from synth import C

PROBE, MODE = "PROBE-TABLE", "table"
SEEDS = [1, 7, 42, 4242, 999_999]


def run(seed, state, events, bad=0):
    return synth.evaluate(PROBE, MODE, seed, state, events, bad)


def failed(result):
    return sorted(n for n, c in result["checks"].items() if not c["passed"])


class TableEvaluatorTests(unittest.TestCase):
    def test_genuine_run_passes_for_many_seeds(self):
        for seed in SEEDS:
            state, events = synth.table_run(seed)
            result = run(seed, state, events)
            self.assertTrue(result["passed"], (seed, result["checks"]))
            self.assertEqual(result["score"], 1.0)

    def test_targets_are_three_distinct_codes_beyond_the_first_screen(self):
        for seed in range(200):
            codes = C.derive_table(seed)["codes"]
            self.assertEqual(len(set(codes)), 3)
            self.assertTrue(all(30 <= int(c[2:]) <= 400 for c in codes))

    def test_missing_code_fails_required_only(self):
        seed = 7
        codes = C.derive_table(seed)["codes"]
        state, events = synth.table_run(seed, flag=codes[:2])
        self.assertEqual(
            failed(run(seed, state, events)), ["final_state_matches", "required_flagged"]
        )

    def test_extra_code_fails_no_extra(self):
        seed = 7
        codes = C.derive_table(seed)["codes"]
        extra = "K-0001" if "K-0001" not in codes else "K-0002"
        state, events = synth.table_run(seed, flag=codes + [extra])
        self.assertEqual(
            failed(run(seed, state, events)), ["final_state_matches", "no_extra_flagged"]
        )

    def test_not_saved_fails(self):
        seed = 7
        state, events = synth.table_run(seed, save=False)
        result = run(seed, state, events)
        self.assertIn("save_pressed", failed(result))
        self.assertFalse(result["passed"])

    def test_flag_after_save_without_resave_fails_final_state(self):
        seed = 7
        codes = C.derive_table(seed)["codes"]
        state, events = synth.table_run(seed)
        seq = events[-1]["seq"] + 1
        events.append(
            {
                "seq": seq,
                "t": events[-1]["t"] + 5,
                "type": "flag_toggle",
                "details": {"code": "K-0003", "row": 2, "flagged": True},
            }
        )
        state["flagged"] = sorted(codes + ["K-0003"])
        state["seq"] = seq
        self.assertEqual(failed(run(seed, state, events)), ["final_state_matches"])

    def test_toggle_on_then_off_is_fine(self):
        seed = 7
        codes = C.derive_table(seed)["codes"]
        state, events = synth.table_run(seed, flag=["K-0005"] + codes)
        # un-flag K-0005 before saving
        save = events.pop()
        events.append(
            {
                "seq": save["seq"],
                "t": save["t"],
                "type": "flag_toggle",
                "details": {"code": "K-0005", "row": 4, "flagged": False},
            }
        )
        events.append(
            {
                "seq": save["seq"] + 1,
                "t": save["t"] + 5,
                "type": "save",
                "details": {"flagged": sorted(codes), "count": 1},
            }
        )
        state.update(
            {
                "flagged": sorted(codes),
                "flagged_count": 3,
                "saved_flagged": sorted(codes),
                "seq": save["seq"] + 1,
            }
        )
        self.assertTrue(run(seed, state, events)["passed"])

    def test_state_edited_without_ui_events_fails(self):
        seed = 7
        state, events = synth.table_run(seed)
        result = run(seed, state, events[:1])
        self.assertFalse(result["passed"])
        self.assertIn("ui_events", failed(result))
        self.assertIn("save_pressed", failed(result))

    def test_forged_saved_snapshot_fails_ui_events(self):
        seed = 7
        codes = C.derive_table(seed)["codes"]
        state, events = synth.table_run(seed, flag=codes[:2])
        state["flagged"] = sorted(codes)
        state["saved_flagged"] = sorted(codes)
        result = run(seed, state, events)
        self.assertFalse(result["passed"])
        self.assertIn("ui_events", failed(result))

    def test_malformed_toggle_event_fails_closed(self):
        seed = 7
        state, events = synth.table_run(seed)
        events[2]["details"]["code"] = "not-a-code"
        self.assertIn("ui_events", failed(run(seed, state, events)))

    def test_wrong_seed_fails_integrity(self):
        state, events = synth.table_run(7)
        self.assertFalse(run(8, state, events)["checks"]["integrity"]["passed"])

    def test_garbage_state_fails_closed(self):
        seed = 7
        _, events = synth.table_run(seed)
        result = run(
            seed,
            {
                "mode": "table",
                "seed": seed,
                "seq": len(events),
                "saved_flagged": "K-0001",
                "flagged": 3,
                "saved": True,
            },
            events,
        )
        self.assertFalse(result["passed"])


if __name__ == "__main__":
    unittest.main()
