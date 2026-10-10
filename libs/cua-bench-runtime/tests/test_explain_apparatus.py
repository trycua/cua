from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from cua_bench_runtime.canon import canonical_json, digest_file, digest_json
from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.explain import (
    _verify_debug_artifact,
    _verify_mediator_seal_artifact,
    _verify_provider_proxy_artifact,
    _verify_provider_proxy_configuration_binding,
)


class ExplainApparatusTests(unittest.TestCase):
    def test_debug_artifact_rejects_content_fields_and_normal_runs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            trial = Path(directory)
            artifacts = trial / "artifacts"
            artifacts.mkdir()
            path = artifacts / "agent.debug.json"
            document = {
                "schema_version": 1,
                "mode": "protected-content-free",
                "events": [{"sequence": 1, "event_type": "tool_activity", "tool_name": "click"}],
                "parsed_event_count": 1,
                "skipped_line_count": 0,
                "token_totals": {
                    "input": 10,
                    "output": 2,
                    "cache_read": 3,
                    "cache_write": 0,
                },
                "termination": {
                    "classification": "timeout",
                    "exit_code": None,
                    "terminal_failure": None,
                },
                "provider_activity": {
                    "accepted_connections": 1,
                    "rejected_connections": 0,
                    "bytes_guest_to_provider": 100,
                    "bytes_provider_to_guest": 200,
                },
                "output": {
                    stream: {
                        "bytes": 0,
                        "sha256": "sha256:" + hashlib.sha256(b"").hexdigest(),
                        "truncated": False,
                    }
                    for stream in ("stdout", "stderr")
                },
                "raw_output_persisted": False,
            }
            path.write_bytes(canonical_json(document) + b"\n")

            _verify_debug_artifact(trial, True)
            with self.assertRaisesRegex(ValidationFailure, "unexpected debug"):
                _verify_debug_artifact(trial, False)

            document["prompt"] = "must-not-persist"
            path.write_bytes(canonical_json(document) + b"\n")
            with self.assertRaisesRegex(ValidationFailure, "contract"):
                _verify_debug_artifact(trial, True)

    def test_provider_proxy_is_bound_to_pinned_configuration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            trial = Path(directory)
            inputs = trial / "inputs"
            configuration_path = inputs / "system-artifacts/frozen/config.json"
            configuration_path.parent.mkdir(parents=True)
            authorities = ["api.example.test:443"]
            allowlist_digest = digest_json(authorities).removeprefix("sha256:")
            system_configuration = {
                "proxy_endpoint": "192.0.2.10@8443",
                "provider_allowlist": ["api.example.test@443"],
                "provider_allowlist_sha256": allowlist_digest,
                "proxy_implementation_sha256": "b" * 64,
            }
            configuration_path.write_bytes(canonical_json(system_configuration) + b"\n")
            system = {
                "harness": {
                    "configuration": {
                        "path": "frozen/config.json",
                        "sha256": digest_file(configuration_path).removeprefix("sha256:"),
                    }
                }
            }
            policy = {
                "network": {
                    "proxy_endpoint": "192.0.2.10@8443",
                    "provider_allowlist_sha256": allowlist_digest,
                    "proxy_implementation_sha256": "b" * 64,
                }
            }
            system_path = inputs / "system.cuabench.json"
            policy_path = inputs / "execution-policy.cuabench.json"
            system_path.write_bytes(canonical_json(system) + b"\n")
            policy_path.write_bytes(canonical_json(policy) + b"\n")
            endpoint = "http://192.0.2.10:8443"
            summary = {
                "endpoint": endpoint,
                "allowed_client_ip": "192.0.2.1",
                "provider_allowlist_sha256": digest_json(authorities),
                "initial_client_binding_digest": digest_json(
                    {
                        "allowed_authorities": authorities,
                        "allowed_client_ip": "192.0.2.1",
                        "endpoint": endpoint,
                    }
                ),
                "sealed_client_binding_digest": digest_json(
                    {
                        "allowed_authorities": authorities,
                        "allowed_client_ip": "192.0.2.1",
                        "endpoint": endpoint,
                    }
                ),
                "initial_implementation_digest": "sha256:" + "b" * 64,
                "sealed_implementation_digest": "sha256:" + "b" * 64,
            }
            config = {
                "system": {"digest": digest_file(system_path)},
                "execution_policy": {"digest": digest_file(policy_path)},
            }
            _verify_provider_proxy_configuration_binding(trial, config, summary)

            swapped = dict(summary)
            swapped["sealed_client_binding_digest"] = "sha256:" + "c" * 64
            with self.assertRaisesRegex(ValidationFailure, "configuration binding"):
                _verify_provider_proxy_configuration_binding(trial, config, swapped)

    def test_provider_proxy_rejects_tampered_initial_binding(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            trial = Path(directory)
            artifacts = trial / "artifacts"
            artifacts.mkdir()
            client_digest = digest_json(
                {
                    "allowed_authorities": ["api.example.test:443"],
                    "allowed_client_ip": "192.0.2.1",
                    "endpoint": "http://192.0.2.10:8443",
                }
            )
            sealed = {
                "schema_version": 1,
                "trial_id": "trial-one",
                "endpoint": "http://192.0.2.10:8443",
                "allowed_client_ip": "192.0.2.1",
                "client_binding_digest": client_digest,
                "allowed_authorities_digest": digest_json(["api.example.test:443"]),
                "implementation_identity": "cb.provider-connect-proxy/v2",
                "implementation_digest": "sha256:" + "a" * 64,
                "accepted_connections": 1,
                "rejected_connections": 0,
                "bytes_guest_to_provider": 10,
                "bytes_provider_to_guest": 20,
                "transcript_chain_digest": "sha256:" + "b" * 64,
                "active": False,
                "sealed": True,
            }
            path = artifacts / "provider-proxy.evidence.json"
            path.write_bytes(canonical_json(sealed) + b"\n")
            initial = {
                **sealed,
                "accepted_connections": 0,
                "rejected_connections": 0,
                "bytes_guest_to_provider": 0,
                "bytes_provider_to_guest": 0,
                "transcript_chain_digest": "sha256:"
                + hashlib.sha256(b"cb.provider-connect-proxy/transcript/v2\n").hexdigest(),
                "active": True,
                "sealed": False,
            }
            summary = {
                "artifact_sha256": digest_file(path),
                "sealed_evidence_digest": digest_json(sealed),
                "initial_evidence_digest": digest_json(initial),
                "schema_version": sealed["schema_version"],
                "trial_id": sealed["trial_id"],
                "endpoint": sealed["endpoint"],
                "allowed_client_ip": sealed["allowed_client_ip"],
                "provider_allowlist_sha256": sealed["allowed_authorities_digest"],
                "sealed_client_binding_digest": client_digest,
                "implementation_identity": sealed["implementation_identity"],
                "sealed_implementation_digest": sealed["implementation_digest"],
                "accepted_connections": sealed["accepted_connections"],
                "rejected_connections": sealed["rejected_connections"],
                "bytes_guest_to_provider": sealed["bytes_guest_to_provider"],
                "bytes_provider_to_guest": sealed["bytes_provider_to_guest"],
                "transcript_chain_digest": sealed["transcript_chain_digest"],
                "active": False,
                "sealed": True,
            }
            _verify_provider_proxy_artifact(trial, summary)
            summary["initial_evidence_digest"] = "sha256:" + "c" * 64
            with self.assertRaisesRegex(ValidationFailure, "initial evidence"):
                _verify_provider_proxy_artifact(trial, summary)

    def test_mediator_seal_rejects_swap_tamper_and_missing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            trial = Path(directory)
            inputs = trial / "inputs"
            tool_path = inputs / "system-artifacts/frozen/tools.json"
            tool_path.parent.mkdir(parents=True)
            daemon_tools = [{"name": "observe", "input_schema": {"type": "object"}}]
            daemon_digest = digest_json(daemon_tools).removeprefix("sha256:")
            daemon_envelope = {
                "schema_version": "1",
                "capability_version": "1",
                "enforcement_adapters": [{"id": "accessibility", "available": True}],
                "tool_observation_owner": "daemon",
                "tools": daemon_tools,
            }
            daemon_envelope_digest = digest_json(daemon_envelope).removeprefix("sha256:")
            tool_inventory = {
                "driver": {
                    "daemon_tools_list": {"tools": daemon_tools},
                    "daemon_tool_schemas_sha256": daemon_digest,
                    "daemon_tools_list_envelope": daemon_envelope,
                    "daemon_tools_list_envelope_sha256": daemon_envelope_digest,
                }
            }
            tool_path.write_bytes(canonical_json(tool_inventory) + b"\n")
            system = {
                "capability_inventory": {
                    "tools": {
                        "path": "frozen/tools.json",
                        "sha256": digest_file(tool_path).removeprefix("sha256:"),
                    }
                }
            }
            (inputs / "system.cuabench.json").write_bytes(canonical_json(system) + b"\n")
            protected = trial / "artifacts/protected"
            protected.mkdir(parents=True)
            body = {
                "event_log_sha256": "a" * 64,
                "event_log_tail": "b" * 64,
                "records": 4,
                "transport_integrity": True,
                "evidence_complete": True,
                "off_target_activity": False,
                "tool_contract_required": True,
                "expected_tool_contract_sha256": daemon_digest,
                "observed_tool_contract_sha256": daemon_digest,
                "tool_contract_validated": True,
                "daemon_tool_list_envelope_required": True,
                "expected_daemon_tool_list_envelope_sha256": daemon_envelope_digest,
                "observed_daemon_tool_list_envelope_sha256": daemon_envelope_digest,
                "daemon_tool_list_envelope_validated": True,
            }
            report = {
                **body,
                "report_digest": digest_json(body).removeprefix("sha256:"),
            }
            path = protected / "participation.sealed.json"
            path.write_bytes(canonical_json(report) + b"\n")
            apparatus = {
                "production_harness": "codex",
                "protected_report_digest": digest_file(path),
                "protected_log_digest": "sha256:" + body["event_log_sha256"],
                "protected_log_tail": body["event_log_tail"],
                "protected_log_records": body["records"],
                "protected_transport_integrity": True,
                "protected_evidence_complete": True,
                "protected_off_target_activity": False,
                "protected_tool_contract_validated": True,
                "observed_daemon_tool_schemas_sha256": body["observed_tool_contract_sha256"],
                "observed_daemon_tool_list_envelope_sha256": body[
                    "observed_daemon_tool_list_envelope_sha256"
                ],
                "expected_daemon_tool_list_envelope_sha256": body[
                    "expected_daemon_tool_list_envelope_sha256"
                ],
                "protected_daemon_tool_list_envelope_validated": True,
            }
            _verify_mediator_seal_artifact(trial, apparatus)

            swapped = dict(apparatus)
            swapped["observed_daemon_tool_schemas_sha256"] = "d" * 64
            with self.assertRaisesRegex(ValidationFailure, "apparatus binding"):
                _verify_mediator_seal_artifact(trial, swapped)

            inventory_swapped = json.loads(tool_path.read_text(encoding="utf-8"))
            inventory_swapped["driver"]["daemon_tools_list"]["tools"][0]["name"] = "act"
            tool_path.write_bytes(canonical_json(inventory_swapped) + b"\n")
            system["capability_inventory"]["tools"]["sha256"] = digest_file(tool_path).removeprefix(
                "sha256:"
            )
            (inputs / "system.cuabench.json").write_bytes(canonical_json(system) + b"\n")
            with self.assertRaisesRegex(ValidationFailure, "tool inventory binding"):
                _verify_mediator_seal_artifact(trial, apparatus)
            tool_path.write_bytes(canonical_json(tool_inventory) + b"\n")
            system["capability_inventory"]["tools"]["sha256"] = digest_file(tool_path).removeprefix(
                "sha256:"
            )
            (inputs / "system.cuabench.json").write_bytes(canonical_json(system) + b"\n")

            tampered = dict(report)
            tampered["tool_contract_validated"] = False
            path.write_text(json.dumps(tampered), encoding="utf-8")
            tampered_apparatus = dict(apparatus)
            tampered_apparatus["protected_report_digest"] = digest_file(path)
            with self.assertRaisesRegex(ValidationFailure, "report digest"):
                _verify_mediator_seal_artifact(trial, tampered_apparatus)

            path.unlink()
            with self.assertRaisesRegex(ValidationFailure, "artifact digest"):
                _verify_mediator_seal_artifact(trial, apparatus)


if __name__ == "__main__":
    unittest.main()
