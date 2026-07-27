from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import finalize_session_cleanup as finalize


def episode(mode: str, *, session_ended: bool) -> dict[str, object]:
    return {
        "mode": mode,
        "agent_failure": None,
        "evaluation": {"score": 0.0},
        "session_ended": session_ended,
        "resolved_models": ["gpt-test"],
        "reset_evidence": {
            "guest_chrome_profile": f"/tmp/{mode}",
            "initial_evaluation": {"score": 0.0},
        },
        "score_gain_from_initial": 0.0,
        "steps_executed": 1,
        "wall_seconds": 1.0,
        "model_seconds": 0.5,
        "cost": {"estimated_usd": 0.1},
    }


class SessionCleanupFinalizationTests(unittest.TestCase):
    def test_verified_vm_deletion_finalizes_only_the_last_arm(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cleanup_path = root / "cleanup.json"
            partial_path = root / "partial.json"
            provenance_path = root / "provenance.json"
            output_dir = root / "finalized"
            cleanup = {
                "namespace": "test-namespace",
                "cleanup_verified": True,
            }
            cleanup_path.write_text(json.dumps(cleanup), encoding="utf-8")
            provenance_path.write_text(
                json.dumps({"fleet": {"namespace": "test-namespace"}}),
                encoding="utf-8",
            )
            partial_path.write_text(
                json.dumps(
                    {
                        "task_id": "098",
                        "requested_model": "gpt-test",
                        "order": "treatment-first",
                        "pair_valid": False,
                        "pair_validation_errors": [
                            "screenshot_ax Cua Driver session cleanup was not verified"
                        ],
                        "run_error": None,
                        "episodes": [
                            episode("combined", session_ended=True),
                            episode("screenshot_ax", session_ended=False),
                        ],
                        "fleet_cleanup": {
                            "path": str(cleanup_path),
                            "record": cleanup,
                        },
                    }
                ),
                encoding="utf-8",
            )

            argv = [
                "finalize_session_cleanup.py",
                "--partial-paired-result",
                str(partial_path),
                "--provenance",
                str(provenance_path),
                "--output-dir",
                str(output_dir),
            ]
            with mock.patch.object(sys, "argv", argv):
                status = finalize.main()
            result = json.loads(
                (output_dir / "paired-result.json").read_text(encoding="utf-8")
            )

        self.assertEqual(status, 0)
        self.assertTrue(result["pair_valid"])
        self.assertTrue(result["episodes"][1]["session_ended"])
        self.assertEqual(
            result["episodes"][1]["session_end_evidence"]["method"],
            "verified_fleet_vm_deletion",
        )
        self.assertIn("supersedes_result", result)


if __name__ == "__main__":
    unittest.main()
