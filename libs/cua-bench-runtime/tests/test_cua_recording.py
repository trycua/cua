from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from cua_bench_runtime.adapters.cua_recording import (
    CuaRecordingObserver,
    _contains_facts,
    merge_app_maps,
)
from cua_bench_runtime.canon import digest_json


REQUIREMENT = {
    "id": "participation.record.update",
    "target": {
        "application_id": "application.example-desk",
        "surface_id": "surface.record.item-1042",
        "platform_application_ids": {"macos": ["com.github.Electron"]},
    },
    "sequence": [
        {
            "kind": "act",
            "required_facts": {"record_id": "ITEM-1042", "control_label": "commit"},
        },
        {
            "kind": "readback",
            "required_facts": {"state": "active", "result_code": "updated"},
        },
    ],
}


class CuaRecordingObserverTests(unittest.TestCase):
    def events(self, observer: CuaRecordingObserver, apps: dict[int, str]) -> list[dict]:
        with patch("cua_bench_runtime.adapters.cua_recording._platform_key", return_value="macos"):
            observer.platform = "macos"
            return observer._events(apps)

    def recording(self, root: Path, *, result_error: bool = False) -> Path:
        recording = root / "recording"
        action_turn = recording / "turn-00001"
        action_turn.mkdir(parents=True)
        (action_turn / "action.json").write_text(
            json.dumps(
                {
                    "tool": "click",
                    "arguments": {
                        "pid": 413,
                        "window_id": 71,
                        "control_label": "Commit",
                    },
                    "result_error": result_error,
                }
            ),
            encoding="utf-8",
        )
        (action_turn / "before_state.json").write_text(
            json.dumps(
                {
                    "pid": 413,
                    "window_id": 71,
                    "tree_markdown": 'record "ITEM-1042" button "Commit"',
                }
            ),
            encoding="utf-8",
        )
        (action_turn / "after_state.json").write_text(
            json.dumps(
                {
                    "pid": 413,
                    "window_id": 71,
                    "tree_markdown": 'record "ITEM-1042" editor updated',
                }
            ),
            encoding="utf-8",
        )

        readback_turn = recording / "turn-00002"
        readback_turn.mkdir()
        (readback_turn / "action.json").write_text(
            json.dumps(
                {
                    "tool": "snapshot",
                    "arguments": {"pid": 413, "window_id": 71},
                    "result_error": result_error,
                }
            ),
            encoding="utf-8",
        )
        (readback_turn / "before_state.json").write_text(
            json.dumps(
                {
                    "pid": 413,
                    "window_id": 71,
                    "tree_markdown": 'record "ITEM-1042" editor updated',
                }
            ),
            encoding="utf-8",
        )
        (readback_turn / "after_state.json").write_text(
            json.dumps(
                {
                    "pid": 413,
                    "window_id": 71,
                    "tree_markdown": ('record "ITEM-1042" state "Active" result code "Updated"'),
                }
            ),
            encoding="utf-8",
        )
        return recording

    def test_maps_action_and_readback_without_retaining_screen_text(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            observer = CuaRecordingObserver()
            observer.recording_dir = self.recording(Path(directory))
            observer.requirements = (REQUIREMENT,)
            observer.provider_version = "0.19.4"
            events = self.events(observer, {413: "com.github.Electron"})
        self.assertEqual([event["kind"] for event in events], ["act", "readback"])
        self.assertEqual(events[0]["fact_digests"]["record_id"], digest_json("ITEM-1042"))
        self.assertEqual(events[0]["correlation_id"], events[1]["correlation_id"])
        self.assertNotIn("ITEM-1042", json.dumps(events))
        self.assertNotIn("Active", json.dumps(events))

    def test_wrong_os_application_identity_is_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            observer = CuaRecordingObserver()
            observer.recording_dir = self.recording(Path(directory))
            observer.requirements = (REQUIREMENT,)
            self.assertEqual(self.events(observer, {413: "com.apple.calculator"}), [])

    def test_failed_driver_action_is_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            observer = CuaRecordingObserver()
            observer.recording_dir = self.recording(Path(directory), result_error=True)
            observer.requirements = (REQUIREMENT,)
            self.assertEqual(self.events(observer, {413: "com.github.Electron"}), [])

    def test_local_adapter_cannot_be_promoted_to_certifying(self) -> None:
        with self.assertRaises(TypeError):
            CuaRecordingObserver(trust="certifying")  # type: ignore[call-arg]
        observer = CuaRecordingObserver()
        with self.assertRaises(AttributeError):
            observer.trust = "certifying"  # type: ignore[misc]

    def test_pid_reuse_is_excluded_while_exited_apps_are_retained(self) -> None:
        self.assertEqual(
            merge_app_maps(
                {10: "com.example.exited", 11: "com.example.original"},
                {11: "com.example.reused", 12: "com.example.new"},
            ),
            {10: "com.example.exited", 12: "com.example.new"},
        )

    def test_act_requires_a_real_input_action_and_exact_fact_token(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            observer = CuaRecordingObserver()
            recording = self.recording(Path(directory))
            action_path = recording / "turn-00001" / "action.json"
            action = json.loads(action_path.read_text(encoding="utf-8"))
            action["tool"] = "snapshot"
            action_path.write_text(json.dumps(action), encoding="utf-8")
            observer.recording_dir = recording
            observer.requirements = (REQUIREMENT,)
            self.assertEqual(
                [event["kind"] for event in self.events(observer, {413: "com.github.Electron"})],
                ["readback"],
            )

    def test_fact_matching_does_not_accept_substrings(self) -> None:
        self.assertFalse(_contains_facts("state Active", {"operation": "activate"}))


if __name__ == "__main__":
    unittest.main()
