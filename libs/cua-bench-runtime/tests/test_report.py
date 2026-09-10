from __future__ import annotations

import copy
import contextlib
import hashlib
import io
import json
import random
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.cli import main
from cua_bench_runtime.report import (
    build_report,
    build_report_preregistration,
    normalize_trials,
)
from cua_bench_runtime.receipt_signing import (
    key_id,
    sign_certification_receipt,
    sign_report_preregistration,
)
from cua_bench_runtime.schemas import SCHEMA_ROOT


ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = SCHEMA_ROOT / "examples"


def row(
    arm: str,
    task: str,
    attempt: int,
    passed: bool,
    *,
    policy: str = "policy-a",
    image: str = "image-a",
    freeze: str = "freeze-a",
    price: str = "price-a",
) -> dict[str, object]:
    return {
        "trial_id": f"trial-{arm}-{task}-{attempt}",
        "task_id": task,
        "task_digest": f"digest-{task}",
        "variant": "fixture-a",
        "dataset_freeze_digest": freeze,
        "pairing_key": f"{task}-attempt-{attempt}",
        "attempt_index": attempt,
        "system_digest": arm,
        "execution_policy_digest": policy,
        "resolved_seed_provenance_sha256": image,
        "apparatus_digest": "apparatus-a",
        "harness_digest": f"harness-{arm}",
        "model_routing_digest": "model-a",
        "driver_candidate_digest": "driver-a",
        "driver_profile": "native-bundle@1.0.0",
        "driver_tool_contract_digest": "tools-a",
        "driver_skill_mode": "bundled",
        "eligible": True,
        "certified": True,
        "passed": passed,
        "termination_status": "completed",
        "infrastructure_failure": False,
        "wall_time_ms": 1000 + attempt,
        "tokens": 100 + attempt,
        "cost_usd": 0.01 + attempt / 1000,
        "price_table_digest": price,
        "required_attempts_per_task": 2,
        "infrastructure_retries": 1,
    }


class ReportTests(unittest.TestCase):
    @staticmethod
    def signing_keys(root: Path) -> tuple[Path, Path]:
        private = root / "signing-key"
        public = root / "signing-key.pub"
        ssh_keygen = next(
            (
                candidate
                for candidate in (
                    Path("/usr/bin/ssh-keygen"),
                    Path(r"C:\Windows\System32\OpenSSH\ssh-keygen.exe"),
                )
                if candidate.is_file()
            ),
            None,
        )
        if ssh_keygen is None:
            raise unittest.SkipTest("pinned system ssh-keygen is unavailable")
        subprocess.run(
            [
                str(ssh_keygen),
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-f",
                str(private),
            ],
            check=True,
        )
        return private, public

    def rows(self) -> list[dict[str, object]]:
        outcomes = {
            "system-a": {"task-a": [True, False], "task-b": [True, False]},
            "system-b": {"task-a": [True, True], "task-b": [True, False]},
        }
        return [
            row(arm, task, attempt, passed)
            for arm, tasks in outcomes.items()
            for task, attempts in tasks.items()
            for attempt, passed in enumerate(attempts)
        ]

    def preregistration_fixture(
        self,
        root: Path,
        *,
        attempts: int = 1,
        apparatus_digests: tuple[str, str] | None = None,
    ) -> tuple[list[Path], list[Path], Path]:
        base_system = json.loads((EXAMPLES / "system.cuabench.json").read_text(encoding="utf-8"))
        base_trial = json.loads(
            (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
        )
        policy = json.loads(
            (EXAMPLES / "execution-policy.cuabench.json").read_text(encoding="utf-8")
        )
        policy["limits"]["attempts_per_task"] = attempts
        policy_path = root / "execution-policy.cuabench.json"
        policy_bytes = (json.dumps(policy, indent=2, sort_keys=True) + "\n").encode()
        policy_path.write_bytes(policy_bytes)
        policy_digest = hashlib.sha256(policy_bytes).hexdigest()

        shared_apparatus = base_trial["bindings"]["apparatus_digest"]
        apparatus = apparatus_digests or (shared_apparatus, shared_apparatus)
        systems: list[tuple[Path, dict[str, object], str]] = []
        for system_index in range(2):
            system = copy.deepcopy(base_system)
            system["id"] = f"system.synthetic.preregister-{system_index}"
            system["harness"]["id"] = f"synthetic-harness-{system_index}"
            system_path = root / f"system.{system_index}.cuabench.json"
            system_bytes = (json.dumps(system, indent=2, sort_keys=True) + "\n").encode()
            system_path.write_bytes(system_bytes)
            systems.append((system_path, system, hashlib.sha256(system_bytes).hexdigest()))

        trials = []
        for attempt in range(attempts):
            for system_index, (_path, system, system_digest) in enumerate(systems):
                trial = copy.deepcopy(base_trial)
                trial["id"] = f"trial.synthetic.preregister-{system_index}.{attempt}"
                trial["pairing_key"] = f"synthetic-task.attempt-{attempt}"
                trial["attempt_index"] = attempt
                trial["system"] = {
                    "id": system["id"],
                    "version": system["version"],
                    "manifest_sha256": system_digest,
                }
                trial["execution_policy"] = {
                    "id": policy["id"],
                    "version": policy["version"],
                    "manifest_sha256": policy_digest,
                }
                trial["bindings"]["system_digest"] = system_digest
                trial["bindings"]["execution_policy_digest"] = policy_digest
                trial["bindings"]["apparatus_digest"] = apparatus[system_index]
                certification = trial["execution"]["certification"]
                certification["bindings"].update(
                    {
                        "trial_id": trial["id"],
                        "system_digest": system_digest,
                        "execution_policy_digest": policy_digest,
                        "apparatus_digest": apparatus[system_index],
                    }
                )
                trial_path = root / f"trial.{system_index}.{attempt}.json"
                trial_path.write_text(json.dumps(trial), encoding="utf-8")
                trials.append(trial_path)
        return trials, [item[0] for item in systems], policy_path

    def test_preregistration_freezes_a_complete_unconfounded_plan(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trials, systems, policy = self.preregistration_fixture(root, attempts=2)
            document = build_report_preregistration(trials, systems, [policy], "system-track")

        self.assertEqual(document["view"], "system-track")
        self.assertEqual(document["varying_factor"], "system_digest")
        self.assertEqual(document["required_attempts_per_task"], 2)
        self.assertEqual(len(document["arms"]), 2)
        self.assertEqual(len(document["pairing_keys"]), 2)
        self.assertEqual(len(document["trial_templates"]), 4)
        self.assertTrue(document["digest"].startswith("sha256:"))

    def test_preregistration_rejects_per_system_apparatus_digests(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trials, systems, policy = self.preregistration_fixture(
                root,
                apparatus_digests=("a" * 64, "b" * 64),
            )
            with self.assertRaisesRegex(
                ValidationFailure,
                "fixed factor apparatus_digest has 2 values",
            ):
                build_report_preregistration(trials, systems, [policy], "system-track")

    def test_system_track_preregistration_allows_one_descriptive_arm(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trials, systems, policy = self.preregistration_fixture(root, attempts=2)
            single_system = systems[:1]
            single_digest = hashlib.sha256(single_system[0].read_bytes()).hexdigest()
            single_trials = []
            for path in trials:
                trial = json.loads(path.read_text(encoding="utf-8"))
                if trial["system"]["manifest_sha256"] == single_digest:
                    single_trials.append(path)
            document = build_report_preregistration(
                single_trials, single_system, [policy], "system-track"
            )

        self.assertEqual(document["arms"], [single_digest])
        self.assertEqual(len(document["trial_plan"]), 2)

    def test_preregister_report_cli_writes_once(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trials, systems, policy = self.preregistration_fixture(root)
            output = root / "report-preregistration.json"
            arguments = [
                "preregister-report",
                "--view",
                "system-track",
                "--execution-policy",
                str(policy),
                "--signing-key",
                str(self.signing_keys(root)[0]),
                "--verifier-key",
                str(root / "signing-key.pub"),
                "--out",
                str(output),
            ]
            for path in trials:
                arguments.extend(("--trial-template", str(path)))
            for path in systems:
                arguments.extend(("--system", str(path)))
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(main(arguments), 0)
            document = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(document["body"]["receipt"]["view"], "system-track")
            with (
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertNotEqual(main(arguments), 0)

    def test_known_task_macro_pass_k_and_paired_delta(self) -> None:
        report = build_report(self.rows(), "system-track", bootstrap_samples=500, seed=17, pass_k=2)
        arms = {item["arm"]: item for item in report["arms"]}
        self.assertEqual(arms["system-a"]["task_macro_success"], 0.5)
        self.assertEqual(arms["system-b"]["task_macro_success"], 0.75)
        self.assertEqual(arms["system-a"]["pass_k"]["task_macro"], 0.25)
        self.assertEqual(arms["system-b"]["pass_k"]["task_macro"], 0.625)
        self.assertEqual(arms["system-a"]["rank_range"], {"best": 1, "worst": 2})
        self.assertEqual(arms["system-b"]["rank_range"], {"best": 1, "worst": 2})
        self.assertEqual(report["paired_deltas"], [])
        self.assertTrue(report["decision_bearing"])

        harness_rows = copy.deepcopy(self.rows())
        paired = build_report(
            harness_rows,
            "harness-comparison",
            bootstrap_samples=500,
            seed=17,
        )
        self.assertEqual(paired["paired_deltas"][0]["delta"], 0.25)
        self.assertEqual(paired["paired_deltas"][0]["pair_count"], 4)

    def test_degenerate_bootstrap_intervals_are_exact(self) -> None:
        rows = self.rows()
        for item in rows:
            item["passed"] = item["system_digest"] == "system-b"
        report = build_report(rows, "harness-comparison", bootstrap_samples=50, seed=11)
        arms = {item["arm"]: item for item in report["arms"]}
        self.assertEqual(arms["harness-system-a"]["confidence_interval"], [0.0, 0.0])
        self.assertEqual(arms["harness-system-b"]["confidence_interval"], [1.0, 1.0])
        self.assertEqual(report["paired_deltas"][0]["confidence_interval"], [1.0, 1.0])

    def test_task_macro_does_not_overweight_tasks_with_more_attempts(self) -> None:
        rows = []
        for arm in ("system-a", "system-b"):
            rows.append(row(arm, "task-a", 0, True))
            rows.extend(row(arm, "task-b", attempt, False) for attempt in range(3))
        for item in rows:
            item["required_attempts_per_task"] = 1
        report = build_report(rows, "system-track", bootstrap_samples=20)
        self.assertEqual(report["arms"][0]["task_macro_success"], 0.5)

    def test_shuffle_does_not_change_report(self) -> None:
        rows = self.rows()
        expected = build_report(rows, "system-track", bootstrap_samples=200, seed=3)
        random.Random(99).shuffle(rows)
        actual = build_report(rows, "system-track", bootstrap_samples=200, seed=3)
        self.assertEqual(actual, expected)

    def test_each_registered_view_accepts_its_varying_factor(self) -> None:
        cases = {
            "system-track": None,
            "harness-comparison": None,
            "model-comparison": "model_routing_digest",
            "driver-profile": "driver_profile",
            "tool-surface": "driver_tool_contract_digest",
        }
        for view, varying in cases.items():
            with self.subTest(view=view):
                rows = self.rows()
                if view != "harness-comparison":
                    for item in rows:
                        item["harness_digest"] = "harness-fixed"
                if varying:
                    for item in rows:
                        item[varying] = f"{varying}-{item['system_digest']}"
                report = build_report(rows, view, bootstrap_samples=10)
                self.assertEqual(report["view"], view)

    def test_confounded_comparison_names_exact_factor(self) -> None:
        rows = self.rows()
        rows[-1]["execution_policy_digest"] = "policy-b"
        with self.assertRaisesRegex(
            ValidationFailure,
            "fixed factor execution_policy_digest has 2 values",
        ):
            build_report(rows, "system-track", bootstrap_samples=10)

    def test_mixed_image_freeze_and_price_table_are_rejected(self) -> None:
        for factor in (
            "resolved_seed_provenance_sha256",
            "dataset_freeze_digest",
            "price_table_digest",
        ):
            with self.subTest(factor=factor):
                rows = self.rows()
                rows[-1][factor] = "different"
                with self.assertRaisesRegex(ValidationFailure, factor):
                    build_report(rows, "system-track", bootstrap_samples=10)

    def test_task_freeze_change_between_attempts_is_rejected(self) -> None:
        rows = self.rows()
        for item in rows:
            if item["task_id"] == "task-b" and item["attempt_index"] == 1:
                item["task_digest"] = "changed-task-freeze"
        with self.assertRaisesRegex(ValidationFailure, "task_digest"):
            build_report(rows, "system-track", bootstrap_samples=10)

    def test_unpaired_arm_is_rejected(self) -> None:
        rows = self.rows()[:-1]
        with self.assertRaisesRegex(ValidationFailure, "pairing_key task-b-attempt-1"):
            build_report(rows, "harness-comparison", bootstrap_samples=10)

    def test_missingness_and_infrastructure_are_explicit(self) -> None:
        rows = self.rows()
        rows[0]["eligible"] = False
        rows[1]["infrastructure_failure"] = True
        rows[2]["certified"] = False
        report = build_report(rows, "system-track", bootstrap_samples=10)
        self.assertEqual(report["missingness"]["ineligible"], 1)
        self.assertEqual(report["missingness"]["infrastructure_failure"], 1)
        self.assertEqual(report["missingness"]["uncertified_outcome"], 1)
        self.assertEqual(report["missingness"]["scored_trials"], 5)
        self.assertEqual(report["missingness"]["paired_infrastructure_exclusion"], 1)
        self.assertFalse(report["decision_bearing"])
        self.assertIn(
            "infrastructure_failure_rate_exceeded",
            report["decision_bearing_violations"],
        )

    def test_repository_example_normalizes_with_bound_system(self) -> None:
        rows = normalize_trials(
            [EXAMPLES / "trial.system-track.cuabench.json"],
            [EXAMPLES / "system.cuabench.json"],
            [EXAMPLES / "execution-policy.cuabench.json"],
        )
        self.assertEqual(rows[0]["system_id"], "system.synthetic.full")
        self.assertEqual(rows[0]["harness_id"], "synthetic-harness")
        self.assertIsNone(rows[0]["tokens"])
        self.assertEqual(rows[0]["required_attempts_per_task"], 5)
        self.assertEqual(rows[0]["infrastructure_retries"], 1)
        self.assertFalse(rows[0]["certified"])
        self.assertEqual(rows[0]["comparison_eligibility_status"], "ineligible")

    def test_executed_ineligible_trial_builds_descriptive_system_report(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copytree(EXAMPLES / "freeze-inputs", root / "freeze-inputs")
            trial = json.loads(
                (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
            )
            trial["execution"]["comparison_eligibility"] = {
                "status": "ineligible",
                "reasons": ["model_telemetry_unavailable"],
            }
            observed = trial["execution"]["observed"]
            observed["tokens"]["includes_subagents"] = False
            trial["observables"]["tokens"] = None
            trial["observables"]["cost_usd"] = None
            trial_path = root / "trial.cuabench.json"
            trial_path.write_text(json.dumps(trial), encoding="utf-8")

            normalized = normalize_trials(
                [trial_path],
                [EXAMPLES / "system.cuabench.json"],
                [EXAMPLES / "execution-policy.cuabench.json"],
            )[0]

        self.assertFalse(normalized["eligible"])
        self.assertEqual(normalized["eligibility_status"], "eligible")
        self.assertEqual(normalized["comparison_eligibility_status"], "ineligible")
        self.assertEqual(normalized["execution_status"], "executed")
        self.assertFalse(normalized["certified"])
        self.assertEqual(normalized["apparatus_status"], "passed")
        self.assertEqual(
            normalized["apparatus_receipt_sha256"],
            trial["execution"]["certification"]["receipt"]["sha256"],
        )
        self.assertIsNone(normalized["tokens"])
        self.assertIsNone(normalized["cost_usd"])

        report = build_report([normalized], "system-track", bootstrap_samples=10)
        self.assertEqual(report["report_type"], "descriptive")
        self.assertFalse(report["decision_bearing"])
        self.assertIn(
            "no_comparison_eligible_trials",
            report["decision_bearing_violations"],
        )
        self.assertEqual(report["missingness"]["scored_trials"], 0)
        self.assertEqual(report["missingness"]["descriptive_trials"], 1)
        self.assertEqual(report["arms"][0]["tokens"], {"median": None, "sample_count": 0})
        self.assertEqual(
            report["arms"][0]["cost_usd"],
            {"median": None, "sample_count": 0},
        )
        self.assertNotIn("rank_range", report["arms"][0])

    def test_single_eligible_system_arm_builds_descriptive_report(self) -> None:
        rows = [item for item in self.rows() if item["system_digest"] == "system-a"]

        report = build_report(rows, "system-track", bootstrap_samples=10)

        self.assertEqual(report["report_type"], "descriptive")
        self.assertFalse(report["decision_bearing"])
        self.assertEqual(
            report["decision_bearing_violations"],
            ["single_arm_descriptive_only"],
        )
        self.assertEqual(report["missingness"]["scored_trials"], len(rows))
        self.assertEqual(report["arms"][0]["trial_count"], len(rows))
        self.assertNotIn("rank_range", report["arms"][0])

    def test_comparison_eligible_trial_rejects_partial_token_scope(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copytree(EXAMPLES / "freeze-inputs", root / "freeze-inputs")
            path = root / "trial.cuabench.json"
            trial = json.loads(
                (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
            )
            trial["execution"]["observed"]["tokens"]["includes_subagents"] = False
            trial["execution"]["comparison_eligibility"] = {
                "status": "eligible",
                "reasons": [],
            }
            trial["observables"]["tokens"]["includes_subagents"] = False
            path.write_text(json.dumps(trial), encoding="utf-8")

            with self.assertRaisesRegex(ValidationFailure, "omit subagents"):
                normalize_trials(
                    [path],
                    [EXAMPLES / "system.cuabench.json"],
                    [EXAMPLES / "execution-policy.cuabench.json"],
                )

    def test_controlled_view_rejects_only_comparison_ineligible_outcomes(self) -> None:
        rows = self.rows()
        for item in rows:
            item["eligible"] = False
            item["tokens"] = None
            item["cost_usd"] = None
        with self.assertRaisesRegex(ValidationFailure, "no eligible scored trials"):
            build_report(rows, "harness-comparison", bootstrap_samples=10)

    def test_final_certification_not_participation_controls_reporting(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trial.cuabench.json"
            trial = json.loads(
                (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
            )
            trial["execution"]["certification"]["apparatus_status"] = "passed"
            trial["execution"]["certification"]["certifying"] = False
            trial["execution"]["certification"]["reasons"] = ["post_run_drift_detected"]
            for field in ("receipt", "signature", "verifier_key_id"):
                trial["execution"]["certification"].pop(field)
            path.write_text(json.dumps(trial), encoding="utf-8")
            rows = normalize_trials(
                [path],
                [EXAMPLES / "system.cuabench.json"],
                [EXAMPLES / "execution-policy.cuabench.json"],
            )
            self.assertFalse(rows[0]["certified"])

    def test_certification_declaration_must_match_attached_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "trial.cuabench.json"
            shutil.copytree(EXAMPLES / "freeze-inputs", root / "freeze-inputs")
            trial = json.loads(
                (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
            )
            trial["execution"]["certification"]["certifying"] = False
            trial["execution"]["certification"]["apparatus_status"] = "failed"
            trial["execution"]["certification"]["reasons"] = ["synthetic downgrade"]
            path.write_text(json.dumps(trial), encoding="utf-8")
            with self.assertRaisesRegex(
                ValidationFailure,
                "contradicts attached receipt",
            ):
                normalize_trials(
                    [path],
                    [EXAMPLES / "system.cuabench.json"],
                    [EXAMPLES / "execution-policy.cuabench.json"],
                )

    def test_trial_without_final_certification_remains_reportable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trial.cuabench.json"
            trial = json.loads(
                (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
            )
            trial["execution"].pop("certification")
            path.write_text(json.dumps(trial), encoding="utf-8")
            rows = normalize_trials(
                [path],
                [EXAMPLES / "system.cuabench.json"],
                [EXAMPLES / "execution-policy.cuabench.json"],
            )
            self.assertFalse(rows[0]["certified"])

    def test_certification_binding_mismatch_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trial.cuabench.json"
            trial = json.loads(
                (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
            )
            trial["execution"]["certification"]["bindings"]["task_digest"] = "0" * 64
            path.write_text(json.dumps(trial), encoding="utf-8")
            with self.assertRaisesRegex(
                ValidationFailure, "apparatus certification bindings mismatch"
            ):
                normalize_trials(
                    [path],
                    [EXAMPLES / "system.cuabench.json"],
                    [EXAMPLES / "execution-policy.cuabench.json"],
                )

    def test_certifying_trial_requires_a_valid_pinned_signature(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            private_key, public_key = self.signing_keys(root)
            self.assertTrue(private_key.is_file())
            with self.assertRaisesRegex(
                ValidationFailure,
                "signed receipt artifact|signature",
            ):
                normalize_trials(
                    [EXAMPLES / "trial.system-track.cuabench.json"],
                    [EXAMPLES / "system.cuabench.json"],
                    [EXAMPLES / "execution-policy.cuabench.json"],
                    certification_verifier_key=public_key,
                )

    def test_signed_certification_receipt_is_bound_to_trial_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copytree(EXAMPLES / "freeze-inputs", root / "freeze-inputs")
            private_key, public_key = self.signing_keys(root)
            trial = json.loads(
                (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
            )
            receipt = {
                "schema_version": 1,
                "eligible": True,
                "bindings": {
                    "trial_id": trial["id"],
                    "task_digest": "sha256:" + trial["task"]["manifest_sha256"],
                    "system_digest": "sha256:" + "0" * 64,
                    "execution_policy_digest": (
                        "sha256:" + trial["execution_policy"]["manifest_sha256"]
                    ),
                    "config_digest": "sha256:" + "2" * 64,
                    "inputs_manifest_digest": "sha256:" + "3" * 64,
                    "seed_provenance_digest": (
                        "sha256:" + trial["environment"]["resolved_seed_provenance_sha256"]
                    ),
                    "agent_digest": "sha256:" + "5" * 64,
                    "evaluator_digest": "sha256:" + "6" * 64,
                },
                "apparatus_decision": {"status": "passed"},
            }
            receipt_bytes = (
                json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n"
            ).encode()
            (root / "freeze-inputs/certification-receipt.json").write_bytes(receipt_bytes)
            artifact = sign_certification_receipt(
                receipt,
                bindings=receipt["bindings"],
                private_key=private_key,
            )
            signature_bytes = (
                json.dumps(artifact, sort_keys=True, separators=(",", ":")) + "\n"
            ).encode()
            (root / "freeze-inputs/certification-signature.json").write_bytes(signature_bytes)
            certification = trial["execution"]["certification"]
            certification["receipt"] = {
                "path": "freeze-inputs/certification-receipt.json",
                "sha256": hashlib.sha256(receipt_bytes).hexdigest(),
            }
            certification["signature"] = {
                "path": "freeze-inputs/certification-signature.json",
                "sha256": hashlib.sha256(signature_bytes).hexdigest(),
            }
            certification["verifier_key_id"] = key_id(public_key)
            trial_path = root / "trial.cuabench.json"
            trial_path.write_text(json.dumps(trial), encoding="utf-8")

            with self.assertRaisesRegex(
                ValidationFailure,
                "signed receipt bindings mismatch",
            ):
                normalize_trials(
                    [trial_path],
                    [EXAMPLES / "system.cuabench.json"],
                    [EXAMPLES / "execution-policy.cuabench.json"],
                    certification_verifier_key=public_key,
                )

    def test_apparatus_check_is_rejected_from_reports(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "apparatus-check.cuabench.json"
            system_path = root / "apparatus-system.cuabench.json"
            system = json.loads((EXAMPLES / "system.cuabench.json").read_text(encoding="utf-8"))
            system["id"] = "apparatus.synthetic-reference"
            system_path.write_text(json.dumps(system), encoding="utf-8")
            system_digest = hashlib.sha256(system_path.read_bytes()).hexdigest()
            trial = json.loads(
                (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
            )
            trial["system"]["id"] = system["id"]
            trial["system"]["manifest_sha256"] = system_digest
            trial["bindings"]["system_digest"] = system_digest
            path.write_text(json.dumps(trial), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "apparatus-check systems"):
                normalize_trials(
                    [path],
                    [system_path],
                    [EXAMPLES / "execution-policy.cuabench.json"],
                )

    def test_normalization_requires_exact_system_digest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "system.cuabench.json"
            system = json.loads((EXAMPLES / "system.cuabench.json").read_text(encoding="utf-8"))
            system["label"] = "Different bytes"
            path.write_text(json.dumps(system), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "missing system manifest"):
                normalize_trials(
                    [EXAMPLES / "trial.system-track.cuabench.json"],
                    [path],
                    [EXAMPLES / "execution-policy.cuabench.json"],
                )

    def test_cli_writes_report_and_normalized_table(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copytree(EXAMPLES / "freeze-inputs", root / "freeze-inputs")
            base_system = json.loads(
                (EXAMPLES / "system.cuabench.json").read_text(encoding="utf-8")
            )
            base_trial = json.loads(
                (EXAMPLES / "trial.system-track.cuabench.json").read_text(encoding="utf-8")
            )
            systems = []
            trials = []
            private_key, public_key = self.signing_keys(root)
            policy = json.loads(
                (EXAMPLES / "execution-policy.cuabench.json").read_text(encoding="utf-8")
            )
            policy["limits"]["attempts_per_task"] = 1
            policy_path = root / "execution-policy.cuabench.json"
            policy_bytes = (json.dumps(policy, indent=2, sort_keys=True) + "\n").encode()
            policy_path.write_bytes(policy_bytes)
            policy_digest = hashlib.sha256(policy_bytes).hexdigest()
            for index in range(2):
                system = copy.deepcopy(base_system)
                system["id"] = f"system.synthetic.harness-{index}"
                system["harness"]["id"] = f"synthetic-harness-{index}"
                system["harness"]["version"] = f"1.0.{index}"
                system_path = root / f"system.{index}.cuabench.json"
                system_bytes = (json.dumps(system, indent=2, sort_keys=True) + "\n").encode()
                system_path.write_bytes(system_bytes)
                system_digest = hashlib.sha256(system_bytes).hexdigest()
                systems.append(system_path)

                trial = copy.deepcopy(base_trial)
                trial["id"] = f"trial.synthetic.harness-{index}.0"
                trial["system"] = {
                    "id": system["id"],
                    "version": system["version"],
                    "manifest_sha256": system_digest,
                }
                trial["bindings"]["system_digest"] = system_digest
                trial["execution_policy"] = {
                    "id": policy["id"],
                    "version": policy["version"],
                    "manifest_sha256": policy_digest,
                }
                trial["bindings"]["execution_policy_digest"] = policy_digest
                trial["execution"]["certification"]["bindings"]["trial_id"] = trial["id"]
                trial["execution"]["certification"]["bindings"]["system_digest"] = system_digest
                trial["execution"]["certification"]["bindings"]["execution_policy_digest"] = (
                    policy_digest
                )
                receipt = json.loads(
                    (EXAMPLES / "freeze-inputs/apparatus-certification-receipt.json").read_text(
                        encoding="utf-8"
                    )
                )
                receipt["bindings"]["trial_id"] = trial["id"]
                receipt["bindings"] = {
                    "trial_id": trial["id"],
                    "task_digest": ("sha256:" + trial["task"]["manifest_sha256"]),
                    "system_digest": "sha256:" + system_digest,
                    "execution_policy_digest": "sha256:"
                    + trial["execution_policy"]["manifest_sha256"],
                    "config_digest": "sha256:" + "2" * 64,
                    "inputs_manifest_digest": "sha256:" + "3" * 64,
                    "seed_provenance_digest": (
                        "sha256:" + trial["environment"]["resolved_seed_provenance_sha256"]
                    ),
                    "agent_digest": "sha256:" + "5" * 64,
                    "evaluator_digest": "sha256:" + "6" * 64,
                }
                receipt["outcome"] = {
                    "available": True,
                    "passed": trial["outcomes"]["completion"],
                    "score": 1 if trial["outcomes"]["completion"] else 0,
                    "digest": "sha256:" + "7" * 64,
                }
                receipt_bytes = (
                    json.dumps(receipt, sort_keys=True, separators=(",", ":")) + "\n"
                ).encode()
                receipt_name = f"apparatus-certification-receipt-{index}.json"
                (root / "freeze-inputs" / receipt_name).write_bytes(receipt_bytes)
                trial["execution"]["certification"]["receipt"] = {
                    "path": f"freeze-inputs/{receipt_name}",
                    "sha256": hashlib.sha256(receipt_bytes).hexdigest(),
                }
                artifact = sign_certification_receipt(
                    receipt,
                    bindings=receipt["bindings"],
                    private_key=private_key,
                )
                signature_name = f"apparatus-certification-signature-{index}.json"
                signature_bytes = (
                    json.dumps(artifact, sort_keys=True, separators=(",", ":")) + "\n"
                ).encode()
                (root / "freeze-inputs" / signature_name).write_bytes(signature_bytes)
                trial["execution"]["certification"]["signature"] = {
                    "path": f"freeze-inputs/{signature_name}",
                    "sha256": hashlib.sha256(signature_bytes).hexdigest(),
                }
                trial["execution"]["certification"]["verifier_key_id"] = key_id(public_key)
                trial["execution"]["comparison_eligibility"] = {
                    "status": "eligible",
                    "reasons": [],
                }
                trial_path = root / f"trial.{index}.cuabench.json"
                trial_path.write_text(json.dumps(trial), encoding="utf-8")
                trials.append(trial_path)

            tampered_trial = json.loads(trials[0].read_text(encoding="utf-8"))
            tampered_trial["outcomes"]["completion"] = not tampered_trial["outcomes"]["completion"]
            tampered_path = root / "trial.tampered-outcome.cuabench.json"
            tampered_path.write_text(json.dumps(tampered_trial), encoding="utf-8")
            with self.assertRaisesRegex(
                ValidationFailure,
                "signed apparatus certification outcome mismatch",
            ):
                normalize_trials(
                    [tampered_path],
                    systems,
                    [policy_path],
                    certification_verifier_key=public_key,
                )

            tampered_termination = json.loads(trials[0].read_text(encoding="utf-8"))
            tampered_termination["termination_status"] = "timeout"
            tampered_termination_path = root / "trial.tampered-termination.cuabench.json"
            tampered_termination_path.write_text(json.dumps(tampered_termination), encoding="utf-8")
            with self.assertRaisesRegex(
                ValidationFailure,
                "certifying apparatus requires completed termination",
            ):
                normalize_trials(
                    [tampered_termination_path],
                    systems,
                    [policy_path],
                    certification_verifier_key=public_key,
                )

            forged_pass = dict(tampered_termination)
            forged_pass["outcomes"] = dict(tampered_termination["outcomes"])
            forged_pass["outcomes"]["completion"] = not forged_pass["outcomes"]["completion"]
            forged_pass_path = root / "trial.forged-timeout-pass.cuabench.json"
            forged_pass_path.write_text(json.dumps(forged_pass), encoding="utf-8")
            with self.assertRaisesRegex(
                ValidationFailure,
                "certifying apparatus requires completed termination",
            ):
                normalize_trials(
                    [forged_pass_path],
                    systems,
                    [policy_path],
                    certification_verifier_key=public_key,
                )

            report_path = root / "report.json"
            normalized_path = root / "trials.jsonl"
            preregistration = build_report_preregistration(
                trials,
                systems,
                [policy_path],
                "harness-comparison",
                bootstrap_samples=10,
            )
            signed_preregistration = sign_report_preregistration(
                preregistration, private_key=private_key
            )
            preregistration_path = root / "preregistration.json"
            preregistration_path.write_text(json.dumps(signed_preregistration), encoding="utf-8")
            arguments = [
                "report",
                "--view",
                "harness-comparison",
                "--out",
                str(report_path),
                "--normalized-out",
                str(normalized_path),
                "--bootstrap-samples",
                "10",
                "--preregistration",
                str(preregistration_path),
                "--verifier-key",
                str(public_key),
                "--execution-policy",
                str(policy_path),
            ]
            for path in trials:
                arguments.extend(("--trial", str(path)))
            for path in systems:
                arguments.extend(("--system", str(path)))
            with contextlib.redirect_stdout(io.StringIO()):
                code = main(arguments)

            self.assertEqual(code, 0)
            report = json.loads(report_path.read_text(encoding="utf-8"))
            self.assertEqual(report["view"], "harness-comparison")
            self.assertEqual(report["preregistration_digest"], preregistration["digest"])
            self.assertEqual(len(normalized_path.read_text().splitlines()), 2)

            mismatched = list(arguments)
            mismatched.extend(("--seed", "9"))
            with (
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertNotEqual(main(mismatched), 0)

            changed_trial = json.loads(trials[0].read_text(encoding="utf-8"))
            changed_trial["bindings"]["apparatus_digest"] = "9" * 64
            changed_trial["execution"]["certification"]["bindings"]["apparatus_digest"] = "9" * 64
            trials[0].write_text(json.dumps(changed_trial), encoding="utf-8")
            changed = list(arguments)
            changed[changed.index(str(report_path))] = str(root / "changed-report.json")
            changed[changed.index(str(normalized_path))] = str(root / "changed-trials.jsonl")
            with (
                contextlib.redirect_stdout(io.StringIO()),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertNotEqual(main(changed), 0)


if __name__ == "__main__":
    unittest.main()
