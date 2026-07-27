from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

import run_persistent_web_certification as persistent


def tasks() -> list[dict[str, str]]:
    return [
        {
            "task_id": f"{index:03d}",
            "stratum": "test",
            "order": "control-first",
        }
        for index in range(1, 11)
    ]


class PersistentCertificationTests(unittest.TestCase):
    def test_adopts_model_result_but_retries_infrastructure_only_failure(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output_dir = Path(temporary)
            valid_dir = output_dir / "01-task001"
            invalid_dir = output_dir / "persistent-continuation" / "task002"
            valid_dir.mkdir()
            invalid_dir.mkdir(parents=True)
            (valid_dir / "paired-result.json").write_text(
                json.dumps(
                    {
                        "task_id": "001",
                        "pair_valid": True,
                        "episodes": [{"steps_executed": 1}],
                    }
                ),
                encoding="utf-8",
            )
            (invalid_dir / "paired-result.json").write_text(
                json.dumps(
                    {
                        "task_id": "002",
                        "pair_valid": False,
                        "episodes": [],
                        "run_error": "Fleet readiness timeout",
                    }
                ),
                encoding="utf-8",
            )

            adopted, attempts = persistent.existing_pair_results(
                output_dir=output_dir,
                tasks=tasks(),
            )

        self.assertEqual(set(adopted), {"001"})
        self.assertEqual([attempt["task_id"] for attempt in attempts], ["002"])

    def test_model_attempt_is_sealed_even_when_pair_is_invalid(self) -> None:
        self.assertTrue(
            persistent.result_has_model_attempt(
                {
                    "pair_valid": False,
                    "episodes": [{"steps_executed": 1}],
                }
            )
        )
        self.assertFalse(
            persistent.result_has_model_attempt(
                {
                    "pair_valid": False,
                    "episodes": [],
                }
            )
        )

    def test_summary_requires_verified_persistent_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output_dir = Path(temporary)
            manifest_path = output_dir / "manifest.json"
            manifest = {
                "max_estimated_model_cost_usd": 75,
                "minimum_valid_pair_rate": 0.8,
            }
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            records = [
                {
                    "pair_valid": True,
                    "estimated_model_cost_usd": 1,
                }
                for _ in range(10)
            ]
            args = SimpleNamespace(
                manifest=manifest_path,
                model="gpt-5.5",
                reasoning_effort="xhigh",
                max_steps=24,
                output_dir=output_dir,
            )

            summary = persistent.write_summary(
                args=args,
                manifest=manifest,
                tasks=tasks(),
                records=records,
                infrastructure_attempts=[],
                persistent_lifecycle={"cleanup_verified": False},
                stopped_reason=None,
                final=True,
            )

        self.assertFalse(summary["passed"])


if __name__ == "__main__":
    unittest.main()
