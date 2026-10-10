from __future__ import annotations

import unittest
from unittest.mock import patch

from cua_bench_runtime.canon import digest_json
from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.model import ObserverReport
from cua_bench_runtime.participation import evaluate_participation, normalize_event


REQUIREMENTS = (
    {
        "id": "record-update",
        "target": {
            "application_id": "example-desk",
            "surface_id": "record-detail",
        },
        "sequence": [
            {
                "kind": "act",
                "required_facts": {"record_id": "ITEM-1042"},
            },
            {
                "kind": "readback",
                "required_facts": {
                    "record_id": "ITEM-1042",
                    "state": "active",
                },
            },
        ],
    },
)


def event(
    kind: str,
    *,
    application: str = "example-desk",
    surface: str = "record-detail",
    correlation: str = "corr-1",
    facts: dict | None = None,
) -> dict:
    return {
        "provider": {"id": "example.driver", "version": "1.2.3"},
        "capability_class": ("pointer_input" if kind == "act" else "accessibility_observation"),
        "target": {
            "application_id": application,
            "surface_id": surface,
            "platform_application_id": "com.example.synthetic",
            "process_id": 417,
            "window_id": "opaque-window-1",
        },
        "kind": kind,
        "correlation_id": correlation,
        "fact_digests": {key: digest_json(value) for key, value in (facts or {}).items()},
    }


class ParticipationTests(unittest.TestCase):
    def evaluate(self, report: ObserverReport, *, bindings: dict | None = None) -> dict:
        sequence = iter(f"event-{index}" for index in range(10))

        def append(_kind: str, _payload: dict) -> str:
            return next(sequence)

        return evaluate_participation(
            REQUIREMENTS,
            report,
            append,
            bindings=bindings
            or {
                "trial_id": "trial-1",
                "task_digest": "sha256:" + "1" * 64,
                "config_digest": "sha256:" + "2" * 64,
            },
        )

    def test_shell_only_success_has_unavailable_participation(self) -> None:
        receipt = self.evaluate(
            ObserverReport(
                name="none",
                trust="unavailable",
                detail="no driver events",
            )
        )
        self.assertIsNone(receipt["passed"])
        self.assertEqual(receipt["status"], "unavailable")
        self.assertEqual(receipt["requirements"][0]["status"], "unavailable")

    def test_unrelated_gui_activity_does_not_satisfy_requirement(self) -> None:
        report = ObserverReport(
            name="proxy",
            trust="certifying",
            events=(
                event("act", application="calculator", facts={"record_id": "ITEM-1042"}),
                event(
                    "readback",
                    application="calculator",
                    facts={"record_id": "ITEM-1042", "state": "active"},
                ),
            ),
        )
        receipt = self.evaluate(report)
        self.assertFalse(receipt["passed"])
        self.assertEqual(receipt["status"], "failed")

    def test_ordered_correlated_action_and_readback_pass(self) -> None:
        report = ObserverReport(
            name="proxy",
            trust="certifying",
            events=(
                event("act", facts={"record_id": "ITEM-1042"}),
                event(
                    "readback",
                    facts={"record_id": "ITEM-1042", "state": "active"},
                ),
            ),
        )
        receipt = self.evaluate(report)
        self.assertTrue(receipt["passed"])
        self.assertEqual(
            receipt["requirements"][0]["evidence_event_hashes"],
            [
                "event-0",
                "event-1",
            ],
        )

    def test_different_correlations_do_not_pass(self) -> None:
        report = ObserverReport(
            name="proxy",
            trust="certifying",
            events=(
                event("act", correlation="a", facts={"record_id": "ITEM-1042"}),
                event(
                    "readback",
                    correlation="b",
                    facts={"record_id": "ITEM-1042", "state": "active"},
                ),
            ),
        )
        self.assertFalse(self.evaluate(report)["passed"])

    def test_declared_platform_identity_fails_closed_when_host_is_missing(self) -> None:
        requirement = {
            **REQUIREMENTS[0],
            "target": {
                **REQUIREMENTS[0]["target"],
                "platform_application_ids": {"macos": ["com.example.synthetic"]},
            },
        }
        report = ObserverReport(
            name="proxy",
            trust="certifying",
            events=(
                event("act", facts={"record_id": "ITEM-1042"}),
                event(
                    "readback",
                    facts={"record_id": "ITEM-1042", "state": "active"},
                ),
            ),
        )
        with patch("cua_bench_runtime.participation.sys.platform", "linux"):
            receipt = evaluate_participation(
                (requirement,),
                report,
                lambda _kind, _payload: "event-hash",
                bindings={},
            )
        self.assertFalse(receipt["passed"])
        self.assertIn("no application identity for linux", receipt["requirements"][0]["reason"])

    def test_raw_screen_content_is_rejected(self) -> None:
        raw = event("act", facts={"record_id": "ITEM-1042"})
        raw["screen_text"] = "private window contents"
        with self.assertRaisesRegex(ValidationFailure, "unsupported fields"):
            normalize_event(raw)

    def test_receipt_is_bound_to_config(self) -> None:
        report = ObserverReport(
            name="proxy",
            trust="certifying",
            events=(
                event("act", facts={"record_id": "ITEM-1042"}),
                event(
                    "readback",
                    facts={"record_id": "ITEM-1042", "state": "active"},
                ),
            ),
        )
        first = self.evaluate(report)
        second = self.evaluate(
            report,
            bindings={
                **first["bindings"],
                "config_digest": "sha256:" + "3" * 64,
            },
        )
        self.assertNotEqual(first["receipt_digest"], second["receipt_digest"])


if __name__ == "__main__":
    unittest.main()
