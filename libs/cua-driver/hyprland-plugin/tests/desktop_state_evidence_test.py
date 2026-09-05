"""Adversarial evidence checks; no desktop or plugin required."""
from copy import deepcopy
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

from desktop_state_evidence import agent_cleared, cancellation, compare_control, primary_effect, destroyed_resources_pruned, recovery_input


def status():
    return {"experiment": {"lanes": [{"lane": 0, "lease_active": False,
        "drag_active": False, "pointer_focus": False, "keyboard_focus": False,
        "held_button": 0, "held_keys": 0}]}}


def trace(events=()):
    kinds = [("start", 0, 0, 0, 0), *events, ("stop", 0, 0, 0, 0)]
    rows = [[index, index * 1_000_000, kind, x, y, actor, state]
            for index, (kind, x, y, actor, state) in enumerate(kinds, 1)]
    return {"hook": True, "active": False, "overflow": False, "timed_out": False,
            "count": len(rows), "events": rows}


class CancellationTest(unittest.TestCase):
    def test_destroyed_resources_require_real_pre_target_baseline_and_pruning(self):
        baseline = [{"lane": lane, "seat_resources": 2, "pointer_resources": 2, "keyboard_resources": 2}
                    for lane in (0, 1)]
        before = {"experiment": {"lanes": [{**row, "seat_resources": 3, "pointer_resources": 3, "keyboard_resources": 3}
                                          for row in baseline]}}
        after = {"experiment": {"lanes": baseline}}
        self.assertTrue(destroyed_resources_pruned(baseline, before, after)["destroyed_client_resources_pruned"])
        for initial, final in ((after, after), (before, before)):
            with self.assertRaises(AssertionError):
                destroyed_resources_pruned(baseline, initial, final)

    def arguments(self):
        return {"fault_ns": 1_100_000_000, "observed_ns": 1_110_000_000,
                "response_ns": 1_120_000_000, "status_ns": 1_130_000_000,
                "response": {"isError": True, "structuredContent": {"reason": "cancelled"}},
                "status": status(), "lane": 0}

    def rows(self):
        return [{"kind": "button-press", "button": 1, "time": 1_000_000_000},
                {"kind": "button-release", "button": 1, "time": 1_150_000_000}]

    def test_surviving_target_requires_all_independent_evidence(self):
        for case in ("move", "resize", "lock", "dpms"):
            result = cancellation(case, self.rows(), **self.arguments())
            self.assertEqual(result["release_latency_ms"], 50)
            self.assertTrue(result["agent_authority_cleared"])

    def test_drag_cannot_hide_other_input(self):
        for kind in ("key-press", "key-release", "scroll"):
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                cancellation("move", self.rows() + [{"kind": kind, "time": 1_200_000_000}], **self.arguments())

    def test_recovery_requires_exact_pair_before_cleanup(self):
        pair = [{"kind": "key-press", "key": "Escape", "time": 1},
                {"kind": "key-release", "key": "Escape", "time": 2}]
        self.assertTrue(recovery_input(pair, status(), 0)["recovery_key_balanced"])
        for bad in (pair[:1], pair * 2, pair + [{"kind": "scroll"}],
                    [{**row, "key": "a"} for row in pair]):
            with self.subTest(rows=bad), self.assertRaises(AssertionError):
                recovery_input(bad, status(), 0)
        held = status()
        held["experiment"]["lanes"][0]["held_keys"] = 1
        with self.assertRaises(AssertionError):
            recovery_input(pair, held, 0)

    def test_destroyed_target_does_not_require_impossible_release(self):
        result = cancellation("destroy", self.rows()[:1], **self.arguments(), target_destroyed=True)
        self.assertEqual(result["release_oracle"], "destroyed_surface_not_receivable")
        with self.assertRaises(AssertionError):
            cancellation("destroy", self.rows()[:1], **self.arguments())

    def test_missing_release_or_pre_fault_release_fails(self):
        for rows in (self.rows()[:1], [], [{**row, "time": row["time"] - 100_000_000} for row in self.rows()]):
            with self.subTest(rows=rows), self.assertRaises(AssertionError):
                cancellation("move", rows, **self.arguments())

    def test_late_or_naturally_completed_drag_cannot_pass(self):
        for update in ({"response_ns": 3_000_000_000}, {"status_ns": 3_000_000_000},
                       {"duration_ms": 800}, {"fault_ns": 900_000_000},
                       {"response": {"structuredContent": {"effect": "unverifiable"}}},
                       {"response": {"isError": True, "structuredContent": {"reason": "lease_expired"}}}):
            with self.subTest(update=update), self.assertRaises(AssertionError):
                cancellation("move", self.rows(), **{**self.arguments(), **update})

    def test_cleanup_requires_every_owned_state_field(self):
        for name in ("lease_active", "drag_active", "pointer_focus", "keyboard_focus", "held_button", "held_keys"):
            for missing in (False, True):
                data = status()
                if missing:
                    del data["experiment"]["lanes"][0][name]
                else:
                    data["experiment"]["lanes"][0][name] = 1
                with self.subTest(name=name, missing=missing), self.assertRaises(AssertionError):
                    agent_cleared(data, 0)
        with self.assertRaises(AssertionError):
            agent_cleared(status(), 1)


class ControlTest(unittest.TestCase):
    def setUp(self):
        self.before = {"held": True, "clicks": 1, "keys": "", "scroll": 0, "motion": 0}
        self.wm = {"pid": 123, "address": "0x123", "cursor": {"x": 0, "y": 0}, "workspace": 1}
        self.identity = {"case": "lock", "source_sha": "a" * 40}

    def effect(self, after=None, events=(), wire="", wm_after=None):
        return primary_effect(self.before, after or self.before, wire, trace(events),
                              self.wm, wm_after or self.wm)

    def control(self, effect):
        return {"result": "passed", "mode": "control", "identity": self.identity,
                "primary_effect": effect}

    def test_legitimate_lock_focus_changes_match_control(self):
        events = [("pointer_leave", 0, 0, 0, 0), ("keyboard_leave", 0, 0, 0, 0)]
        expected = self.effect(events=events, wire="wl_pointer#2.leave(5, wl_surface#1)")
        actual = self.effect(events=events + [events[0]], wire="wl_pointer#7.leave(6, wl_surface#8)")
        self.assertTrue(compare_control(self.control(expected), actual, self.identity)["fault_only_control_matched"])

    def test_extra_input_or_transient_warp_is_not_hidden_by_matching_endpoints(self):
        expected = self.effect()
        actuals = [self.effect(events=[("cursor", 100, 50, 0, 0), ("cursor", 0, 0, 0, 0)]),
                   self.effect(events=[("pointer_button", 0, 0, 0, 1), ("pointer_button", 0, 0, 0, 0)]),
                   self.effect(wire="wl_keyboard#2.key(5, 123, 30, 1)"),
                   self.effect(after={**self.before, "keys": "x"})]
        for actual in actuals:
            with self.subTest(actual=actual), self.assertRaises(AssertionError):
                compare_control(self.control(expected), actual, self.identity)

    def test_same_maximum_cannot_hide_extra_cursor_path(self):
        control_events = [("cursor", 100, 0, 0, 0), ("cursor", 0, 0, 0, 0)]
        expected = self.effect(events=control_events)
        actual = self.effect(events=control_events + [("cursor", 10, 0, 0, 0), ("cursor", 0, 0, 0, 0)])
        self.assertEqual(expected["max_cursor_displacement"], actual["max_cursor_displacement"])
        with self.assertRaises(AssertionError):
            compare_control(self.control(expected), actual, self.identity)

    def test_changed_boolean_cannot_hide_wrong_foreground_identity(self):
        expected = self.effect(wm_after={**self.wm, "pid": 456, "address": "0x456"})
        actual = self.effect(wm_after={**self.wm, "pid": 789, "address": "0x789"})
        with self.assertRaises(AssertionError):
            compare_control(self.control(expected), actual, self.identity)

    def test_extra_focus_round_trip_is_not_hidden_by_control_categories(self):
        events = [("pointer_leave", 0, 0, 0, 0), ("pointer_enter", 0, 0, 0, 0)]
        expected = self.effect(events=events)
        actual = self.effect(events=events + events)
        with self.assertRaises(AssertionError):
            compare_control(self.control(expected), actual, self.identity)

    def test_missing_or_wrong_control_and_incomplete_trace_fail_closed(self):
        expected = self.effect()
        control = self.control(expected)
        for update in ({"result": "failed"}, {"mode": "action"}, {"identity": {}}):
            with self.assertRaises(AssertionError):
                compare_control({**control, **update}, expected, self.identity)
        for field in ("overflow", "timed_out", "active"):
            incomplete = trace()
            incomplete[field] = True
            with self.assertRaises(AssertionError):
                primary_effect(self.before, self.before, "", incomplete, self.wm, self.wm)
        missing = deepcopy(expected)
        del missing["wire_inputs"]
        with self.assertRaises(AssertionError):
            compare_control(control, missing, self.identity)


class CleanupTest(unittest.TestCase):
    def test_fault_and_video_failure_still_close_all_owned_resources_and_retain_failure(self):
        from desktop_state_live import run
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bounds = {"x": 0, "y": 0, "width": 600, "height": 500}
            plan = {"background": {"pid": 101, "window_id": "a"},
                    "foreground": {"pid": 102, "window_id": "b"},
                    "background_bounds": bounds, "foreground_bounds": bounds,
                    "background_address": "0xabc", "from": [100, 100], "to": [200, 200],
                    "foreground_point": [300, 300], "move_to": [20, 20]}
            plan_path = root / "plan.json"
            plan_path.write_text(json.dumps(plan))
            module = root / "module.so"
            module.write_bytes(b"synthetic module")
            wire = root / "wire.log"
            wire.write_text("wl_pointer#2.button(5, 123, 272, 1)\n")
            args = SimpleNamespace(evidence=root / "attempt", plan=plan_path, mode="control", case="move",
                source_sha="a" * 40, module=module, control=None, driver=root / "driver",
                driver_socket=root / "driver.sock", compositor_pid=100, instance="synthetic",
                disposable=True, lock_fixture=None, compositor_exe=root / "Hyprland",
                primary_grab=root / "grab", input_directory=root,
                background_journal=root / "bg.jsonl", foreground_journal=root / "fg.jsonl",
                foreground_wire=wire)
            observer, client = MagicMock(), MagicMock()
            def tool(name, _arguments):
                if name == "stop_recording":
                    raise RuntimeError("synthetic video finalization failure")
                data = {"window_bounds": bounds, "screenshot_width": 600, "screenshot_height": 500,
                        "screen_width": 1024, "screen_height": 768, "video_active": True}
                return {"structuredContent": data}
            observer.tool.side_effect = client.tool.side_effect = tool
            fault = MagicMock()
            fault.snapshot.return_value = {}
            fault.rollback.return_value = {"restored": True}
            fault.apply.side_effect = RuntimeError("synthetic fault failure")
            grab = MagicMock()
            grab.poll.return_value = None
            grab.stdout.readline.return_value = "HELD\n"
            tracer = MagicMock()
            tracer.collect.return_value = trace()
            initial = status()
            initial["experiment"]["lease_active"] = False
            primary = {"held": True, "clicks": 0, "keys": "", "scroll": 0, "motion": 0}
            with patch("desktop_faults.FaultController", return_value=fault), \
                 patch("desktop_state_live.MCP", side_effect=[observer, client]), \
                 patch("desktop_state_live.Trace", return_value=tracer), \
                 patch("desktop_state_live.subprocess.check_output", return_value=json.dumps(initial)), \
                 patch("desktop_state_live.subprocess.Popen", return_value=grab), \
                 patch("desktop_state_live.select.select", return_value=([grab.stdout], [], [])), \
                 patch("desktop_state_live.state", return_value=primary), \
                 patch("desktop_state_live.journal", return_value=[]), \
                 patch("desktop_state_live.wm", return_value={"pid": 102}), \
                 redirect_stdout(io.StringIO()), self.assertRaises(AssertionError):
                run(args)
            fault.rollback.assert_called_once()
            fault.close.assert_called_once()
            grab.terminate.assert_called_once()
            client.close.assert_called_once()
            observer.close.assert_called_once()
            tracer.close.assert_called_once()
            result = json.loads((args.evidence / "result.json").read_text())
            self.assertEqual(result["result"], "failed")
            self.assertEqual(result["error"], "synthetic fault failure")
            self.assertTrue(result["cleanup_errors"])
            self.assertTrue((args.evidence / "foreground-wire-final.log").is_file())
            self.assertTrue((args.evidence / "failed-primary-trace.json").is_file())


if __name__ == "__main__":
    unittest.main()
