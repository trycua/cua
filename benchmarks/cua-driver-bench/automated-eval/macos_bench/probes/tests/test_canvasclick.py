from __future__ import annotations

import unittest

import synth
from synth import C

PROBE, MODE = "PROBE-CANVASCLICK", "canvasclick"
SEEDS = [1, 7, 42, 4242, 999_999]


def run(seed, state, events, bad=0):
    return synth.evaluate(PROBE, MODE, seed, state, events, bad)


def failed(result):
    return sorted(n for n, c in result["checks"].items() if not c["passed"])


def full(seed):
    exp = C.derive_canvasclick(seed)
    return (
        [("left", n) for n in exp["left_sequence"]],
        [("right", n) for n in exp["right_targets"]],
        exp,
    )


class CanvasClickEvaluatorTests(unittest.TestCase):
    def test_genuine_run_passes_for_many_seeds(self):
        for seed in SEEDS:
            state, events = synth.canvasclick_run(seed)
            result = run(seed, state, events)
            self.assertTrue(result["passed"], (seed, result["checks"]))
            self.assertEqual(result["score"], 1.0)
            self.assertAlmostEqual(sum(c["weight"] for c in result["checks"].values()), 1.0)

    def test_layout_invariants(self):
        for seed in range(300):
            d = C.derive_canvasclick(seed)
            circles = d["circles"]
            self.assertEqual(sorted(c["label"] for c in circles), list(range(1, 25)))
            for c in circles:
                self.assertTrue(14 <= c["cx"] <= 700 - 14 and 14 <= c["cy"] <= 420 - 14)
            for i, a in enumerate(circles):
                for b in circles[i + 1 :]:
                    self.assertGreater(
                        ((a["cx"] - b["cx"]) ** 2 + (a["cy"] - b["cy"]) ** 2) ** 0.5, 2 * 14 + 4
                    )
            picked = d["left_sequence"] + d["right_targets"]
            self.assertEqual(len(picked), 16)
            self.assertEqual(len(set(picked)), 16)
            self.assertTrue(set(picked) <= set(range(1, 25)))

    def test_misses_outside_circles_are_allowed(self):
        seed = 7
        left, right, _ = full(seed)
        actions = (
            [("miss", "left")]
            + left[:5]
            + [("miss", "left")]
            + left[5:]
            + [("miss", "right")]
            + right
            + [("done",)]
        )
        state, events = synth.canvasclick_run(seed, actions)
        result = run(seed, state, events)
        self.assertTrue(result["passed"], result["checks"])
        self.assertIn("2 left click(s) missed", result["checks"]["no_extra_left_clicks"]["detail"])

    def test_wrong_order_fails_both_left_checks(self):
        seed = 7
        left, right, _ = full(seed)
        left[2], left[3] = left[3], left[2]
        state, events = synth.canvasclick_run(seed, left + right + [("done",)])
        self.assertEqual(
            failed(run(seed, state, events)), ["left_sequence_in_order", "no_extra_left_clicks"]
        )

    def test_extra_left_click_on_another_circle_fails_no_extra_only(self):
        seed = 7
        left, right, exp = full(seed)
        other = next(
            n
            for n in range(1, 25)
            if n not in exp["left_sequence"] and n not in exp["right_targets"]
        )
        actions = left[:6] + [("left", other)] + left[6:] + right + [("done",)]
        state, events = synth.canvasclick_run(seed, actions)
        self.assertEqual(failed(run(seed, state, events)), ["no_extra_left_clicks"])

    def test_repeated_click_on_a_required_circle_fails_no_extra(self):
        seed = 7
        left, right, _ = full(seed)
        actions = left[:3] + [left[2]] + left[3:] + right + [("done",)]
        state, events = synth.canvasclick_run(seed, actions)
        self.assertEqual(failed(run(seed, state, events)), ["no_extra_left_clicks"])

    def test_missing_left_click_fails(self):
        seed = 7
        left, right, _ = full(seed)
        state, events = synth.canvasclick_run(seed, left[:-1] + right + [("done",)])
        self.assertEqual(
            failed(run(seed, state, events)), ["left_sequence_in_order", "no_extra_left_clicks"]
        )

    def test_missing_right_target_fails_only_right_targets(self):
        seed = 7
        left, right, _ = full(seed)
        state, events = synth.canvasclick_run(seed, left + right[:-1] + [("done",)])
        self.assertEqual(failed(run(seed, state, events)), ["right_targets"])

    def test_right_click_on_a_non_target_fails_only_that_check(self):
        seed = 7
        left, right, exp = full(seed)
        other = next(
            n
            for n in range(1, 25)
            if n not in exp["left_sequence"] and n not in exp["right_targets"]
        )
        state, events = synth.canvasclick_run(seed, left + right + [("right", other)] + [("done",)])
        self.assertEqual(failed(run(seed, state, events)), ["no_wrong_right_clicks"])

    def test_left_click_instead_of_right_click_fails(self):
        seed = 7
        left, right, _ = full(seed)
        actions = left + [("left", right[0][1])] + right[1:] + [("done",)]
        state, events = synth.canvasclick_run(seed, actions)
        self.assertEqual(
            failed(run(seed, state, events)), ["no_extra_left_clicks", "right_targets"]
        )

    def test_extra_right_clicks_on_a_target_are_fine(self):
        seed = 7
        left, right, _ = full(seed)
        state, events = synth.canvasclick_run(seed, left + right + [right[0]] + [("done",)])
        self.assertTrue(run(seed, state, events)["passed"])

    def test_done_not_pressed_fails(self):
        seed = 7
        left, right, _ = full(seed)
        state, events = synth.canvasclick_run(seed, left + right)
        self.assertEqual(failed(run(seed, state, events)), ["done_after_clicks", "done_pressed"])

    def test_done_before_the_last_click_fails(self):
        seed = 7
        left, right, _ = full(seed)
        state, events = synth.canvasclick_run(seed, left + right[:-1] + [("done",)] + right[-1:])
        self.assertEqual(failed(run(seed, state, events)), ["done_after_clicks"])

    def test_mouse_down_without_mouse_up_is_not_a_genuine_click(self):
        seed = 7
        left, right, _ = full(seed)
        actions = left[:-1] + [("down_only", "left", left[-1][1])] + right + [("done",)]
        state, events = synth.canvasclick_run(seed, actions)
        result = run(seed, state, events)
        self.assertFalse(result["passed"])
        self.assertIn("mouse_pairs", failed(result))
        self.assertIn("no_extra_left_clicks", failed(result))

    def test_state_edited_without_ui_events_fails(self):
        seed = 7
        state, events = synth.canvasclick_run(seed)
        result = run(seed, state, events[:1])
        self.assertFalse(result["passed"])
        for name in (
            "mouse_pairs",
            "left_sequence_in_order",
            "right_targets",
            "done_pressed",
            "state_matches_events",
        ):
            self.assertIn(name, failed(result))

    def test_forged_state_counts_fail_only_consistency(self):
        seed = 7
        state, events = synth.canvasclick_run(seed)
        state["circles"][0]["left_clicks"] += 1
        self.assertEqual(failed(run(seed, state, events)), ["state_matches_events"])

    def test_forged_click_log_fails_consistency(self):
        seed = 7
        state, events = synth.canvasclick_run(seed)
        state["click_log"][0]["button"] = "right"
        self.assertEqual(failed(run(seed, state, events)), ["state_matches_events"])

    def test_event_hit_that_disagrees_with_geometry_fails_pairs(self):
        seed = 7
        state, events = synth.canvasclick_run(seed)
        first = next(e for e in events if e["type"] == "mouse")
        first["details"]["circle"] = (first["details"]["circle"] + 1) % 24
        result = run(seed, state, events)
        self.assertIn("mouse_pairs", failed(result))
        self.assertFalse(result["passed"])

    def test_phase_and_button_must_agree(self):
        seed = 7
        state, events = synth.canvasclick_run(seed)
        first = next(e for e in events if e["type"] == "mouse")
        first["details"]["button"] = "right"
        self.assertIn("mouse_pairs", failed(run(seed, state, events)))

    def test_wrong_seed_fails_integrity_and_layout(self):
        state, events = synth.canvasclick_run(7)
        result = run(8, state, events)
        self.assertFalse(result["checks"]["integrity"]["passed"])
        self.assertFalse(result["checks"]["circles_consistent"]["passed"])
        self.assertFalse(result["passed"])

    def test_garbage_state_fails_closed(self):
        seed = 7
        _, events = synth.canvasclick_run(seed)
        result = run(
            seed,
            {"mode": MODE, "seed": seed, "seq": len(events), "circles": "x", "click_log": 3},
            events,
        )
        self.assertFalse(result["passed"])


if __name__ == "__main__":
    unittest.main()
