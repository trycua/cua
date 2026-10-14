import unittest

from lifecycle_evidence import held_release, primary_wire_events, unchanged_primary


def button(kind, timestamp, number=1):
    return {"kind": kind, "time": timestamp, "button": number}


class LifecycleEvidenceTest(unittest.TestCase):
    def test_release_requires_input_held_at_fault(self):
        rows = [button("button-press", 10), button("button-release", 40_000_010)]
        result = held_release(rows, fault_ns=20_000_010)
        self.assertEqual(result["release_latency_ms"], 20)
        for bad in ([], rows[:1], rows[1:],
                    [button("button-press", 30_000_010), rows[1]],
                    [rows[0], button("button-release", 11)]):
            with self.subTest(rows=bad), self.assertRaises(AssertionError):
                held_release(bad, fault_ns=20_000_010)

    def test_late_natural_completion_is_not_prompt_cancellation(self):
        with self.assertRaises(AssertionError):
            held_release([button("button-press", 10), button("button-release", 2_000_000_010)],
                         fault_ns=20_000_010)

    def test_duplicate_press_wrong_release_and_bad_order_refuse(self):
        press = button("button-press", 10)
        release = button("button-release", 40)
        for rows in ([press, press, release], [press, button("button-release", 40, 2)],
                     [press, release, button("button-release", 30)]):
            with self.subTest(rows=rows), self.assertRaises(AssertionError):
                held_release(rows, fault_ns=20)

    def test_wire_parser_covers_both_wayland_debug_formats(self):
        text = "[1] wl_pointer#8.motion(2, 3, 4)\n[2] wl_keyboard@9.key(1, 2, 30, 1)"
        self.assertEqual(primary_wire_events(text), ["wl_pointer.motion", "wl_keyboard.key"])
        self.assertEqual(primary_wire_events("wl_pointer#8.frame()\nwl_surface#4.commit()"), [])

    def test_warp_and_return_is_not_hidden_by_matching_endpoints(self):
        state = {"held": True, "clicks": 0, "keys": ""}
        wire = "wl_pointer#8.motion(2, 80, 80)\nwl_pointer#8.motion(3, 30, 30)"
        with self.assertRaises(AssertionError):
            unchanged_primary(state, state, wire)

    def test_primary_state_needs_real_grab_and_complete_fields(self):
        state = {"held": True, "clicks": 0, "keys": ""}
        self.assertEqual(unchanged_primary(state, state, "")["primary_wire_input_events"], 0)
        for after in ({}, {**state, "held": False}, {**state, "keys": "a"}):
            with self.assertRaises(AssertionError):
                unchanged_primary(state, after, "")
        with self.assertRaises(AssertionError):
            unchanged_primary({**state, "held": False}, {**state, "held": False}, "")


if __name__ == "__main__":
    unittest.main()
