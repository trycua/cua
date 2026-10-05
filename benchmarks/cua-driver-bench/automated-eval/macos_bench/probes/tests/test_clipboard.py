from __future__ import annotations

import unittest

import synth
from synth import C

PROBE, MODE = "PROBE-CLIPBOARD", "clipboard"
SEEDS = [1, 7, 42, 4242, 999_999]


def run(seed, state, events, bad=0):
    return synth.evaluate(PROBE, MODE, seed, state, events, bad)


def failed(result):
    return sorted(n for n, c in result["checks"].items() if not c["passed"])


class ClipboardEvaluatorTests(unittest.TestCase):
    def test_genuine_run_passes_for_many_seeds(self):
        for seed in SEEDS:
            state, events = synth.clipboard_run(seed)
            result = run(seed, state, events)
            self.assertTrue(result["passed"], (seed, result["checks"]))
            self.assertEqual(result["score"], 1.0)

    def test_expected_value_is_a_times_b_plus_c(self):
        for seed in range(300):
            d = C.derive_clipboard(seed)
            self.assertEqual(d["result"], d["a"] * d["b"] + d["c"])
            self.assertTrue(120 <= d["a"] <= 989 and 11 <= d["b"] <= 97 and 1000 <= d["c"] <= 99999)

    def test_thousands_separators_and_whitespace_are_accepted(self):
        seed = 7
        value = C.derive_clipboard(seed)["result"]
        for typed in (
            f"{value:,}",
            f" {value} ",
            f"{value:,}\n",
            f"{value:,}".replace(",", "\u202f,"),
            f"\u00a0{value}",
        ):
            state, events = synth.clipboard_run(seed, typed=typed)
            self.assertTrue(run(seed, state, events)["passed"], repr(typed))

    def test_wrong_values_fail(self):
        seed = 7
        value = C.derive_clipboard(seed)["result"]
        for typed in (
            str(value + 1),
            str(value - 1),
            "abc",
            "1,23,456",
            f"{value}.5",
            f"{value} x",
            "",
            f"={value}",
        ):
            state, events = synth.clipboard_run(seed, typed=typed)
            result = run(seed, state, events)
            self.assertFalse(result["passed"], repr(typed))
            self.assertIn("result_correct", failed(result))

    def test_not_saved_fails(self):
        seed = 7
        state, events = synth.clipboard_run(seed)
        events = events[:-1]
        state.update({"saved": False, "save_count": 0, "saved_value": None, "seq": len(events)})
        result = run(seed, state, events)
        self.assertEqual(failed(result), ["result_correct", "result_saved", "ui_events"])

    def test_state_edited_without_ui_events_fails(self):
        seed = 7
        state, events = synth.clipboard_run(seed)
        result = run(seed, state, events[:1])
        self.assertFalse(result["passed"])
        self.assertIn("result_saved", failed(result))
        self.assertIn("ui_events", failed(result))

    def test_saved_value_differing_from_typed_events_fails(self):
        seed = 7
        value = C.derive_clipboard(seed)["result"]
        state, events = synth.clipboard_run(seed, typed="1")
        state["saved_value"] = str(value)
        events[-1]["details"]["value"] = str(value)
        result = run(seed, state, events)
        self.assertEqual(failed(result), ["ui_events"])

    def test_wrong_seed_fails_integrity(self):
        state, events = synth.clipboard_run(7)
        self.assertFalse(run(8, state, events)["checks"]["integrity"]["passed"])

    def test_garbage_state_fails_closed(self):
        seed = 7
        _, events = synth.clipboard_run(seed)
        result = run(
            seed,
            {
                "mode": "clipboard",
                "seed": seed,
                "seq": len(events),
                "saved": True,
                "saved_value": 123,
            },
            events,
        )
        self.assertFalse(result["passed"])


if __name__ == "__main__":
    unittest.main()
