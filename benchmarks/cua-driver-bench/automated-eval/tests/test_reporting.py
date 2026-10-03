from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).resolve().parents[1] / "reporting.py"
SPEC = importlib.util.spec_from_file_location("test_reporting_module", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
reporting = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(reporting)


class ReportingTests(unittest.TestCase):
    def test_extract_papercuts_is_deterministic(self) -> None:
        self.assertIsNone(reporting.extract_papercuts("no marker"))
        self.assertEqual(
            reporting.extract_papercuts("<PAPERCUTS>\n- first\n2. second\n</PAPERCUTS>"),
            ["first", "second"],
        )
        self.assertEqual(reporting.extract_papercuts("<PAPERCUTS>None</PAPERCUTS>"), [])

    def test_trajectory_normalization_preserves_order_and_omits_reasoning(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            trial = Path(temporary)
            (trial / "inputs" / "artifacts").mkdir(parents=True)
            (trial / "artifacts").mkdir()
            (trial / "inputs" / "artifacts" / "brief.md").write_text(
                "Task prompt", encoding="utf-8"
            )
            events = [
                {"type": "item.completed", "item": {"type": "reasoning", "text": "private"}},
                {"type": "item.completed", "item": {"type": "agent_message", "text": "<hello>"}},
                {
                    "type": "item.started",
                    "item": {
                        "id": "1",
                        "type": "mcp_tool_call",
                        "tool": "click",
                        "arguments": {"x": 1},
                    },
                },
                {
                    "type": "item.completed",
                    "item": {
                        "id": "1",
                        "type": "mcp_tool_call",
                        "tool": "click",
                        "result": {"text": "<script>bad</script>"},
                    },
                },
            ]
            (trial / "artifacts" / "codex-events.jsonl").write_text(
                "\n".join(json.dumps(event) for event in events) + "\n",
                encoding="utf-8",
            )
            normalized = reporting.load_trajectory_events(trial)

        self.assertEqual(
            [event["kind"] for event in normalized],
            ["user", "agent", "tool_call", "tool_result"],
        )
        self.assertNotIn("private", " ".join(event["text"] for event in normalized))
        self.assertIn("<script>bad</script>", normalized[-1]["text"])

    def test_bundle_writes_links_assets_and_missing_papercut_marker(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            trial_id = "CDB-S01-0.28.0-run"
            trial = output / "trials" / trial_id
            (trial / "inputs" / "artifacts").mkdir(parents=True)
            (trial / "artifacts").mkdir()
            (trial / "observer" / "cua-driver-recording" / "turn-00001").mkdir(parents=True)
            (trial / "inputs" / "artifacts" / "brief.md").write_text("prompt", encoding="utf-8")
            (trial / "artifacts" / "codex-events.jsonl").write_text(
                "\n".join(
                    [
                        json.dumps(
                            {
                                "type": "item.started",
                                "item": {
                                    "id": "1",
                                    "type": "mcp_tool_call",
                                    "tool": "observe",
                                    "arguments": {},
                                },
                            }
                        ),
                        json.dumps(
                            {
                                "type": "item.completed",
                                "item": {
                                    "id": "1",
                                    "type": "mcp_tool_call",
                                    "tool": "observe",
                                    "result": "<script>bad</script>",
                                },
                            }
                        ),
                        json.dumps(
                            {
                                "type": "item.completed",
                                "item": {"type": "agent_message", "text": "done"},
                            }
                        ),
                    ]
                )
                + "\n",
                encoding="utf-8",
            )
            (
                trial / "observer" / "cua-driver-recording" / "turn-00001" / "screenshot.png"
            ).write_bytes(b"png")
            report = {
                "trials": [
                    {
                        "task": "CDB-S01",
                        "version": "0.28.0",
                        "trial_id": trial_id,
                        "passed": True,
                        "score": 1,
                        "total_ms": 1,
                        "cua_calls": 1,
                        "input_actions": 1,
                        "termination": "completed",
                        "participation_required": True,
                        "participation_status": "failed",
                        "participation_detail": "application identity unavailable",
                        "recorded_input_actions": 0,
                        "recording_input_actions_complete": False,
                        "recording_detail": "recorded 0 of 1 successful input actions",
                        "codex_tokens": None,
                        "foreground_disturbance_available": True,
                        "foreground_keyboard_focus_available": True,
                        "foreground_drag_available": True,
                        "foreground_cursor_trajectory_available": True,
                    }
                ],
                "comparisons": [],
            }
            reporting.write_html_bundle(report, output)
            html = (output / "report" / "trials" / trial_id / "trajectory.html").read_text(
                encoding="utf-8"
            )
            papercuts = (output / "report" / "trials" / trial_id / "papercuts.md").read_text(
                encoding="utf-8"
            )
            comparison_html = (output / "report" / "index.html").read_text(encoding="utf-8")

        self.assertIn("prompt", html)
        self.assertIn("assets/0001.png", html)
        self.assertIn("&lt;script&gt;bad&lt;/script&gt;", html)
        self.assertNotIn("<script>bad</script>", html)
        self.assertIn("Papercut section was not produced", papercuts)
        self.assertIsNone(report["trials"][0]["papercut_count"])
        self.assertIn("Diagnostic Integrity", comparison_html)
        self.assertIn("recorded 0 of 1 successful input actions", comparison_html)


if __name__ == "__main__":
    unittest.main()
