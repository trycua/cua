from __future__ import annotations

import json
import shutil
import tempfile
import unittest
from pathlib import Path
from types import MethodType
from unittest import mock

import cua_bench_runtime.export_trial as exporter
from cua_bench_runtime.canon import digest_file
from cua_bench_runtime.export_trial import export_trial
from cua_bench_runtime.explain import inspect_trial
from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.report import build_report, normalize_trials
import test_policy


class ExportTrialTests(unittest.TestCase):
    def test_debug_trial_cannot_be_exported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial = root / "trial"
            trial.mkdir()
            (trial / "config.json").write_text(
                json.dumps({"schema_version": "0.3.0", "debug_mode": True}),
                encoding="utf-8",
            )
            (trial / "result.json").write_text(
                json.dumps(
                    {
                        "schema_version": "0.3.0",
                        "apparatus_check": False,
                        "debug_mode": True,
                    }
                ),
                encoding="utf-8",
            )
            template = root / "template.json"
            template.write_text(
                json.dumps(
                    {
                        "schema_version": "0.3.0",
                        "eligibility_status": "eligible",
                        "pre_freeze": False,
                    }
                ),
                encoding="utf-8",
            )

            with mock.patch("cua_bench_runtime.export_trial.inspect_trial", return_value={}):
                with self.assertRaisesRegex(ValidationFailure, "debug-mode"):
                    export_trial(trial, template, root / "out.cuabench.json")

            self.assertFalse((root / "out.cuabench.json").exists())

    def _run(
        self,
        root: Path,
        trial_id: str,
        *,
        unavailable: bool = False,
        partial: bool = False,
    ):
        policy_test = test_policy.PolicyTests()
        if unavailable:

            def record_unavailable(_self, controller, **_changes):
                controller.record_model_call(
                    route_id="route.primary",
                    role="primary",
                    provider="synthetic-provider",
                    model="synthetic-model",
                    snapshot="2026-08-01",
                    service_tier="standard",
                    tokens=None,
                    cost_usd=None,
                    trust="non_certifying",
                )

            policy_test.record_primary = MethodType(record_unavailable, policy_test)
        elif partial:

            def record_partial(_self, controller, **_changes):
                test_policy.PolicyTests.record_primary(
                    _self,
                    controller,
                    includes_subagents=False,
                    trust="non_certifying",
                )

            policy_test.record_primary = MethodType(record_partial, policy_test)
        code, trial_dir, result = policy_test.run_instrumented(root / "runs", trial_id=trial_id)
        self.assertEqual(code, 0)
        return trial_dir, result

    def _template(self, root: Path, trial_dir: Path, result: dict) -> Path:
        examples = root / "examples"
        shutil.copytree(test_policy.EXAMPLES, examples)
        path = examples / "trial.system-track.cuabench.json"
        template = json.loads(path.read_text(encoding="utf-8"))
        config = json.loads((trial_dir / "config.json").read_text(encoding="utf-8"))
        template["id"] = result["trial_id"]
        template["task"] = {
            "id": config["task"]["id"],
            "version": config["task"]["version"],
            "manifest_sha256": config["task"]["digest"].removeprefix("sha256:"),
        }
        template["variant"] = config["variant"]
        certification = template["execution"]["certification"]
        certification["bindings"]["trial_id"] = result["trial_id"]
        certification["bindings"]["task_digest"] = template["task"]["manifest_sha256"]
        path.write_text(json.dumps(template), encoding="utf-8")
        return path

    def test_eligible_export_round_trips_to_normalize_trials(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.eligible")
            template = self._template(root, trial_dir, result)
            out = root / "results" / "trial.cuabench.json"

            manifest = export_trial(trial_dir, template, out)
            rows = normalize_trials(
                [out],
                [template.parent / "system.cuabench.json"],
                [template.parent / "execution-policy.cuabench.json"],
            )

        self.assertEqual(
            manifest["execution"]["comparison_eligibility"],
            {"status": "eligible", "reasons": []},
        )
        self.assertTrue(rows[0]["eligible"])
        self.assertEqual(rows[0]["tokens"], 120)

    def test_infrastructure_failure_does_not_export_model_completion(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.infrastructure")
            template = self._template(root, trial_dir, result)
            verified = inspect_trial(trial_dir)
            verified["certifying"] = False
            result["status"] = "infrastructure_error"
            (trial_dir / "result.json").write_text(json.dumps(result), encoding="utf-8")
            out = root / "infrastructure.cuabench.json"

            with mock.patch("cua_bench_runtime.export_trial.inspect_trial", return_value=verified):
                manifest = export_trial(trial_dir, template, out)
            rows = normalize_trials(
                [out],
                [template.parent / "system.cuabench.json"],
                [template.parent / "execution-policy.cuabench.json"],
            )

            self.assertEqual(manifest["termination_status"], "infrastructure_error")
            self.assertFalse(manifest["outcomes"]["completion"])
            self.assertFalse(rows[0]["certified"])
            self.assertTrue(rows[0]["infrastructure_failure"])
            self.assertTrue(
                any(item["path"].endswith("evaluation.json") for item in manifest["evidence"])
            )

    def test_unavailable_telemetry_stays_null_and_ineligible(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.unavailable", unavailable=True)
            template = self._template(root, trial_dir, result)
            out = root / "trial.cuabench.json"
            manifest = export_trial(trial_dir, template, out)
            rows = normalize_trials(
                [out],
                [trial_dir / "inputs/system.cuabench.json"],
                [trial_dir / "inputs/execution-policy.cuabench.json"],
            )

        self.assertIsNone(manifest["execution"]["observed"]["tokens"])
        self.assertIsNone(manifest["observables"]["tokens"])
        self.assertIsNone(manifest["observables"]["cost_usd"])
        self.assertEqual(
            manifest["execution"]["comparison_eligibility"]["status"],
            "ineligible",
        )
        self.assertIn(
            "model_telemetry_unavailable",
            manifest["execution"]["comparison_eligibility"]["reasons"],
        )
        self.assertFalse(rows[0]["eligible"])
        self.assertIsNone(rows[0]["tokens"])
        self.assertIsNone(rows[0]["cost_usd"])

    def test_partial_accounting_is_provenance_only_for_certifying_export(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.partial", partial=True)
            template = self._template(root, trial_dir, result)
            template_value = json.loads(template.read_text(encoding="utf-8"))
            config = json.loads((trial_dir / "config.json").read_text(encoding="utf-8"))
            verified = inspect_trial(trial_dir)
            verified["execution_policy"]["observed"]["cost_usd"] = None
            verified["participation"].update(
                {
                    "required": True,
                    "status": "passed",
                    "passed": True,
                    "observer": {"name": "synthetic-observer", "trust": "certifying"},
                    "requirements": [
                        {
                            "requirement_id": "synthetic.participation",
                            "status": "satisfied",
                            "evidence_event_hashes": ["a" * 64],
                        }
                    ],
                }
            )
            signature_path = trial_dir / "artifacts/fake-certification-signature.json"
            signature_path.write_text('{"signature":"synthetic"}\n', encoding="utf-8")
            verified["apparatus_certification"] = {
                "bindings": {
                    "trial_id": result["trial_id"],
                    "task_digest": config["task"]["digest"],
                    "system_digest": config["system"]["digest"],
                    "execution_policy_digest": config["execution_policy"]["digest"],
                    "seed_provenance_digest": "sha256:"
                    + template_value["environment"]["resolved_seed_provenance_sha256"],
                },
                "apparatus_decision": {"status": "passed", "reasons": []},
                "eligible": True,
            }
            verified["apparatus_certification_signature"] = {
                "path": signature_path.relative_to(trial_dir).as_posix(),
                "digest": digest_file(signature_path),
                "key_id": "sha256:" + "c" * 64,
            }
            verified["certifying"] = True
            out = root / "partial.cuabench.json"

            with mock.patch("cua_bench_runtime.export_trial.inspect_trial", return_value=verified):
                manifest = export_trial(trial_dir, template, out)
            policy_receipt_path = next(
                (root / "partial.cuabench.json.artifacts").glob("*execution-policy-receipt.json")
            )
            exported_policy_receipt = json.loads(policy_receipt_path.read_text(encoding="utf-8"))
            rows = normalize_trials(
                [out],
                [template.parent / "system.cuabench.json"],
                [template.parent / "execution-policy.cuabench.json"],
            )
            report = build_report(rows, "system-track", bootstrap_samples=10)

        self.assertFalse(manifest["execution"]["observed"]["tokens"]["includes_subagents"])
        self.assertIsNone(exported_policy_receipt["observed"]["cost_usd"])
        self.assertIsNone(manifest["observables"]["tokens"])
        self.assertIsNone(manifest["observables"]["cost_usd"])
        self.assertTrue(manifest["execution"]["certification"]["certifying"])
        self.assertFalse(rows[0]["certified"])
        self.assertFalse(rows[0]["comparison_eligible"])
        self.assertIsNone(rows[0]["tokens"])
        self.assertIsNone(rows[0]["cost_usd"])
        self.assertEqual(report["arms"][0]["tokens"], {"median": None, "sample_count": 0})
        self.assertEqual(report["arms"][0]["cost_usd"], {"median": None, "sample_count": 0})

    def test_comparison_eligible_export_rejects_partial_token_scope(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.partial-forged")
            template = self._template(root, trial_dir, result)
            verified = inspect_trial(trial_dir)
            verified["execution_policy"]["observed"]["tokens"]["includes_subagents"] = False

            with mock.patch("cua_bench_runtime.export_trial.inspect_trial", return_value=verified):
                with self.assertRaisesRegex(
                    ValidationFailure, "subagent-inclusive accounting telemetry"
                ):
                    export_trial(trial_dir, template, root / "forged.cuabench.json")

    def test_certification_uses_seed_provenance_not_descriptor_digest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.image-binding")
            template = self._template(root, trial_dir, result)
            template_value = json.loads(template.read_text(encoding="utf-8"))
            resolved_image = "9" * 64
            self.assertNotEqual(template_value["environment"]["image"]["sha256"], resolved_image)
            template_value["environment"]["resolved_seed_provenance_sha256"] = resolved_image
            template.write_text(json.dumps(template_value), encoding="utf-8")

            signature_path = trial_dir / "artifacts/fake-certification-signature.json"
            signature_path.write_text('{"signature":"synthetic"}\n', encoding="utf-8")
            config = json.loads((trial_dir / "config.json").read_text(encoding="utf-8"))
            verified = inspect_trial(trial_dir)
            verified["apparatus_certification"] = {
                "bindings": {
                    "trial_id": result["trial_id"],
                    "task_digest": config["task"]["digest"],
                    "system_digest": config["system"]["digest"],
                    "execution_policy_digest": config["execution_policy"]["digest"],
                    "seed_provenance_digest": "sha256:" + resolved_image,
                },
                "apparatus_decision": {
                    "status": "unavailable",
                    "reasons": ["synthetic_apparatus_unavailable"],
                },
                "eligible": False,
            }
            verified["apparatus_certification_signature"] = {
                "path": signature_path.relative_to(trial_dir).as_posix(),
                "digest": digest_file(signature_path),
                "key_id": "sha256:" + "c" * 64,
            }
            verified["certifying"] = False
            out = root / "resolved-seed-provenance.cuabench.json"
            with mock.patch("cua_bench_runtime.export_trial.inspect_trial", return_value=verified):
                manifest = export_trial(trial_dir, template, out)

        self.assertEqual(
            manifest["execution"]["certification"]["bindings"]["resolved_seed_provenance_sha256"],
            resolved_image,
        )

    def test_source_mutation_during_copy_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.toctou")
            template = self._template(root, trial_dir, result)
            out = root / "toctou.cuabench.json"
            original_copy = exporter._copy_atomic
            mutated = False

            def mutate_then_copy(source, destination, expected_digest):
                nonlocal mutated
                if (
                    not mutated
                    and isinstance(source, Path)
                    and source.resolve() == (trial_dir / "events.ndjson").resolve()
                ):
                    source.write_bytes(source.read_bytes() + b"changed\n")
                    mutated = True
                return original_copy(source, destination, expected_digest)

            with mock.patch(
                "cua_bench_runtime.export_trial._copy_atomic", side_effect=mutate_then_copy
            ):
                with self.assertRaisesRegex(ValidationFailure, "source changed"):
                    export_trial(trial_dir, template, out)
            self.assertFalse(out.exists())

    def test_manifest_publish_failure_rolls_back_artifact_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.publish-failure")
            template = self._template(root, trial_dir, result)
            out = root / "publish-failure.cuabench.json"
            artifact_dir = out.parent / f"{out.name}.artifacts"
            original_replace = exporter.os.replace

            def fail_manifest_publish(source, destination):
                if Path(destination).resolve() == out.resolve():
                    raise OSError("synthetic manifest publish failure")
                return original_replace(source, destination)

            with mock.patch(
                "cua_bench_runtime.export_trial.os.replace", side_effect=fail_manifest_publish
            ):
                with self.assertRaisesRegex(OSError, "manifest publish failure"):
                    export_trial(trial_dir, template, out)

            self.assertFalse(out.exists())
            self.assertFalse(artifact_dir.exists())
            self.assertEqual(list(root.glob(".cb-export-stage-*")), [])

    def test_symlinked_source_and_destination_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.symlinks")
            template = self._template(root, trial_dir, result)
            template_value = json.loads(template.read_text(encoding="utf-8"))
            source = template.parent / template_value["environment"]["image"]["path"]
            source_payload = source.read_bytes()
            target = root / "artifact-target"
            target.write_bytes(source_payload)
            try:
                source.unlink()
                source.symlink_to(target)
            except OSError as error:
                self.skipTest(f"symlink creation unavailable: {error}")
            with self.assertRaisesRegex(ValidationFailure, "must not be a symlink"):
                export_trial(trial_dir, template, root / "source-link.json")

            output_target = root / "output-target.json"
            output_target.write_text("unchanged", encoding="utf-8")
            output_link = root / "output-link.json"
            output_link.symlink_to(output_target)
            with self.assertRaisesRegex(ValidationFailure, "must not be a symlink"):
                export_trial(trial_dir, template, output_link)
            self.assertEqual(output_target.read_text(encoding="utf-8"), "unchanged")

    def test_report_semantic_validation_runs_before_install(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.semantic")
            template = self._template(root, trial_dir, result)
            value = json.loads(template.read_text(encoding="utf-8"))
            value["candidate"]["manifest_sha256"] = "0" * 64
            template.write_text(json.dumps(value), encoding="utf-8")
            out = root / "semantic.cuabench.json"

            with self.assertRaisesRegex(ValidationFailure, "candidate"):
                export_trial(trial_dir, template, out)
            self.assertFalse(out.exists())

    def test_contradiction_and_missing_accounting_are_rejected_atomically(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            trial_dir, result = self._run(root, "trial.export.reject")
            template = self._template(root, trial_dir, result)
            value = json.loads(template.read_text(encoding="utf-8"))
            value["variant"] = "fixture.contradiction"
            template.write_text(json.dumps(value), encoding="utf-8")
            out = root / "contradiction.cuabench.json"
            with self.assertRaisesRegex(ValidationFailure, "variant"):
                export_trial(trial_dir, template, out)
            self.assertFalse(out.exists())

            template = self._template(root / "second", trial_dir, result)
            verified = inspect_trial(trial_dir)
            verified["execution_policy"]["observed"]["tokens"] = None
            missing_out = root / "missing.cuabench.json"
            with mock.patch("cua_bench_runtime.export_trial.inspect_trial", return_value=verified):
                with self.assertRaisesRegex(ValidationFailure, "accounting telemetry"):
                    export_trial(trial_dir, template, missing_out)
            self.assertFalse(missing_out.exists())


if __name__ == "__main__":
    unittest.main()
