from __future__ import annotations

import unittest

from cua_bench_runtime.certification import (
    APPARATUS_FIELDS,
    build_certification_receipt,
    evaluate_apparatus,
    verify_certification_receipt,
)
from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.canon import digest_json


DIGEST = "sha256:" + "a" * 64
OTHER_DIGEST = "sha256:" + "b" * 64


def passing_apparatus() -> dict:
    value = {
        "environment_adapter": "lume-macos-certifying",
        "environment_declared_certifying": True,
        "apparatus_check": False,
        "cleanup_ok": True,
        "seed_provenance_digest": DIGEST,
        "pristine_fingerprint": DIGEST,
        "post_run_fingerprint": DIGEST,
        "post_run_matches_pristine": True,
        "initial_driver_identity_digest": DIGEST,
        "post_run_driver_identity_digest": DIGEST,
        "fresh_harness_workspace": True,
        "target_reset": True,
        "task_store_relative": "task-store",
        "agent_processes_frozen": True,
        "network_mode": "none",
        "network_enforcer": "guest-root-pf-anchor",
        "network_evidence_digest": DIGEST,
        "post_run_network_evidence_digest": DIGEST,
        "production_harness": None,
        "provider_proxy": None,
        "human_input_channel": "closed-no-vnc",
        "human_input_enforcer": "host-lume-vnc-disabled",
        "human_input_evidence_digest": DIGEST,
        "post_run_human_input_evidence_digest": DIGEST,
        "protected_endpoint_binding_digest": DIGEST,
        "sealed_endpoint_binding_digest": DIGEST,
        "vm_stopped_before_collection": True,
        "protected_collection_read_only": True,
        "collection_manifest_digest": DIGEST,
        "protected_collection_manifest_digest": DIGEST,
        "protected_log_digest": DIGEST,
        "protected_report_digest": DIGEST,
        "protected_log_tail": "a" * 64,
        "protected_log_records": 4,
        "protected_transport_integrity": True,
        "protected_evidence_complete": True,
        "protected_off_target_activity": False,
        "protected_tool_contract_validated": True,
        "observed_daemon_tool_schemas_sha256": "a" * 64,
        "expected_daemon_tool_list_envelope_sha256": "b" * 64,
        "observed_daemon_tool_list_envelope_sha256": "b" * 64,
        "protected_daemon_tool_list_envelope_validated": True,
        "inputs_unchanged": True,
        "evaluation_digest": DIGEST,
        "participation_receipt_digest": DIGEST,
        "participation_signature_digest": DIGEST,
        "execution_policy_receipt_digest": DIGEST,
        "execution_policy_integrity_passed": True,
        "execution_policy_integrity_violations": [],
    }
    assert set(value) == APPARATUS_FIELDS
    return value


def bindings() -> dict:
    return {
        "trial_id": "trial-one",
        "task_digest": DIGEST,
        "system_digest": DIGEST,
        "execution_policy_digest": DIGEST,
        "config_digest": DIGEST,
        "inputs_manifest_digest": DIGEST,
        "seed_provenance_digest": DIGEST,
        "agent_digest": DIGEST,
        "evaluator_digest": DIGEST,
    }


def passing_provider_proxy() -> dict:
    return {
        "enforcer": "host-cdb-connect-proxy",
        "schema_version": 1,
        "trial_id": "trial-one",
        "endpoint": "http://192.0.2.10:8443",
        "allowed_client_ip": "192.0.2.1",
        "provider_allowlist_sha256": DIGEST,
        "initial_client_binding_digest": DIGEST,
        "sealed_client_binding_digest": DIGEST,
        "initial_implementation_digest": DIGEST,
        "sealed_implementation_digest": DIGEST,
        "implementation_identity": "cb.provider-connect-proxy/v2",
        "initial_evidence_digest": DIGEST,
        "sealed_evidence_digest": OTHER_DIGEST,
        "artifact_sha256": DIGEST,
        "accepted_connections": 1,
        "rejected_connections": 0,
        "bytes_guest_to_provider": 100,
        "bytes_provider_to_guest": 200,
        "transcript_chain_digest": DIGEST,
        "active": False,
        "sealed": True,
    }


def participation(
    *,
    passed: bool | None = True,
    required: bool = True,
    trust: str = "certifying",
    status: str | None = None,
) -> dict:
    return {
        "required": required,
        "status": status
        or ("not_required" if not required else ("passed" if passed else "failed")),
        "passed": passed,
        "observer": {"trust": trust},
        "receipt_digest": DIGEST,
    }


class CertificationTests(unittest.TestCase):
    def test_legacy_nonproduction_v1_receipt_remains_verifiable(self) -> None:
        receipt = self.build()
        for field in (
            "protected_report_digest",
            "protected_tool_contract_validated",
            "observed_daemon_tool_schemas_sha256",
            "expected_daemon_tool_list_envelope_sha256",
            "observed_daemon_tool_list_envelope_sha256",
            "protected_daemon_tool_list_envelope_validated",
        ):
            receipt["apparatus"].pop(field)
        body = {key: value for key, value in receipt.items() if key != "receipt_digest"}
        receipt["receipt_digest"] = digest_json(body)
        self.assertEqual(verify_certification_receipt(receipt), receipt)

    def build(
        self,
        *,
        apparatus: dict | None = None,
        outcome: dict | None = None,
        participation_receipt: dict | None = None,
        comparison: dict | None = None,
    ) -> dict:
        outcome_value = (
            {"passed": False, "score": 0.0, "detail": {}} if outcome is None else outcome
        )
        participation_value = participation_receipt or participation()
        comparison_value = (
            {"eligible": False, "status": "unavailable", "receipt_digest": DIGEST}
            if comparison is None
            else comparison
        )
        apparatus_value = dict(apparatus or passing_apparatus())
        apparatus_value["evaluation_digest"] = digest_json(outcome_value)
        apparatus_value["participation_receipt_digest"] = participation_value["receipt_digest"]
        apparatus_value["execution_policy_receipt_digest"] = comparison_value["receipt_digest"]
        return build_certification_receipt(
            bindings=bindings(),
            apparatus=apparatus_value,
            outcome=outcome_value,
            participation=participation_value,
            comparison=comparison_value,
        )

    def test_failed_task_outcome_does_not_downgrade_apparatus(self) -> None:
        receipt = self.build()
        self.assertTrue(receipt["apparatus_decision"]["eligible"])
        self.assertFalse(receipt["outcome"]["passed"])
        self.assertTrue(receipt["eligible"])
        self.assertEqual(verify_certification_receipt(receipt), receipt)

    def test_comparison_ineligibility_is_independent(self) -> None:
        receipt = self.build(
            comparison={
                "eligible": False,
                "status": "failed",
                "receipt_digest": OTHER_DIGEST,
            }
        )
        self.assertFalse(receipt["comparison"]["eligible"])
        self.assertTrue(receipt["eligible"])

    def test_participation_failure_preserves_apparatus_decision(self) -> None:
        receipt = self.build(participation_receipt=participation(passed=False))
        self.assertTrue(receipt["apparatus_decision"]["eligible"])
        self.assertFalse(receipt["participation"]["eligible"])
        self.assertFalse(receipt["eligible"])

    def test_all_reachable_participation_states_build_and_verify(self) -> None:
        for required, passed, trust in (
            (False, None, "unavailable"),
            (True, True, "certifying"),
            (True, True, "non_certifying"),
            (True, False, "certifying"),
            (True, None, "unavailable"),
        ):
            with self.subTest(required=required, passed=passed, trust=trust):
                receipt = self.build(
                    participation_receipt=participation(
                        required=required,
                        passed=passed,
                        trust=trust,
                        status=("unavailable" if required and passed is None else None),
                    )
                )
                self.assertEqual(verify_certification_receipt(receipt), receipt)
                self.assertIs(
                    receipt["participation"]["eligible"],
                    bool((not required) or (passed and trust == "certifying")),
                )

    def test_each_security_attack_fails_with_specific_reason(self) -> None:
        attacks = {
            "post_run_fingerprint": (OTHER_DIGEST, "post_run_drift_detected"),
            "post_run_network_evidence_digest": (
                OTHER_DIGEST,
                "network_enforcement_changed",
            ),
            "post_run_human_input_evidence_digest": (
                OTHER_DIGEST,
                "human_input_enforcement_changed",
            ),
            "sealed_endpoint_binding_digest": (
                OTHER_DIGEST,
                "protected_endpoint_changed",
            ),
            "protected_off_target_activity": (True, "off_target_driver_activity"),
            "inputs_unchanged": (False, "runtime_inputs_changed"),
            "cleanup_ok": (False, "cleanup_incomplete"),
        }
        for field, (value, reason) in attacks.items():
            with self.subTest(field=field):
                apparatus = passing_apparatus()
                apparatus[field] = value
                receipt = self.build(apparatus=apparatus)
                self.assertFalse(receipt["eligible"])
                self.assertIn(reason, receipt["apparatus_decision"]["reasons"])
                if field == "protected_off_target_activity":
                    self.assertNotIn(
                        "protected_evidence_incomplete",
                        receipt["apparatus_decision"]["reasons"],
                    )

    def test_production_harness_requires_sealed_provider_proxy(self) -> None:
        apparatus = passing_apparatus()
        apparatus["production_harness"] = "codex"
        apparatus["provider_proxy"] = passing_provider_proxy()
        self.assertTrue(self.build(apparatus=apparatus)["eligible"])

        apparatus["provider_proxy"]["sealed_client_binding_digest"] = OTHER_DIGEST
        receipt = self.build(apparatus=apparatus)
        self.assertFalse(receipt["eligible"])
        self.assertIn(
            "provider_proxy_unavailable",
            receipt["apparatus_decision"]["reasons"],
        )

    def test_production_proxy_is_bound_to_exact_trial(self) -> None:
        apparatus = passing_apparatus()
        apparatus["production_harness"] = "codex"
        apparatus["provider_proxy"] = passing_provider_proxy()
        apparatus["provider_proxy"]["trial_id"] = "trial-two"
        receipt = self.build(apparatus=apparatus)
        self.assertFalse(receipt["eligible"])
        self.assertIn(
            "provider_proxy_trial_binding_mismatch",
            receipt["binding_decision"]["reasons"],
        )

    def test_production_mediator_tool_contract_is_signed_apparatus(self) -> None:
        for field, value in (
            ("protected_report_digest", None),
            ("protected_tool_contract_validated", False),
            ("observed_daemon_tool_schemas_sha256", None),
            ("observed_daemon_tool_schemas_sha256", "b" * 63),
            ("protected_daemon_tool_list_envelope_validated", False),
            ("expected_daemon_tool_list_envelope_sha256", None),
            ("observed_daemon_tool_list_envelope_sha256", "c" * 64),
        ):
            with self.subTest(field=field, value=value):
                apparatus = passing_apparatus()
                apparatus["production_harness"] = "codex"
                apparatus["provider_proxy"] = passing_provider_proxy()
                apparatus[field] = value
                receipt = self.build(apparatus=apparatus)
                self.assertFalse(receipt["eligible"])
                self.assertIn(
                    "mediator_tool_contract_unavailable",
                    receipt["apparatus_decision"]["reasons"],
                )

    def test_production_harness_missing_provider_proxy_is_unavailable(self) -> None:
        apparatus = passing_apparatus()
        apparatus["production_harness"] = "claude-code"
        receipt = self.build(apparatus=apparatus)
        self.assertEqual(receipt["apparatus_decision"]["status"], "unavailable")
        self.assertIn(
            "provider_proxy_unavailable",
            receipt["apparatus_decision"]["reasons"],
        )

    def test_opencode_is_a_supported_production_harness(self) -> None:
        apparatus = passing_apparatus()
        apparatus["production_harness"] = "opencode"
        apparatus["provider_proxy"] = passing_provider_proxy()
        receipt = self.build(apparatus=apparatus)
        self.assertTrue(receipt["apparatus_decision"]["eligible"])
        self.assertNotIn(
            "production_harness_unsupported",
            receipt["apparatus_decision"]["reasons"],
        )

    def test_missing_evidence_is_unavailable_not_passed(self) -> None:
        apparatus = passing_apparatus()
        apparatus["post_run_fingerprint"] = None
        receipt = self.build(apparatus=apparatus)
        self.assertEqual(receipt["apparatus_decision"]["status"], "unavailable")
        self.assertFalse(receipt["eligible"])

    def test_detected_failure_is_not_masked_by_unavailable_evidence(self) -> None:
        apparatus = passing_apparatus()
        apparatus["post_run_fingerprint"] = None
        apparatus["protected_off_target_activity"] = True
        receipt = self.build(apparatus=apparatus)
        self.assertEqual(receipt["apparatus_decision"]["status"], "failed")
        self.assertIn(
            "off_target_driver_activity",
            receipt["apparatus_decision"]["reasons"],
        )

    def test_tampered_decision_and_digest_fail_closed(self) -> None:
        receipt = self.build()
        receipt["eligible"] = False
        with self.assertRaisesRegex(ValidationFailure, "digest mismatch"):
            verify_certification_receipt(receipt)
        receipt = self.build()
        receipt["apparatus_decision"]["eligible"] = False
        body = {key: value for key, value in receipt.items() if key != "receipt_digest"}
        receipt["receipt_digest"] = digest_json(body)
        with self.assertRaisesRegex(ValidationFailure, "apparatus decision"):
            verify_certification_receipt(receipt)

    def test_cross_receipt_digest_inconsistency_fails_closed(self) -> None:
        for field, nested, message in (
            (
                "evaluation_digest",
                ("outcome", "digest"),
                "outcome digest binding",
            ),
            (
                "participation_receipt_digest",
                ("participation", "receipt_digest"),
                "participation digest binding",
            ),
            (
                "execution_policy_receipt_digest",
                ("comparison", "receipt_digest"),
                "comparison digest binding",
            ),
        ):
            with self.subTest(field=field):
                receipt = self.build()
                receipt["apparatus"][field] = OTHER_DIGEST
                receipt["apparatus_decision"] = evaluate_apparatus(receipt["apparatus"])
                body = {key: value for key, value in receipt.items() if key != "receipt_digest"}
                receipt["receipt_digest"] = digest_json(body)
                with self.assertRaisesRegex(ValidationFailure, message):
                    verify_certification_receipt(receipt)

    def test_missing_system_and_policy_bindings_are_non_certifying(self) -> None:
        value = bindings()
        value["system_digest"] = None
        value["execution_policy_digest"] = None
        receipt = build_certification_receipt(
            bindings=value,
            apparatus=passing_apparatus(),
            outcome={"passed": True, "score": 1.0},
            participation=participation(),
            comparison=None,
        )
        self.assertIsNone(receipt["bindings"]["system_digest"])
        self.assertIsNone(receipt["bindings"]["execution_policy_digest"])
        self.assertFalse(receipt["binding_decision"]["eligible"])
        self.assertFalse(receipt["eligible"])


if __name__ == "__main__":
    unittest.main()
