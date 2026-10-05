from __future__ import annotations

import unittest

import synth
from synth import C

PROBE, MODE = "PROBE-HOVER", "hover"
SEEDS = [1, 7, 42, 4242, 999_999]


def run(seed, state, events, bad=0):
    return synth.evaluate(PROBE, MODE, seed, state, events, bad)


def failed(result):
    return sorted(n for n, c in result["checks"].items() if not c["passed"])


class HoverEvaluatorTests(unittest.TestCase):
    def test_genuine_run_passes_for_many_seeds(self):
        for seed in SEEDS:
            state, events = synth.hover_run(seed)
            result = run(seed, state, events)
            self.assertTrue(result["passed"], (seed, result["checks"]))
            self.assertEqual(result["score"], 1.0)

    def test_derivation_shows_four_distinct_names_and_target_is_one_of_them(self):
        seen_targets = set()
        for seed in range(300):
            d = C.derive_hover(seed)
            self.assertEqual(len(set(d["names"])), 4)
            self.assertTrue(set(d["names"]) <= set(C.HOVER_NAMES))
            self.assertIn(d["target"], d["names"])
            seen_targets.add(d["target"])
        self.assertEqual(seen_targets, set(C.HOVER_NAMES))

    def test_wrong_button_fails(self):
        seed = 7
        exp = C.derive_hover(seed)
        wrong = next(n for n in exp["names"] if n != exp["target"])
        state, events = synth.hover_run(seed, click=wrong)
        result = run(seed, state, events)
        self.assertEqual(failed(result), ["clicked_target", "no_wrong_clicks"])
        self.assertFalse(result["passed"])

    def test_click_while_overlay_hidden_fails(self):
        seed = 7
        state, events = synth.hover_run(seed, overlay_visible=False)
        self.assertEqual(failed(run(seed, state, events)), ["overlay_visible_at_click"])

    def test_click_without_any_hover_events_fails(self):
        seed = 7
        state, events = synth.hover_run(seed, with_hover=False)
        self.assertEqual(failed(run(seed, state, events)), ["hover_genuine"])

    def test_state_edited_without_ui_events_fails(self):
        seed = 7
        state, events = synth.hover_run(seed)
        result = run(seed, state, events[:1])
        self.assertFalse(result["passed"])
        for name in ("clicked_target", "overlay_visible_at_click", "hover_genuine", "single_click"):
            self.assertIn(name, failed(result))

    def test_state_that_disagrees_with_events_fails(self):
        seed = 7
        exp = C.derive_hover(seed)
        wrong = next(n for n in exp["names"] if n != exp["target"])
        state, events = synth.hover_run(seed, click=wrong)
        state["last_clicked"] = exp["target"]
        state["clicks"][0]["name"] = exp["target"]
        result = run(seed, state, events)
        self.assertFalse(result["passed"])
        self.assertIn("clicked_target", failed(result))

    def test_two_clicks_fail_single_click(self):
        seed = 7
        state, events = synth.hover_run(seed)
        click = next(e for e in events if e["type"] == "overlay_click")
        extra = {
            "seq": events[-1]["seq"] + 1,
            "t": events[-1]["t"] + 5,
            "type": "overlay_click",
            "details": dict(click["details"]),
        }
        events.append(extra)
        state["clicks"].append(dict(state["clicks"][0]))
        state["seq"] = extra["seq"]
        self.assertEqual(failed(run(seed, state, events)), ["single_click"])

    def test_overlay_names_must_match_seed(self):
        seed = 7
        state, events = synth.hover_run(seed)
        state["overlay_names"] = list(reversed(state["overlay_names"]))
        self.assertIn("overlay_names_consistent", failed(run(seed, state, events)))

    def test_hover_counts_must_match_events(self):
        seed = 7
        state, events = synth.hover_run(seed)
        state["hover_enter_count"] = 3
        self.assertIn("hover_genuine", failed(run(seed, state, events)))

    def test_no_click_at_all_fails(self):
        seed = 7
        state, events = synth.hover_run(seed)
        events = [e for e in events if e["type"] != "overlay_click"]
        for i, e in enumerate(events, 1):
            e["seq"] = i
        state.update(
            {
                "clicks": [],
                "last_clicked": None,
                "last_click_overlay_visible": None,
                "seq": len(events),
            }
        )
        self.assertFalse(run(seed, state, events)["passed"])


if __name__ == "__main__":
    unittest.main()
