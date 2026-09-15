from __future__ import annotations

import copy
import json
import shutil
import tempfile
from pathlib import Path
import unittest

from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.schemas import (
    REPO_ROOT,
    SCHEMA_ROOT,
    load_json,
    validate_manifest,
    validate_task_artifacts,
)


class SchemaTests(unittest.TestCase):
    def test_synthetic_task_uses_canonical_schema(self) -> None:
        path = REPO_ROOT / "conformance/tasks/synthetic-echo-v1/task.cuabench.json"
        task = validate_manifest(path, "task")
        self.assertEqual(task["id"], "cua.synthetic.lifecycle-echo")
        self.assertEqual(task["evaluator"]["staging"], "agent-visible")

    def test_evaluator_entrypoint_must_be_digest_pinned(self) -> None:
        path = REPO_ROOT / "conformance/tasks/synthetic-echo-v1/task.cuabench.json"
        task = copy.deepcopy(load_json(path))
        task["evaluator"]["entrypoint"] = "reset/reset-workspace.txt"
        with self.assertRaisesRegex(ValidationFailure, "pinned"):
            validate_task_artifacts(path, task)

    def test_runtime_selects_schema_from_manifest_version(self) -> None:
        source = REPO_ROOT / "conformance/tasks/synthetic-echo-v1/task.cuabench.json"
        task = load_json(source)
        task["schema_version"] = "0.2.0"
        task["participation_requirements"] = [
            {
                "id": "participation.synthetic.update",
                "target": {
                    "application_id": "application.example-desk",
                    "surface_id": "surface.record.item-1042",
                },
                "sequence": [
                    {
                        "kind": "act",
                        "required_facts": {"record_id": "ITEM-1042"},
                    }
                ],
            }
        ]
        with tempfile.TemporaryDirectory() as directory:
            task_root = Path(directory) / "task"
            shutil.copytree(source.parent, task_root)
            path = task_root / "task.cuabench.json"
            path.write_text(json.dumps(task), encoding="utf-8")
            validated = validate_manifest(path, "task")
        self.assertEqual(validated["schema_version"], "0.2.0")
        self.assertEqual(len(validated["participation_requirements"]), 1)

    def test_unknown_schema_version_is_rejected_without_network_lookup(self) -> None:
        source = REPO_ROOT / "conformance/tasks/synthetic-echo-v1/task.cuabench.json"
        task = load_json(source)
        task["schema_version"] = "0.9.0"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "task.cuabench.json"
            path.write_text(json.dumps(task), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "no schemas found"):
                validate_manifest(path, "task")

    def test_v03_system_policy_and_runtime_shaped_trial_validate(self) -> None:
        examples = SCHEMA_ROOT / "examples"
        system = validate_manifest(examples / "system.cuabench.json")
        policy = validate_manifest(examples / "execution-policy.cuabench.json")
        trial = validate_manifest(examples / "trial.system-track.cuabench.json")
        self.assertEqual(system["configuration_class"], "maintainer_default")
        self.assertEqual(policy["autonomy"]["max_human_interventions"], 0)
        self.assertTrue(trial["execution"]["observed"]["fresh_harness_workspace"]["satisfied"])

    def test_v03_trial_rejects_missing_fresh_workspace_observation(self) -> None:
        source = SCHEMA_ROOT / "examples" / "trial.system-track.cuabench.json"
        trial = load_json(source)
        del trial["execution"]["observed"]["fresh_harness_workspace"]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trial.system-track.cuabench.json"
            path.write_text(json.dumps(trial), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "fresh_harness_workspace"):
                validate_manifest(path)

    def test_v03_proxy_policy_requires_all_frozen_bindings(self) -> None:
        source = SCHEMA_ROOT / "examples/execution-policy.cuabench.json"
        policy = load_json(source)
        policy["network"] = {
            "mode": "allowlist",
            "allowlist_sha256": "1" * 64,
            "proxy_endpoint": "192.0.2.10@8443",
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "execution-policy.cuabench.json"
            path.write_text(json.dumps(policy), encoding="utf-8")
            with self.assertRaisesRegex(ValidationFailure, "provider_allowlist_sha256"):
                validate_manifest(path, "execution-policy")
            policy["network"].update(
                {
                    "provider_allowlist_sha256": "2" * 64,
                    "proxy_implementation_sha256": "3" * 64,
                }
            )
            path.write_text(json.dumps(policy), encoding="utf-8")
            validate_manifest(path, "execution-policy")

    def test_v03_represents_noncertifying_and_unsigned_execution_states(self) -> None:
        source = SCHEMA_ROOT / "examples" / "trial.system-track.cuabench.json"
        for mode in ("apparatus_passed_participation_failed", "unsigned_failure", "absent"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                trial = load_json(source)
                certification = trial["execution"]["certification"]
                certification["certifying"] = False
                certification["reasons"] = ["participation_not_certifying"]
                if mode == "unsigned_failure":
                    certification["apparatus_status"] = "unavailable"
                    for field in ("receipt", "signature", "verifier_key_id"):
                        certification.pop(field)
                elif mode == "absent":
                    trial["execution"].pop("certification")
                path = Path(directory) / "trial.system-track.cuabench.json"
                path.write_text(json.dumps(trial), encoding="utf-8")
                validate_manifest(path)


if __name__ == "__main__":
    unittest.main()
