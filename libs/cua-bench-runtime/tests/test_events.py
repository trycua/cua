from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from cua_bench_runtime.clock import Clock
from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.events import EventLog, verify_event_log


class EventTests(unittest.TestCase):
    def test_round_trip_and_tamper_detection(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.ndjson"
            with EventLog(path, Clock()) as events:
                events.append("trial_started", {"id": "trial"}, sync=True)
                events.append("trial_finished", {"status": "completed"}, sync=True)
            self.assertEqual(len(verify_event_log(path)), 2)
            data = path.read_bytes().replace(b'"completed"', b'"tampered"')
            path.write_bytes(data)
            with self.assertRaises(ValidationFailure):
                verify_event_log(path)

    def test_truncated_tail_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.ndjson"
            with EventLog(path, Clock()) as events:
                events.append("trial_started", {})
            path.write_bytes(path.read_bytes().rstrip(b"\n"))
            with self.assertRaisesRegex(ValidationFailure, "truncated"):
                verify_event_log(path)


if __name__ == "__main__":
    unittest.main()
