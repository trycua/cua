from __future__ import annotations

import json
import shutil
import tempfile
from pathlib import Path
import unittest
from unittest import mock

from cua_bench_runtime.adapters.local import adapters as local_adapters
from cua_bench_runtime.canon import digest_file
from cua_bench_runtime.engine import run_trial
from cua_bench_runtime.errors import BudgetExceeded, UsageFailure, ValidationFailure
from cua_bench_runtime.explain import inspect_trial
from cua_bench_runtime.policy import ExecutionPolicyController, certification_integrity
from cua_bench_runtime.schemas import REPO_ROOT, SCHEMA_ROOT
from cua_bench_runtime.model import EnvironmentHandle


EXAMPLES = SCHEMA_ROOT / "examples"
SYSTEM_PATH = EXAMPLES / "system.cuabench.json"
POLICY_PATH = EXAMPLES / "execution-policy.cuabench.json"
TASK = REPO_ROOT / "conformance/tasks/synthetic-echo-v1/task.cuabench.json"
AGENT = REPO_ROOT / "conformance/agents/reference_ok.py"
HANGING_AGENT = REPO_ROOT / "conformance/agents/reference_hang.py"


class PolicyTests(unittest.TestCase):
    def test_debug_mode_is_never_comparison_or_certification_eligible(self) -> None:
        controller = self.controller()
        controller.record_debug_mode()
        receipt = controller.finalize(
            config_digest="sha256:" + "a" * 64,
            elapsed_ms=1,
            environment_facts=self.trusted_facts(),
        )

        self.assertFalse(receipt["eligible"])
        self.assertIn("debug_mode", receipt["violations"])
        self.assertEqual(
            certification_integrity(receipt),
            {"passed": False, "violations": ["debug_mode"]},
        )

    def test_certification_integrity_excludes_only_model_observability(self) -> None:
        telemetry_only = certification_integrity(
            {
                "violations": [
                    "model_telemetry_unavailable",
                    "model_telemetry_not_certifying",
                ]
            }
        )
        self.assertEqual(telemetry_only, {"passed": True, "violations": []})
        self.assertEqual(
            certification_integrity(
                {
                    "violations": [
                        "model_telemetry_unavailable",
                        "package_installation_observed",
                    ]
                }
            ),
            {"passed": False, "violations": ["package_installation_observed"]},
        )

    def controller(self, *, cost_basis: str | None = None) -> ExecutionPolicyController:
        policy = json.loads(POLICY_PATH.read_text(encoding="utf-8"))
        if cost_basis is not None:
            policy["accounting"]["cost_basis"] = cost_basis
        return ExecutionPolicyController(
            trial_id="policy-test",
            system=json.loads(SYSTEM_PATH.read_text(encoding="utf-8")),
            policy=policy,
            model_price_table=json.loads(
                (EXAMPLES / "freeze-inputs/model-price-table.json").read_text(encoding="utf-8")
            ),
            system_digest=digest_file(SYSTEM_PATH),
            policy_digest=digest_file(POLICY_PATH),
        )

    def proxy_controller(self) -> ExecutionPolicyController:
        policy = json.loads(POLICY_PATH.read_text(encoding="utf-8"))
        policy["network"] = {
            "mode": "allowlist",
            "allowlist_sha256": "1" * 64,
            "proxy_endpoint": "192.0.2.10@8443",
            "provider_allowlist_sha256": "2" * 64,
            "proxy_implementation_sha256": "a" * 64,
        }
        return ExecutionPolicyController(
            trial_id="policy-test",
            system=json.loads(SYSTEM_PATH.read_text(encoding="utf-8")),
            policy=policy,
            model_price_table=json.loads(
                (EXAMPLES / "freeze-inputs/model-price-table.json").read_text(encoding="utf-8")
            ),
            system_digest=digest_file(SYSTEM_PATH),
            policy_digest="sha256:" + "3" * 64,
        )

    @staticmethod
    def provider_proxy_evidence() -> dict:
        digest = "sha256:" + "a" * 64
        return {
            "enforcer": "host-cdb-connect-proxy",
            "schema_version": 1,
            "trial_id": "policy-test",
            "endpoint": "http://192.0.2.10:8443",
            "allowed_client_ip": "192.0.2.1",
            "provider_allowlist_sha256": "sha256:" + "2" * 64,
            "initial_client_binding_digest": digest,
            "sealed_client_binding_digest": digest,
            "initial_implementation_digest": digest,
            "sealed_implementation_digest": digest,
            "implementation_identity": "cb.provider-connect-proxy/v2",
            "initial_evidence_digest": digest,
            "sealed_evidence_digest": digest,
            "artifact_sha256": digest,
            "accepted_connections": 1,
            "rejected_connections": 2,
            "bytes_guest_to_provider": 100,
            "bytes_provider_to_guest": 200,
            "transcript_chain_digest": digest,
            "active": False,
            "sealed": True,
        }

    def trusted_facts(self) -> dict:
        policy = json.loads(POLICY_PATH.read_text(encoding="utf-8"))
        return {
            "fresh_harness_workspace": True,
            "target_reset": True,
            "cache_state": "empty",
            "persistent_state": "absent",
            "applied_network_mode": "full",
            "applied_network_allowlist_sha256": None,
            "applied_permission_policy_sha256": policy["permissions"]["policy_sha256"],
            "credential_state_profile_sha256": policy["credential_state_profile"]["sha256"],
            "applications": [{"id": "synthetic-note", "version": "1.0.0"}],
            "display": {"width_px": 1440, "height_px": 900, "scale": 2},
        }

    def record_primary(self, controller: ExecutionPolicyController, **changes) -> None:
        values = {
            "route_id": "route.primary",
            "role": "primary",
            "provider": "synthetic-provider",
            "model": "synthetic-model",
            "snapshot": "2026-08-01",
            "service_tier": "standard",
            "tokens": {"input": 100, "output": 20, "cache_read": 0, "cache_write": 0},
            "cost_usd": None,
            "trust": "certifying",
            "includes_subagents": True,
        }
        values.update(changes)
        controller.record_model_call(**values)

    def test_trusted_adapter_facts_produce_eligible_receipt(self) -> None:
        controller = self.controller()
        self.record_primary(controller)
        receipt = controller.finalize(
            config_digest="sha256:" + "a" * 64,
            elapsed_ms=1000,
            environment_facts=self.trusted_facts(),
        )
        self.assertEqual(receipt["status"], "passed")
        self.assertTrue(receipt["eligible"])
        self.assertEqual(receipt["violations"], [])

    def test_required_apparatus_enforcement_fails_closed(self) -> None:
        controller = self.controller()
        self.record_primary(controller)
        facts = self.trusted_facts()
        facts["apparatus_enforcement_required"] = True
        receipt = controller.finalize(
            config_digest="sha256:" + "a" * 64,
            elapsed_ms=1000,
            environment_facts=facts,
        )
        self.assertFalse(receipt["eligible"])
        self.assertIn("apparatus_enforcement_unverified", receipt["violations"])

    def test_bound_apparatus_enforcement_remains_eligible(self) -> None:
        controller = self.controller()
        self.record_primary(controller)
        facts = self.trusted_facts()
        facts.update(
            {
                "apparatus_enforcement_required": True,
                "enforcement": {
                    "network": {
                        "enforcer": "guest-root-pf-anchor",
                        "evidence_digest": "sha256:" + "1" * 64,
                    },
                    "human_input": {
                        "enforcer": "host-lume-vnc-disabled",
                        "evidence_digest": "sha256:" + "2" * 64,
                    },
                    "privileged_helper": {
                        "enforcer": "guest-root-cdb-helper",
                        "evidence_digest": "sha256:" + "3" * 64,
                    },
                    "lume_binary": {"sha256": "sha256:" + "4" * 64},
                },
            }
        )
        receipt = controller.finalize(
            config_digest="sha256:" + "a" * 64,
            elapsed_ms=1000,
            environment_facts=facts,
        )
        self.assertTrue(receipt["eligible"])
        self.assertEqual(receipt["violations"], [])

    def test_provider_proxy_is_policy_bound_and_rejections_are_allowed(self) -> None:
        controller = self.proxy_controller()
        self.record_primary(controller)
        facts = self.trusted_facts()
        facts.update(
            {
                "applied_network_mode": "allowlist",
                "applied_network_allowlist_sha256": "1" * 64,
                "apparatus_enforcement_required": True,
                "enforcement": {
                    "network": {
                        "enforcer": "guest-root-pf-anchor",
                        "evidence_digest": "sha256:" + "1" * 64,
                    },
                    "provider_proxy": self.provider_proxy_evidence(),
                    "human_input": {
                        "enforcer": "host-lume-vnc-disabled",
                        "evidence_digest": "sha256:" + "2" * 64,
                    },
                    "privileged_helper": {
                        "enforcer": "guest-root-cdb-helper",
                        "evidence_digest": "sha256:" + "3" * 64,
                    },
                    "lume_binary": {"sha256": "sha256:" + "4" * 64},
                },
            }
        )
        receipt = controller.finalize(
            config_digest="sha256:" + "a" * 64,
            elapsed_ms=1000,
            environment_facts=facts,
        )
        self.assertTrue(receipt["eligible"], receipt)

        facts["enforcement"]["provider_proxy"]["accepted_connections"] = 0
        controller = self.proxy_controller()
        self.record_primary(controller)
        receipt = controller.finalize(
            config_digest="sha256:" + "a" * 64,
            elapsed_ms=1000,
            environment_facts=facts,
        )
        self.assertIn(
            "provider_proxy_provider_connection_unverified",
            receipt["violations"],
        )

    def test_undeclared_fallback_and_intervention_are_ineligible(self) -> None:
        controller = self.controller()
        self.record_primary(controller, route_id="route.fallback", role="fallback")
        controller.record_human_intervention(approval_prompt=True)
        receipt = controller.finalize(
            config_digest="sha256:" + "b" * 64,
            elapsed_ms=1000,
            environment_facts=self.trusted_facts(),
        )
        self.assertFalse(receipt["eligible"])
        self.assertIn("undeclared_or_mismatched_model_route", receipt["violations"])
        self.assertIn("approval_prompt_observed", receipt["violations"])

    def test_cost_limit_is_runtime_owned_and_receipt_survives(self) -> None:
        controller = self.controller(cost_basis="provider_billed")
        with self.assertRaises(BudgetExceeded):
            self.record_primary(controller, cost_usd=11.0)
        receipt = controller.finalize(
            config_digest="sha256:" + "c" * 64,
            elapsed_ms=1000,
            environment_facts=self.trusted_facts(),
        )
        self.assertIn("cost_limit_exceeded", receipt["violations"])
        self.assertEqual(receipt["observed"]["cost_usd"], 11.0)

    def test_token_limit_is_runtime_owned(self) -> None:
        controller = self.controller()
        with self.assertRaises(BudgetExceeded):
            self.record_primary(
                controller,
                tokens={
                    "input": 100001,
                    "output": 0,
                    "cache_read": 0,
                    "cache_write": 0,
                },
            )
        receipt = controller.finalize(
            config_digest="sha256:" + "d" * 64,
            elapsed_ms=1000,
            environment_facts=self.trusted_facts(),
        )
        self.assertIn("token_limit_exceeded", receipt["violations"])

    def test_unavailable_usage_is_not_reported_as_observed_zero(self) -> None:
        controller = self.controller()
        self.record_primary(
            controller,
            tokens=None,
            cost_usd=None,
            trust="non_certifying",
        )
        receipt = controller.finalize(
            config_digest="sha256:" + "e" * 64,
            elapsed_ms=1000,
            environment_facts=self.trusted_facts(),
        )
        self.assertEqual(receipt["status"], "unavailable")
        self.assertFalse(receipt["eligible"])
        self.assertIn("model_telemetry_unavailable", receipt["violations"])
        self.assertIsNone(receipt["observed"]["tokens"])
        self.assertIsNone(receipt["observed"]["cost_usd"])
        self.assertIsNone(receipt["observed"]["model_calls"][0]["tokens"])

    def test_missing_subagent_scope_keeps_tokens_explicitly_unavailable(self) -> None:
        controller = self.controller()
        self.record_primary(controller, includes_subagents=None)
        receipt = controller.finalize(
            config_digest="sha256:" + "e" * 64,
            elapsed_ms=1000,
            environment_facts=self.trusted_facts(),
        )
        self.assertEqual(receipt["status"], "unavailable")
        self.assertFalse(receipt["eligible"])
        self.assertIsNone(receipt["observed"]["tokens"])

    def test_primary_only_totals_are_honest_and_comparison_ineligible(self) -> None:
        controller = self.controller()
        self.record_primary(controller, includes_subagents=False)
        receipt = controller.finalize(
            config_digest="sha256:" + "e" * 64,
            elapsed_ms=1000,
            environment_facts=self.trusted_facts(),
        )
        self.assertEqual(receipt["status"], "unavailable")
        self.assertFalse(receipt["eligible"])
        self.assertEqual(receipt["observed"]["tokens"]["input"], 100)
        self.assertIs(receipt["observed"]["tokens"]["includes_subagents"], False)

    def test_one_unknown_scope_makes_aggregated_tokens_unavailable(self) -> None:
        controller = self.controller()
        self.record_primary(controller, includes_subagents=True)
        self.record_primary(controller, includes_subagents=None)
        receipt = controller.finalize(
            config_digest="sha256:" + "e" * 64,
            elapsed_ms=1000,
            environment_facts=self.trusted_facts(),
        )
        self.assertFalse(receipt["eligible"])
        self.assertIsNone(receipt["observed"]["tokens"])

    def test_provider_billed_cost_unavailability_preserves_observed_tokens(self) -> None:
        controller = self.controller(cost_basis="provider_billed")
        self.record_primary(controller, cost_usd=None)
        receipt = controller.finalize(
            config_digest="sha256:" + "f" * 64,
            elapsed_ms=1000,
            environment_facts=self.trusted_facts(),
        )
        self.assertFalse(receipt["eligible"])
        self.assertEqual(receipt["observed"]["tokens"]["input"], 100)
        self.assertIsNone(receipt["observed"]["cost_usd"])

    def test_generic_subprocess_scores_but_fails_closed_for_comparison(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, trial, result = run_trial(
                task_path=TASK,
                agent_command=AGENT,
                out=Path(directory),
                trial_id="v03-unobserved",
                system_path=SYSTEM_PATH,
                execution_policy_path=POLICY_PATH,
            )
            report = inspect_trial(trial)
            system_copy = (trial / "inputs/system.cuabench.json").is_file()
            policy_copy = (trial / "inputs/execution-policy.cuabench.json").is_file()
        self.assertEqual(code, 0)
        self.assertTrue(result["evaluation"]["passed"])
        self.assertEqual(result["schema_version"], "0.3.0")
        self.assertEqual(result["execution_policy"]["status"], "unavailable")
        self.assertFalse(result["comparison_eligible"])
        self.assertFalse(result["certifying"])
        self.assertTrue(report["verified"])
        self.assertTrue(system_copy)
        self.assertTrue(policy_copy)

    def run_instrumented(
        self,
        root: Path,
        *,
        trial_id: str,
        intervention: bool = False,
        cost_usd: float | None = None,
        system_path: Path = SYSTEM_PATH,
        policy_path: Path = POLICY_PATH,
    ):
        facts = self.trusted_facts()

        def instrumented(*args, **kwargs):
            local_args = ("local", *args[1:])
            environment, harness, observer, evaluator = local_adapters(*local_args, **kwargs)

            class TrustedEnvironment:
                name = "protected-policy-test"

                def setup(self, context):
                    handle = environment.setup(context)
                    return EnvironmentHandle(
                        kind=self.name,
                        root=handle.root,
                        facts={**handle.facts, **facts},
                    )

                def cleanup(self, context, handle, timeout_seconds):
                    return environment.cleanup(context, handle, timeout_seconds)

            class InstrumentedHarness:
                name = "instrumented-test-harness"

                def run(self, context, handle, interrupt, timeout_seconds):
                    self_outer.record_primary(context.policy, cost_usd=cost_usd)
                    if intervention:
                        context.policy.record_human_intervention(approval_prompt=True)
                    return harness.run(context, handle, interrupt, timeout_seconds)

            self_outer = self
            return TrustedEnvironment(), InstrumentedHarness(), observer, evaluator

        with (
            mock.patch(
                "cua_bench_runtime.engine.CERTIFYING_ENVIRONMENT_ADAPTERS",
                frozenset({"protected-policy-test"}),
            ),
            mock.patch("cua_bench_runtime.engine.adapters", side_effect=instrumented),
        ):
            return run_trial(
                task_path=TASK,
                agent_command=AGENT,
                out=root,
                trial_id=trial_id,
                environment_name="protected-policy-test",
                system_path=system_path,
                execution_policy_path=policy_path,
            )

    def test_instrumented_provider_and_environment_certify(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, trial, result = self.run_instrumented(Path(directory), trial_id="v03-certified")
            report = inspect_trial(trial)
        self.assertEqual(code, 0)
        self.assertTrue(result["evaluation"]["passed"])
        self.assertTrue(result["comparison_eligible"])
        self.assertTrue(result["certifying"])
        self.assertTrue(report["verified"])

    def test_intervention_keeps_score_but_fails_comparison(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            code, _trial, result = self.run_instrumented(
                Path(directory), trial_id="v03-intervention", intervention=True
            )
        self.assertEqual(code, 0)
        self.assertTrue(result["evaluation"]["passed"])
        self.assertFalse(result["comparison_eligible"])
        self.assertIn(
            "human_intervention_exceeded",
            result["execution_policy"]["violations"],
        )

    def test_cost_termination_is_explainable_and_cleans_up(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            examples = root / "examples"
            shutil.copytree(EXAMPLES, examples)
            policy_path = examples / "execution-policy.cuabench.json"
            policy = json.loads(policy_path.read_text(encoding="utf-8"))
            policy["limits"]["cost_usd"] = 0.0001
            policy_path.write_text(json.dumps(policy), encoding="utf-8")
            code, trial, result = self.run_instrumented(
                root / "trials",
                trial_id="v03-cost-limit",
                system_path=examples / "system.cuabench.json",
                policy_path=policy_path,
            )
            report = inspect_trial(trial)
            cleanup_exists = (trial / "artifacts/cleanup.json").is_file()
        self.assertEqual(code, 7)
        self.assertEqual(result["status"], "cost_limit")
        self.assertIsNone(result["evaluation"])
        self.assertTrue(result["cleanup_ok"])
        self.assertTrue(cleanup_exists)
        self.assertTrue(report["verified"])

    def test_policy_wall_time_clamps_looser_caller_timeout(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            examples = root / "examples"
            shutil.copytree(EXAMPLES, examples)
            policy_path = examples / "execution-policy.cuabench.json"
            policy = json.loads(policy_path.read_text(encoding="utf-8"))
            policy["limits"]["wall_time_ms"] = 100
            policy_path.write_text(json.dumps(policy), encoding="utf-8")
            code, trial, result = run_trial(
                task_path=TASK,
                agent_command=HANGING_AGENT,
                out=root / "trials",
                trial_id="v03-policy-timeout",
                timeout_seconds=5.0,
                system_path=examples / "system.cuabench.json",
                execution_policy_path=policy_path,
            )
            config = json.loads((trial / "config.json").read_text(encoding="utf-8"))
            report = inspect_trial(trial)
        self.assertEqual(code, 4)
        self.assertEqual(result["status"], "timeout")
        self.assertEqual(config["limits"]["agent_seconds"], 0.1)
        self.assertTrue(result["cleanup_ok"])
        self.assertTrue(report["verified"])

    def test_malformed_policy_observer_fails_closed_without_losing_score(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch(
                "cua_bench_runtime.engine.ExecutionPolicyController.finalize",
                side_effect=ValueError("malformed protected evidence"),
            ):
                code, trial, result = run_trial(
                    task_path=TASK,
                    agent_command=AGENT,
                    out=Path(directory),
                    trial_id="v03-policy-observer-error",
                    system_path=SYSTEM_PATH,
                    execution_policy_path=POLICY_PATH,
                )
            report = inspect_trial(trial)
        self.assertEqual(code, 0)
        self.assertTrue(result["evaluation"]["passed"])
        self.assertEqual(result["execution_policy"]["status"], "unavailable")
        self.assertEqual(result["execution_policy"]["violations"], ["policy_observer_error"])
        self.assertTrue(result["cleanup_ok"])
        self.assertTrue(report["verified"])

    def test_explain_rejects_policy_receipt_tampering(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            _code, trial, result = self.run_instrumented(
                Path(directory), trial_id="v03-policy-tamper"
            )
            result["execution_policy"]["eligible"] = False
            (trial / "result.json").write_text(json.dumps(result), encoding="utf-8")
            with self.assertRaisesRegex(
                ValidationFailure, "execution policy receipt digest mismatch"
            ):
                inspect_trial(trial)

    def test_explain_rejects_input_manifest_rewrite(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            _code, trial, _result = self.run_instrumented(
                Path(directory), trial_id="v03-manifest-tamper"
            )
            manifest = trial / "inputs.manifest.json"
            manifest.chmod(0o644)
            manifest.write_bytes(manifest.read_bytes() + b"\n")
            with self.assertRaisesRegex(ValidationFailure, "input manifest digest"):
                inspect_trial(trial)

    def test_system_and_policy_must_be_supplied_together(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(UsageFailure, "supplied together"):
                run_trial(
                    task_path=TASK,
                    agent_command=AGENT,
                    out=Path(directory),
                    trial_id="v03-missing-policy",
                    system_path=SYSTEM_PATH,
                )


if __name__ == "__main__":
    unittest.main()
