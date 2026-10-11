from __future__ import annotations

import unittest

import synth
from synth import C

PROBE, MODE = "PROBE-CANVAS", "canvas"
SEEDS = [1, 7, 42, 4242, 999_999]


def run(seed, state, events, bad=0):
    return synth.evaluate(PROBE, MODE, seed, state, events, bad)


def failed(result):
    return sorted(n for n, c in result["checks"].items() if not c["passed"])


def zone_center(seed, name):
    z = next(z for z in C.derive_canvas(seed)["zones"] if z["name"] == name)
    return z["x"] + z["w"] / 2, z["y"] + z["h"] / 2


class CanvasEvaluatorTests(unittest.TestCase):
    def test_genuine_run_passes_for_many_seeds(self):
        for seed in SEEDS:
            state, events = synth.canvas_run(seed)
            result = run(seed, state, events)
            self.assertTrue(result["passed"], (seed, result["checks"]))
            self.assertEqual(result["score"], 1.0)

    def test_layout_invariants(self):
        for seed in range(300):
            d = C.derive_canvas(seed)
            for z in d["zones"]:
                self.assertTrue(
                    0 <= z["x"] and z["x"] + 96 <= 700 and 0 <= z["y"] and z["y"] + 96 <= 420
                )
            for t in d["tiles"]:
                self.assertTrue(0 <= t["x"] and t["x"] + 56 <= 700 and t["y"] + 56 <= 420)
            # zones sit in the upper band, tiles in the lower band, so every tile must be dragged
            self.assertTrue(max(z["y"] + 96 for z in d["zones"]) < min(t["y"] for t in d["tiles"]))
            xs = sorted(z["x"] for z in d["zones"])
            self.assertTrue(xs[0] + 96 <= xs[1] and xs[1] + 96 <= xs[2])

    def test_tolerance_is_eight_pixels(self):
        seed = 7
        cx, cy = zone_center(seed, "red")
        # tile (56) in zone (96): centered slack is 20 px, plus the 8 px tolerance
        inside = {"red": (cx + 27.5, cy)}
        outside = {"red": (cx + 28.5, cy)}
        state, events = synth.canvas_run(seed, targets=inside)
        self.assertTrue(run(seed, state, events)["passed"])
        state, events = synth.canvas_run(seed, targets=outside)
        self.assertEqual(failed(run(seed, state, events)), ["placed_when_done", "red_in_zone"])

    def test_wrong_zone_fails_two_tiles(self):
        seed = 7
        targets = {"red": zone_center(seed, "green"), "green": zone_center(seed, "red")}
        state, events = synth.canvas_run(seed, targets=targets)
        result = run(seed, state, events)
        self.assertEqual(failed(result), ["green_in_zone", "placed_when_done", "red_in_zone"])

    def test_done_not_pressed_fails(self):
        seed = 7
        state, events = synth.canvas_run(seed, press_done=False)
        result = run(seed, state, events)
        self.assertEqual(failed(result), ["done_pressed", "placed_when_done"])
        self.assertFalse(result["passed"])

    def test_fewer_than_three_drag_sequences_fails(self):
        seed = 7
        state, events = synth.canvas_run(seed)
        # Remove the green drag entirely; state still claims the tile sits in its zone.
        kept = [e for e in events if e["details"].get("tile") != "green"]
        for i, e in enumerate(kept, 1):
            e["seq"] = i
        state["seq"] = len(kept)
        state["drag_sequences"] = 2
        result = run(seed, state, kept)
        self.assertIn("drag_sequences", failed(result))
        self.assertIn("positions_via_events", failed(result))
        self.assertFalse(result["passed"])

    def test_mouse_down_up_without_dragging_is_not_a_sequence(self):
        seed = 7
        state, events = synth.canvas_run(seed)
        kept = [e for e in events if e["details"].get("phase") != "mouseDragged"]
        for i, e in enumerate(kept, 1):
            e["seq"] = i
        state["seq"] = len(kept)
        result = run(seed, state, kept)
        self.assertIn("drag_sequences", failed(result))

    def test_state_edited_without_ui_events_fails(self):
        seed = 7
        state, events = synth.canvas_run(seed)
        result = run(seed, state, events[:1])
        self.assertFalse(result["passed"])
        for name in ("drag_sequences", "positions_via_events", "done_pressed"):
            self.assertIn(name, failed(result))

    def test_forged_positions_not_matching_events_fail(self):
        seed = 7
        state, events = synth.canvas_run(seed, targets={"blue": (5.0, 5.0)})
        # The state claims blue is in its zone, but the logged drags put it at (5, 5).
        cx, cy = zone_center(seed, "blue")
        for t in state["tiles"] + state["done_snapshot"]:
            if t["name"] == "blue":
                t["cx"], t["cy"] = cx, cy
        for e in events:
            if e["type"] == "done_click":
                e["details"]["tiles"] = state["done_snapshot"]
        result = run(seed, state, events)
        self.assertFalse(result["passed"])
        self.assertIn("positions_via_events", failed(result))

    def test_tile_not_moved_fails(self):
        seed = 7
        exp = C.derive_canvas(seed)
        # a tile that starts (almost) in place cannot occur by construction, so emulate a no-op drag
        state, events = synth.canvas_run(seed)
        t = next(t for t in exp["tiles"] if t["name"] == "red")
        start = (t["x"] + 28, t["y"] + 28)
        for tile in state["tiles"]:
            if tile["name"] == "red":
                tile["cx"], tile["cy"] = start
        self.assertIn("tiles_moved", failed(run(seed, state, events)))

    def test_garbage_state_fails_closed(self):
        seed = 7
        _, events = synth.canvas_run(seed)
        result = run(
            seed,
            {"mode": "canvas", "seed": seed, "seq": len(events), "tiles": "x", "zones": 5},
            events,
        )
        self.assertFalse(result["passed"])

    def test_wrong_seed_fails_integrity_and_zone_consistency(self):
        state, events = synth.canvas_run(7)
        result = run(8, state, events)
        self.assertFalse(result["checks"]["integrity"]["passed"])
        self.assertFalse(result["checks"]["zones_consistent"]["passed"])


if __name__ == "__main__":
    unittest.main()
