from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import summarize_sentinel  # noqa: E402

SENT = "ai.cua.benchsentinel"
T0 = 1_760_000_000_000.0


class Log:
    """Builds a synthetic sentinel log; time advances 50 ms per sample."""

    def __init__(self) -> None:
        self.lines: list[dict] = []
        self.t = T0
        self.armed = False
        self.mouse = [500.0, 400.0]
        self.front = {"bid": SENT, "pid": 100}
        self.idle = {"move": 5.0, "down": 5.0, "key": 5.0, "scroll": 5.0}

    def _base(self, ev: str, **extra) -> dict:
        line = {"ev": ev, "t": self.t, "armed": self.armed}
        line.update(extra)
        return line

    def _sample_fields(self) -> dict:
        return {
            "mouse": list(self.mouse),
            "front": dict(self.front),
            "active": self.front["bid"] == SENT,
            "key": self.front["bid"] == SENT,
            "idle": dict(self.idle),
        }

    def sample(self, n: int = 1, **overrides) -> "Log":
        for _ in range(n):
            self.t += 50
            for k in self.idle:
                self.idle[k] += 0.05
            for k, v in overrides.items():
                setattr(self, k, v)
            self.lines.append(self._base("s", **self._sample_fields()))
        return self

    def event(self, ev: str, **extra) -> "Log":
        self.t += 5
        self.lines.append(self._base(ev, **extra))
        return self

    def arm(self) -> "Log":
        self.armed = True
        self.t += 5
        self.lines.append(self._base("armed", **self._sample_fields()))
        return self

    def disarm(self) -> "Log":
        self.armed = False
        self.t += 5
        self.lines.append(self._base("disarmed", **self._sample_fields()))
        return self

    def hid(self, key: str, value: float = 0.01) -> "Log":
        self.idle[key] = value
        return self

    def write(self, directory: str, torn_tail: bool = False) -> str:
        path = Path(directory) / "sentinel.jsonl"
        text = "".join(json.dumps(line) + "\n" for line in self.lines)
        if torn_tail:
            text += '{"ev":"s","t":12'
        path.write_text(text, "utf-8")
        return str(path)


class SummarizeSentinelTests(unittest.TestCase):
    def summarize(self, log: Log, **kwargs) -> dict:
        with tempfile.TemporaryDirectory() as tmp:
            return summarize_sentinel.summarize(log.write(tmp, **kwargs))

    def test_quiet_window_has_zero_disturbance(self) -> None:
        log = Log().sample(5).arm().sample(40).disarm().sample(3)
        result = self.summarize(log)
        self.assertTrue(result["available"])
        self.assertEqual(result["samples"], 40)
        self.assertEqual(result["front_changes"], 0)
        self.assertEqual(result["front_changed_to"], [])
        self.assertEqual(result["key_loss"], 0)
        self.assertEqual(result["activations_lost"], 0)
        self.assertEqual(result["keystrokes_leaked"], 0)
        self.assertEqual(result["clicks_leaked"], 0)
        self.assertEqual(result["scrolls_leaked"], 0)
        self.assertEqual(result["pointer_max_deviation_px"], 0.0)
        self.assertEqual(result["pointer_deviation_episodes"], 0)
        self.assertEqual(result["hid_events"], {"move": 0, "down": 0, "key": 0, "scroll": 0})
        self.assertAlmostEqual(result["duration_s"], 2.01, places=2)

    def test_events_outside_armed_window_are_ignored(self) -> None:
        log = Log().sample(5)
        log.event("keyDown", chars="a", keyCode=0).event("mouseDown", button=0, loc=[1, 1])
        log.hid("key").hid("move").sample(2)
        log.arm().sample(12).disarm()
        log.event("keyDown", chars="b", keyCode=11).event("didResignKey")
        log.hid("down").sample(2, mouse=[900.0, 900.0], front={"bid": "com.apple.Safari", "pid": 7})
        result = self.summarize(log)
        self.assertEqual(result["keystrokes_leaked"], 0)
        self.assertEqual(result["clicks_leaked"], 0)
        self.assertEqual(result["key_loss"], 0)
        self.assertEqual(result["front_changes"], 0)
        self.assertEqual(result["pointer_max_deviation_px"], 0.0)
        self.assertEqual(result["hid_events"], {"move": 0, "down": 0, "key": 0, "scroll": 0})
        self.assertEqual(result["samples"], 12)

    def test_front_changes_count_transitions_away_from_sentinel(self) -> None:
        calc = {"bid": "com.apple.calculator", "pid": 200}
        safari = {"bid": "com.apple.Safari", "pid": 300}
        log = Log().arm().sample(10)
        log.sample(3, front=calc)  # steal 1
        log.sample(2, front=dict(log.front))  # stays on Calculator: no new change
        log.sample(3, front={"bid": SENT, "pid": 100})  # back to sentinel: not counted
        log.sample(3, front=calc)  # steal 2
        log.sample(3, front=safari)  # steal 3 (calc -> safari)
        log.sample(2, front={"bid": SENT, "pid": 100})
        log.disarm()
        result = self.summarize(log)
        self.assertEqual(result["front_changes"], 3)
        self.assertEqual(result["front_changed_to"], ["com.apple.Safari", "com.apple.calculator"])

    def test_front_event_between_samples_is_counted_once(self) -> None:
        calc = {"bid": "com.apple.calculator", "pid": 200}
        log = Log().arm().sample(10)
        log.event("front", front=calc)
        log.sample(5, front=calc)
        log.disarm()
        result = self.summarize(log)
        self.assertEqual(result["front_changes"], 1)

    def test_front_without_bundle_id_uses_pid(self) -> None:
        log = Log().arm().sample(10).sample(2, front={"bid": None, "pid": 4242}).disarm()
        result = self.summarize(log)
        self.assertEqual(result["front_changed_to"], ["pid:4242"])

    def test_already_other_app_at_arm_is_not_a_change(self) -> None:
        term = {"bid": "com.apple.Terminal", "pid": 50}
        log = Log().sample(2, front=term).arm().sample(12).disarm()
        result = self.summarize(log)
        self.assertEqual(result["front_changes"], 0)

    def test_event_counts(self) -> None:
        log = Log().arm().sample(10)
        log.event("didResignKey").event("didResignActive").event("didResignActive")
        log.event("keyDown", chars="x", keyCode=7).event("keyDown", chars="y", keyCode=16)
        log.event("mouseDown", button=0, loc=[3, 4]).event("mouseUp", button=0, loc=[3, 4])
        log.event("scrollWheel", dx=0.0, dy=-2.0).event("scrollWheel", dx=0.0, dy=-1.0)
        log.event("scrollWheel", dx=0.0, dy=-1.0)
        log.sample(2).disarm()
        result = self.summarize(log)
        self.assertEqual(result["key_loss"], 1)
        self.assertEqual(result["activations_lost"], 2)
        self.assertEqual(result["keystrokes_leaked"], 2)
        self.assertEqual(result["clicks_leaked"], 1)
        self.assertEqual(result["scrolls_leaked"], 3)

    def test_pointer_deviation_and_episodes(self) -> None:
        log = Log().arm().sample(3)
        log.sample(2, mouse=[505.0, 400.0])  # 5 px: below threshold
        log.sample(2, mouse=[530.0, 400.0])  # 30 px: episode 1
        log.sample(2, mouse=[540.0, 400.0])  # 40 px: still episode 1
        log.sample(2, mouse=[500.0, 400.0])  # back home
        log.sample(2, mouse=[500.0, 415.0])  # 15 px: episode 2
        log.sample(2, mouse=[500.0, 405.0])  # home again
        log.sample(2, mouse=[500.0, 500.0])  # 100 px: episode 3
        log.disarm()
        result = self.summarize(log)
        self.assertEqual(result["pointer_deviation_episodes"], 3)
        self.assertAlmostEqual(result["pointer_max_deviation_px"], 100.0, places=2)

    def test_pointer_reference_is_position_at_armed_instant(self) -> None:
        log = Log().sample(3, mouse=[100.0, 100.0])
        log.arm()  # armed at (100, 100)
        log.sample(12, mouse=[103.0, 104.0])  # 5 px away
        log.disarm()
        result = self.summarize(log)
        self.assertAlmostEqual(result["pointer_max_deviation_px"], 5.0, places=2)
        self.assertEqual(result["pointer_deviation_episodes"], 0)

    def test_hid_events_count_idle_drops(self) -> None:
        log = Log().arm().sample(3)
        log.hid("move").sample(1)  # move drop 1
        log.sample(3)
        log.hid("move").sample(1)  # move drop 2
        log.hid("move", 0.0).sample(1)  # move drop 3 (0.05 -> 0.06 would be no drop)
        log.hid("key").sample(1)  # key drop 1
        log.hid("down").hid("scroll").sample(1)  # down 1, scroll 1
        log.sample(3).disarm()
        result = self.summarize(log)
        self.assertEqual(result["hid_events"], {"move": 3, "down": 1, "key": 1, "scroll": 1})

    def test_hid_drop_between_arm_line_and_first_sample_counts(self) -> None:
        log = Log().arm()
        log.hid("key").sample(12)
        result = self.summarize(log)
        self.assertEqual(result["hid_events"]["key"], 1)

    def test_never_disarmed_runs_to_eof(self) -> None:
        log = Log().sample(2).arm().sample(15)
        log.event("keyDown", chars="q", keyCode=12)
        result = self.summarize(log)
        self.assertEqual(result["samples"], 15)
        self.assertEqual(result["keystrokes_leaked"], 1)
        self.assertTrue(result["available"])

    def test_too_few_samples_is_unavailable(self) -> None:
        log = Log().arm().sample(9).disarm()
        result = self.summarize(log)
        self.assertEqual(result["samples"], 9)
        self.assertFalse(result["available"])

    def test_never_armed_is_unavailable_with_zeros(self) -> None:
        log = Log().sample(30)
        log.event("keyDown", chars="a", keyCode=0)
        result = self.summarize(log)
        self.assertFalse(result["available"])
        self.assertEqual(result["samples"], 0)
        self.assertEqual(result["keystrokes_leaked"], 0)
        self.assertEqual(result["duration_s"], 0.0)

    def test_only_first_armed_window_counts(self) -> None:
        log = Log().arm().sample(12).disarm()
        log.arm().event("keyDown", chars="z", keyCode=6).sample(12).disarm()
        result = self.summarize(log)
        self.assertEqual(result["samples"], 12)
        self.assertEqual(result["keystrokes_leaked"], 0)

    def test_torn_last_line_is_tolerated(self) -> None:
        log = Log().arm().sample(12)
        result = self.summarize(log, torn_tail=True)
        self.assertEqual(result["samples"], 12)

    def test_cli_prints_json(self) -> None:
        log = Log().arm().sample(12).disarm()
        with tempfile.TemporaryDirectory() as tmp:
            path = log.write(tmp)
            import contextlib
            import io

            buffer = io.StringIO()
            with contextlib.redirect_stdout(buffer):
                self.assertEqual(summarize_sentinel.main([path]), 0)
        self.assertEqual(json.loads(buffer.getvalue())["samples"], 12)


class WindowsRaisedTests(unittest.TestCase):
    """Amendment 14 (CUA-1282): windows ordered above the sentinel's window."""

    def summarize(self, lines: list[dict]) -> dict:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "s.jsonl"
            path.write_text("".join(json.dumps(x) + "\n" for x in lines), "utf-8")
            return summarize_sentinel.summarize(str(path))

    @staticmethod
    def line(ev: str, i: int, above: list[str] | None, armed: bool = True) -> dict:
        out = {"ev": ev, "t": T0 + 50 * i, "armed": armed, "mouse": [1.0, 1.0],
               "front": {"bid": SENT, "pid": 1}, "idle": {"move": 9, "down": 9, "key": 9, "scroll": 9}}
        if above is not None:
            out["above_n"] = len(above)
            if above:
                out["above"] = above
        return out

    def test_old_logs_are_not_measured(self) -> None:
        lines = [self.line("armed", 0, None)] + [self.line("s", i, None) for i in range(1, 12)]
        result = self.summarize(lines)
        self.assertIsNone(result["windows_raised"])
        self.assertEqual(result["raised_by"], [])

    def test_counts_raise_episodes_not_samples(self) -> None:
        seq = [[]] * 3 + [["BenchLab"]] * 5 + [[]] * 2 + [["BenchLab", "Google Chrome"]] * 3
        lines = [self.line("armed", 0, [])] + [self.line("s", i + 1, a) for i, a in enumerate(seq)]
        lines.append(self.line("disarmed", 20, [], armed=False))
        result = self.summarize(lines)
        self.assertEqual(result["windows_raised"], 2)
        self.assertEqual(result["raised_by"], ["BenchLab", "Google Chrome"])

    def test_tool_overlays_are_metadata_not_raises(self) -> None:
        seq = [["Cua Driver Bench Script"]] * 3 + [["ChatGPT Computer Use"]] * 3
        lines = [self.line("armed", 0, [])] + [self.line("s", i + 1, a) for i, a in enumerate(seq)]
        lines += [self.line("s", 20 + i, []) for i in range(6)]
        result = self.summarize(lines)
        self.assertEqual(result["windows_raised"], 0)
        self.assertEqual(result["overlay_owners"], ["ChatGPT Computer Use", "Cua Driver Bench Script"])

    def test_user_activity_counts(self) -> None:
        lines = [self.line("armed", 0, [])] + [self.line("s", i, []) for i in range(1, 12)]
        lines += [{"ev": "user_type", "t": T0 + 700, "armed": True}, {"ev": "user_blocked", "t": T0 + 800, "armed": True},
                  {"ev": "user_text", "t": T0 + 900, "armed": True, "intact": True}]
        ua = self.summarize(lines)["user_activity"]
        self.assertEqual((ua["typed"], ua["blocked"], ua["disrupted"]), (1, 1, True))
        plain = [self.line("armed", 0, [])] + [self.line("s", i, []) for i in range(1, 12)]
        self.assertIsNone(self.summarize(plain)["user_activity"])

    def test_windows_already_above_at_arm_are_not_raises(self) -> None:
        lines = [self.line("armed", 0, ["Terminal"])] + [self.line("s", i, ["Terminal"]) for i in range(1, 12)]
        result = self.summarize(lines)
        self.assertEqual(result["windows_raised"], 0)
        self.assertEqual(result["above_at_start"], ["Terminal"])


if __name__ == "__main__":
    unittest.main()
