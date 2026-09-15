"""Integrity verification and human-readable trial explanation."""

from __future__ import annotations

import hashlib
import json
import os
import re
import stat
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from cua_bench_runtime.canon import digest_file, digest_json
from cua_bench_runtime.certification import verify_certification_receipt
from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.events import verify_event_log
from cua_bench_runtime.policy import certification_integrity
from cua_bench_runtime.receipt_signing import (
    key_id,
    verify_certification_signature,
    verify_receipt,
)


_PROVIDER_PROXY_ARTIFACT_FIELDS = frozenset(
    {
        "schema_version",
        "trial_id",
        "endpoint",
        "allowed_client_ip",
        "client_binding_digest",
        "allowed_authorities_digest",
        "implementation_identity",
        "implementation_digest",
        "accepted_connections",
        "rejected_connections",
        "bytes_guest_to_provider",
        "bytes_provider_to_guest",
        "transcript_chain_digest",
        "active",
        "sealed",
    }
)

_EMPTY_PROVIDER_PROXY_TRANSCRIPT = (
    "sha256:" + hashlib.sha256(b"cb.provider-connect-proxy/transcript/v2\n").hexdigest()
)

_DEBUG_FIELDS = frozenset(
    {
        "schema_version",
        "mode",
        "events",
        "parsed_event_count",
        "skipped_line_count",
        "token_totals",
        "termination",
        "provider_activity",
        "output",
        "raw_output_persisted",
    }
)
_DEBUG_EVENT_TYPES = frozenset(
    {
        "assistant",
        "error",
        "message",
        "result",
        "step_finish",
        "step_start",
        "system",
        "text",
        "tool_activity",
        "turn.completed",
        "turn.failed",
        "unknown",
    }
)
_DEBUG_FAILURES = frozenset(
    {
        "authentication_unavailable",
        "debug_capture_unavailable",
        "harness_failed",
        "provider_error",
        "rate_limited",
        "usage_limit",
    }
)
_SAFE_DEBUG_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/-]{0,255}$")
_SHA256 = re.compile(r"^sha256:[a-f0-9]{64}$")


def _verify_debug_artifact(trial_dir: Path, enabled: bool) -> None:
    path = (trial_dir / "artifacts/agent.debug.json").resolve()
    if not enabled:
        if path.exists():
            raise ValidationFailure("unexpected debug artifact")
        return
    if (
        not path.is_relative_to((trial_dir / "artifacts").resolve())
        or not path.is_file()
        or path.is_symlink()
    ):
        raise ValidationFailure("debug artifact is unavailable")
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValidationFailure("debug artifact is invalid") from error
    if (
        not isinstance(document, dict)
        or set(document) != _DEBUG_FIELDS
        or document.get("schema_version") != 1
        or document.get("mode") != "protected-content-free"
        or document.get("raw_output_persisted") is not False
    ):
        raise ValidationFailure("debug artifact contract is invalid")
    events = document.get("events")
    if (
        not isinstance(events, list)
        or len(events) > 1024
        or document.get("parsed_event_count") != len(events)
        or not isinstance(document.get("skipped_line_count"), int)
        or isinstance(document.get("skipped_line_count"), bool)
        or document["skipped_line_count"] < 0
    ):
        raise ValidationFailure("debug event summary is invalid")
    for sequence, event in enumerate(events, 1):
        if (
            not isinstance(event, dict)
            or set(event)
            not in ({"sequence", "event_type"}, {"sequence", "event_type", "tool_name"})
            or event.get("sequence") != sequence
            or event.get("event_type") not in _DEBUG_EVENT_TYPES
            or (
                "tool_name" in event
                and (
                    not isinstance(event["tool_name"], str)
                    or _SAFE_DEBUG_NAME.fullmatch(event["tool_name"]) is None
                )
            )
        ):
            raise ValidationFailure("debug event is invalid")
    tokens = document.get("token_totals")
    if tokens is not None and (
        not isinstance(tokens, dict)
        or set(tokens) != {"input", "output", "cache_read", "cache_write"}
        or any(
            not isinstance(value, int) or isinstance(value, bool) or value < 0
            for value in tokens.values()
        )
    ):
        raise ValidationFailure("debug token totals are invalid")
    termination = document.get("termination")
    if (
        not isinstance(termination, dict)
        or set(termination)
        not in (
            {"classification", "exit_code", "terminal_failure"},
            {"classification", "exit_code", "terminal_failure", "error_type"},
        )
        or termination.get("classification") not in {"completed", "failed", "timeout"}
        or not (
            termination.get("exit_code") is None
            or (
                isinstance(termination.get("exit_code"), int)
                and not isinstance(termination.get("exit_code"), bool)
            )
        )
        or not (
            termination.get("terminal_failure") is None
            or termination.get("terminal_failure") in _DEBUG_FAILURES
        )
        or (
            "error_type" in termination
            and (
                not isinstance(termination["error_type"], str)
                or re.fullmatch(r"[A-Za-z][A-Za-z0-9_]{0,127}", termination["error_type"]) is None
            )
        )
    ):
        raise ValidationFailure("debug termination summary is invalid")
    provider = document.get("provider_activity")
    provider_fields = {
        "accepted_connections",
        "rejected_connections",
        "bytes_guest_to_provider",
        "bytes_provider_to_guest",
    }
    if provider is not None and (
        not isinstance(provider, dict)
        or set(provider) != provider_fields
        or any(
            not isinstance(value, int) or isinstance(value, bool) or value < 0
            for value in provider.values()
        )
    ):
        raise ValidationFailure("debug provider summary is invalid")
    output = document.get("output")
    if output is not None:
        if not isinstance(output, dict) or set(output) != {"stdout", "stderr"}:
            raise ValidationFailure("debug output summary is invalid")
        for stream in output.values():
            if (
                not isinstance(stream, dict)
                or set(stream) != {"bytes", "sha256", "truncated"}
                or not isinstance(stream.get("bytes"), int)
                or isinstance(stream.get("bytes"), bool)
                or stream["bytes"] < 0
                or not isinstance(stream.get("sha256"), str)
                or _SHA256.fullmatch(stream["sha256"]) is None
                or not isinstance(stream.get("truncated"), bool)
            ):
                raise ValidationFailure("debug output summary is invalid")


def _verify_provider_proxy_artifact(trial_dir: Path, summary: Mapping[str, Any] | None) -> None:
    path = (trial_dir / "artifacts/provider-proxy.evidence.json").resolve()
    if summary is None:
        if path.exists():
            raise ValidationFailure("unexpected provider proxy evidence artifact")
        return
    if not isinstance(summary, Mapping):
        raise ValidationFailure("provider proxy apparatus evidence is invalid")
    if (
        not path.is_relative_to((trial_dir / "artifacts").resolve())
        or not path.is_file()
        or path.is_symlink()
        or digest_file(path) != summary.get("artifact_sha256")
    ):
        raise ValidationFailure("provider proxy evidence artifact digest mismatch")
    try:
        artifact = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValidationFailure("provider proxy evidence artifact is invalid") from error
    if (
        not isinstance(artifact, dict)
        or set(artifact) != _PROVIDER_PROXY_ARTIFACT_FIELDS
        or digest_json(artifact) != summary.get("sealed_evidence_digest")
    ):
        raise ValidationFailure("provider proxy sealed evidence mismatch")
    correlations = {
        "schema_version": "schema_version",
        "trial_id": "trial_id",
        "endpoint": "endpoint",
        "allowed_client_ip": "allowed_client_ip",
        "allowed_authorities_digest": "provider_allowlist_sha256",
        "client_binding_digest": "sealed_client_binding_digest",
        "implementation_identity": "implementation_identity",
        "implementation_digest": "sealed_implementation_digest",
        "accepted_connections": "accepted_connections",
        "rejected_connections": "rejected_connections",
        "bytes_guest_to_provider": "bytes_guest_to_provider",
        "bytes_provider_to_guest": "bytes_provider_to_guest",
        "transcript_chain_digest": "transcript_chain_digest",
        "active": "active",
        "sealed": "sealed",
    }
    if any(artifact[source] != summary.get(target) for source, target in correlations.items()):
        raise ValidationFailure("provider proxy artifact binding mismatch")
    initial_artifact = {
        **artifact,
        "accepted_connections": 0,
        "rejected_connections": 0,
        "bytes_guest_to_provider": 0,
        "bytes_provider_to_guest": 0,
        "transcript_chain_digest": _EMPTY_PROVIDER_PROXY_TRANSCRIPT,
        "active": True,
        "sealed": False,
    }
    if digest_json(initial_artifact) != summary.get("initial_evidence_digest"):
        raise ValidationFailure("provider proxy initial evidence mismatch")


def _verify_provider_proxy_configuration_binding(
    trial_dir: Path,
    config: Mapping[str, Any],
    summary: Mapping[str, Any] | None,
) -> None:
    """Bind proxy evidence to the exact pinned policy and system configuration."""

    if summary is None:
        return
    try:
        system_path = trial_dir / "inputs/system.cuabench.json"
        policy_path = trial_dir / "inputs/execution-policy.cuabench.json"
        system = json.loads(system_path.read_text(encoding="utf-8"))
        policy = json.loads(policy_path.read_text(encoding="utf-8"))
        configuration = system["harness"]["configuration"]
        configuration_relative = configuration["path"]
        configuration_path = (
            trial_dir / "inputs/system-artifacts" / configuration_relative
        ).resolve()
        system_configuration = json.loads(configuration_path.read_text(encoding="utf-8"))
        network = policy["network"]
    except (OSError, KeyError, TypeError, UnicodeError, json.JSONDecodeError) as error:
        raise ValidationFailure("provider proxy pinned configuration is invalid") from error
    config_system = config.get("system")
    config_policy = config.get("execution_policy")
    if (
        not isinstance(config_system, Mapping)
        or config_system.get("digest") != digest_file(system_path)
        or not isinstance(config_policy, Mapping)
        or config_policy.get("digest") != digest_file(policy_path)
        or not isinstance(configuration_relative, str)
        or not configuration_relative
        or not configuration_path.is_file()
        or configuration_path.is_symlink()
        or not configuration_path.is_relative_to((trial_dir / "inputs/system-artifacts").resolve())
        or digest_file(configuration_path) != "sha256:" + str(configuration.get("sha256"))
        or not isinstance(system_configuration, dict)
        or not isinstance(network, dict)
    ):
        raise ValidationFailure("provider proxy pinned configuration mismatch")
    allowlist = system_configuration.get("provider_allowlist")
    if not isinstance(allowlist, list) or any(
        not isinstance(entry, str) or entry.count("@") != 1 for entry in allowlist
    ):
        raise ValidationFailure("provider proxy allowlist binding is invalid")
    authorities = [entry.replace("@", ":") for entry in allowlist]
    endpoint = system_configuration.get("proxy_endpoint")
    if not isinstance(endpoint, str) or endpoint.count("@") != 1:
        raise ValidationFailure("provider proxy endpoint binding is invalid")
    expected_endpoint = "http://" + endpoint.replace("@", ":")
    expected_allowlist_digest = digest_json(authorities)
    expected_implementation_digest = "sha256:" + str(
        system_configuration.get("proxy_implementation_sha256")
    )
    expected_client_digest = digest_json(
        {
            "allowed_authorities": authorities,
            "allowed_client_ip": summary.get("allowed_client_ip"),
            "endpoint": expected_endpoint,
        }
    )
    if (
        network.get("proxy_endpoint") != endpoint
        or system_configuration.get("provider_allowlist_sha256")
        != expected_allowlist_digest.removeprefix("sha256:")
        or network.get("provider_allowlist_sha256")
        != system_configuration.get("provider_allowlist_sha256")
        or network.get("proxy_implementation_sha256")
        != system_configuration.get("proxy_implementation_sha256")
        or summary.get("endpoint") != expected_endpoint
        or summary.get("provider_allowlist_sha256") != expected_allowlist_digest
        or summary.get("initial_client_binding_digest") != expected_client_digest
        or summary.get("sealed_client_binding_digest") != expected_client_digest
        or summary.get("initial_implementation_digest") != expected_implementation_digest
        or summary.get("sealed_implementation_digest") != expected_implementation_digest
    ):
        raise ValidationFailure("provider proxy configuration binding mismatch")


def _verify_mediator_seal_artifact(trial_dir: Path, apparatus: Mapping[str, Any]) -> None:
    """Recompute signed mediator facts from its canonical stopped-disk seal."""

    path = (trial_dir / "artifacts/protected/participation.sealed.json").resolve()
    expected_digest = apparatus.get("protected_report_digest")
    if expected_digest is None:
        if apparatus.get("production_harness") is not None:
            raise ValidationFailure("production mediator seal artifact is missing")
        return
    if (
        not path.is_relative_to((trial_dir / "artifacts/protected").resolve())
        or not path.is_file()
        or path.is_symlink()
        or digest_file(path) != expected_digest
    ):
        raise ValidationFailure("mediator seal artifact digest mismatch")
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValidationFailure("mediator seal artifact is invalid") from error
    if not isinstance(report, dict):
        raise ValidationFailure("mediator seal artifact is invalid")
    frozen_daemon_digest: str | None = None
    frozen_daemon_envelope_digest: str | None = None
    if apparatus.get("production_harness") is not None:
        try:
            system_path = trial_dir / "inputs/system.cuabench.json"
            system = json.loads(system_path.read_text(encoding="utf-8"))
            tool_declaration = system["capability_inventory"]["tools"]
            tool_relative = tool_declaration["path"]
            tool_root = (trial_dir / "inputs/system-artifacts").resolve()
            tool_path = tool_root / tool_relative
            tool_inventory = json.loads(tool_path.read_text(encoding="utf-8"))
            frozen_daemon_tools = tool_inventory["driver"]["daemon_tools_list"]["tools"]
            frozen_daemon_envelope = tool_inventory["driver"]["daemon_tools_list_envelope"]
            frozen_daemon_digest = tool_inventory["driver"]["daemon_tool_schemas_sha256"]
            frozen_daemon_envelope_digest = tool_inventory["driver"][
                "daemon_tools_list_envelope_sha256"
            ]
        except (
            OSError,
            KeyError,
            TypeError,
            UnicodeError,
            json.JSONDecodeError,
        ) as error:
            raise ValidationFailure("mediator frozen tool inventory is invalid") from error
        if (
            not isinstance(tool_relative, str)
            or not tool_relative
            or Path(tool_relative).is_absolute()
            or any(part in {"", ".", ".."} for part in Path(tool_relative).parts)
            or not tool_path.resolve().is_relative_to(tool_root)
            or not tool_path.is_file()
            or tool_path.is_symlink()
            or digest_file(tool_path) != "sha256:" + str(tool_declaration.get("sha256"))
            or not isinstance(frozen_daemon_tools, list)
            or not isinstance(frozen_daemon_digest, str)
            or digest_json(frozen_daemon_tools).removeprefix("sha256:") != frozen_daemon_digest
            or not isinstance(frozen_daemon_envelope, dict)
            or frozen_daemon_envelope.get("tools") != frozen_daemon_tools
            or not isinstance(frozen_daemon_envelope_digest, str)
            or digest_json(frozen_daemon_envelope).removeprefix("sha256:")
            != frozen_daemon_envelope_digest
        ):
            raise ValidationFailure("mediator frozen tool inventory binding mismatch")
    report_body = {key: value for key, value in report.items() if key != "report_digest"}
    report_digest = report.get("report_digest")
    if (
        not isinstance(report_digest, str)
        or len(report_digest) != 64
        or any(character not in "0123456789abcdef" for character in report_digest)
        or report_digest != digest_json(report_body).removeprefix("sha256:")
    ):
        raise ValidationFailure("mediator seal report digest mismatch")
    correlations = {
        "tool_contract_validated": "protected_tool_contract_validated",
        "observed_tool_contract_sha256": "observed_daemon_tool_schemas_sha256",
        "observed_daemon_tool_list_envelope_sha256": "observed_daemon_tool_list_envelope_sha256",
        "expected_daemon_tool_list_envelope_sha256": "expected_daemon_tool_list_envelope_sha256",
        "daemon_tool_list_envelope_validated": "protected_daemon_tool_list_envelope_validated",
        "event_log_tail": "protected_log_tail",
        "records": "protected_log_records",
        "transport_integrity": "protected_transport_integrity",
        "evidence_complete": "protected_evidence_complete",
        "off_target_activity": "protected_off_target_activity",
    }
    if any(report.get(source) != apparatus.get(target) for source, target in correlations.items()):
        raise ValidationFailure("mediator seal apparatus binding mismatch")
    log_digest = report.get("event_log_sha256")
    if (
        not isinstance(log_digest, str)
        or apparatus.get("protected_log_digest") != f"sha256:{log_digest}"
    ):
        raise ValidationFailure("mediator seal log binding mismatch")
    if apparatus.get("production_harness") is not None and (
        report.get("tool_contract_required") is not True
        or report.get("tool_contract_validated") is not True
        or report.get("observed_tool_contract_sha256")
        != report.get("expected_tool_contract_sha256")
        or report.get("expected_tool_contract_sha256") != frozen_daemon_digest
        or report.get("daemon_tool_list_envelope_required") is not True
        or report.get("daemon_tool_list_envelope_validated") is not True
        or report.get("observed_daemon_tool_list_envelope_sha256")
        != report.get("expected_daemon_tool_list_envelope_sha256")
        or report.get("expected_daemon_tool_list_envelope_sha256") != frozen_daemon_envelope_digest
    ):
        raise ValidationFailure("mediator seal tool contract mismatch")


def _verify_collection_manifest(
    manifest_path: Path,
    root: Path,
    *,
    remap: dict[str, Path] | None = None,
) -> None:
    try:
        document = json.loads(manifest_path.read_text(encoding="utf-8"))
        entries = document["files"]
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise ValidationFailure("collection manifest is invalid") from error
    if not isinstance(entries, list):
        raise ValidationFailure("collection manifest files are invalid")
    expected: dict[str, tuple[str, int]] = {}
    for entry in entries:
        if not isinstance(entry, dict) or set(entry) != {"path", "sha256", "size"}:
            raise ValidationFailure("collection manifest entry is invalid")
        relative = entry.get("path")
        digest = entry.get("sha256")
        size = entry.get("size")
        if (
            not isinstance(relative, str)
            or not relative
            or relative.startswith("/")
            or ".." in Path(relative).parts
            or "\\" in relative
            or relative in expected
            or not isinstance(digest, str)
            or len(digest) != 64
            or any(character not in "0123456789abcdef" for character in digest)
            or not isinstance(size, int)
            or isinstance(size, bool)
            or size < 0
        ):
            raise ValidationFailure("collection manifest entry is invalid")
        expected[relative] = (digest, size)
    remap = remap or {}
    remap_parts = {prefix: Path(prefix).parts for prefix in remap}
    actual: dict[str, tuple[str, int]] = {}
    if not root.is_dir() or root.is_symlink():
        raise ValidationFailure("collection root is unavailable")
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root).as_posix()
        relative_parts = Path(relative).parts
        if any(
            relative_parts[: len(prefix_parts)] == prefix_parts
            for prefix_parts in remap_parts.values()
        ):
            continue
        mode = os.lstat(path).st_mode
        if stat.S_ISDIR(mode):
            continue
        if not stat.S_ISREG(mode):
            raise ValidationFailure(f"collection contains a special file: {relative}")
        actual[relative] = (
            digest_file(path).removeprefix("sha256:"),
            path.stat().st_size,
        )
    for prefix, source_root in sorted(remap.items()):
        if not source_root.is_dir() or source_root.is_symlink():
            raise ValidationFailure("remapped collection root is unavailable")
        for path in sorted(source_root.rglob("*")):
            relative = Path(prefix) / path.relative_to(source_root)
            mode = os.lstat(path).st_mode
            if stat.S_ISDIR(mode):
                continue
            if not stat.S_ISREG(mode):
                raise ValidationFailure(
                    f"collection contains a special file: {relative.as_posix()}"
                )
            actual[relative.as_posix()] = (
                digest_file(path).removeprefix("sha256:"),
                path.stat().st_size,
            )
    if actual != expected:
        raise ValidationFailure("collection manifest contents mismatch")


def inspect_trial(trial_dir: Path) -> dict[str, Any]:
    trial_dir = trial_dir.resolve()
    config_path = trial_dir / "config.json"
    manifest_path = trial_dir / "inputs.manifest.json"
    result_path = trial_dir / "result.json"
    for path in (config_path, manifest_path, result_path, trial_dir / "events.ndjson"):
        if not path.is_file():
            raise ValidationFailure(f"missing trial artifact: {path.name}")

    config = json.loads(config_path.read_text(encoding="utf-8"))
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    result = json.loads(result_path.read_text(encoding="utf-8"))
    events = verify_event_log(trial_dir / "events.ndjson")
    if not events:
        raise ValidationFailure("event log is empty")
    start = next((event for event in events if event["type"] == "trial_started"), None)
    if start is None:
        raise ValidationFailure("event log has no trial_started record")
    actual_config = digest_json(config)
    if start["data"].get("config_digest") != actual_config:
        raise ValidationFailure("config digest does not match trial_started")
    if result.get("config_digest") != actual_config:
        raise ValidationFailure("result config digest mismatch")
    evaluator_runtime = config.get("evaluator_runtime")
    evaluation = result.get("evaluation")
    if evaluator_runtime is not None and evaluation is not None:
        if not isinstance(evaluator_runtime, dict) or not isinstance(evaluation, dict):
            raise ValidationFailure("evaluator runtime identity is invalid")
        expected_node = evaluator_runtime.get("node")
        detail = evaluation.get("detail")
        observed_node = detail.get("evaluator_node") if isinstance(detail, dict) else None
        if observed_node != expected_node:
            raise ValidationFailure(
                "evaluation Node identity does not match the pinned evaluator runtime"
            )
    actual_manifest = digest_file(manifest_path)
    started_manifest = start["data"].get("inputs_manifest_digest")
    result_manifest = result.get("inputs_manifest_digest")
    if started_manifest is not None or result_manifest is not None:
        if started_manifest != actual_manifest:
            raise ValidationFailure("input manifest digest does not match trial_started")
        if result_manifest != actual_manifest:
            raise ValidationFailure("result input manifest digest mismatch")
    elif result.get("schema_version") == "0.3.0":
        raise ValidationFailure("v0.3.0 result has no input manifest binding")
    for relative, expected in manifest["files"].items():
        source = (trial_dir / "inputs" / relative).resolve()
        if not source.is_relative_to((trial_dir / "inputs").resolve()):
            raise ValidationFailure(f"input manifest path escapes input root: {relative}")
        if not source.is_file() or digest_file(source) != expected:
            raise ValidationFailure(f"input digest mismatch: {relative}")
    event_digest = digest_file(trial_dir / "events.ndjson")
    if result.get("event_log_digest") != event_digest:
        raise ValidationFailure("result event log digest mismatch")
    participation = result.get("participation")
    participation_signature = result.get("participation_signature")
    certification = result.get("apparatus_certification")
    certification_signature = result.get("apparatus_certification_signature")
    policy_receipt = result.get("execution_policy")
    if result.get("apparatus_check", False) is not config.get("apparatus_check", False):
        raise ValidationFailure("apparatus-check status does not match config")
    debug_mode = config.get("debug_mode", False)
    if not isinstance(debug_mode, bool) or result.get("debug_mode", False) is not debug_mode:
        raise ValidationFailure("debug-mode status does not match config")
    _verify_debug_artifact(trial_dir, debug_mode)
    policy_eligible: bool | None = None
    policy_integrity = certification_integrity(None)
    if policy_receipt is not None:
        if not isinstance(policy_receipt, dict):
            raise ValidationFailure("execution policy receipt is not an object")
        policy_body = {
            key: value for key, value in policy_receipt.items() if key != "receipt_digest"
        }
        if policy_receipt.get("receipt_digest") != digest_json(policy_body):
            raise ValidationFailure("execution policy receipt digest mismatch")
        if policy_receipt.get("bindings") != {
            "trial_id": result.get("trial_id"),
            "system_digest": config.get("system", {}).get("digest"),
            "execution_policy_digest": config.get("execution_policy", {}).get("digest"),
            "config_digest": actual_config,
        }:
            raise ValidationFailure("execution policy receipt bindings mismatch")
        policy_event = next(
            (event for event in events if event["type"] == "execution_policy_receipt"),
            None,
        )
        if policy_event is None or policy_event["data"] != policy_receipt:
            raise ValidationFailure("event log execution policy receipt mismatch")
        policy_eligible = policy_receipt.get("eligible") is True
        policy_integrity = certification_integrity(policy_receipt)
        if result.get("comparison_eligible") is not policy_eligible:
            raise ValidationFailure("comparison eligibility contradicts policy receipt")
        if debug_mode and (
            policy_eligible
            or "debug_mode" not in policy_receipt.get("violations", [])
            or result.get("certifying") is not False
        ):
            raise ValidationFailure("debug-mode trial is certification eligible")
    elif config.get("schema_version") == "0.3.0":
        raise ValidationFailure("v0.3.0 result has no execution policy receipt")
    if participation is None and result.get("schema_version") == "0.1.0":
        certifying = bool(result.get("certifying"))
    else:
        if not isinstance(participation, dict):
            raise ValidationFailure("result has no participation receipt")
        receipt_body = {
            key: value for key, value in participation.items() if key != "receipt_digest"
        }
        if participation.get("receipt_digest") != digest_json(receipt_body):
            raise ValidationFailure("participation receipt digest mismatch")
        bindings = participation.get("bindings", {})
        if bindings != {
            "trial_id": result.get("trial_id"),
            "task_digest": config.get("task", {}).get("digest"),
            "config_digest": actual_config,
        }:
            raise ValidationFailure("participation receipt bindings mismatch")
        if result.get("environment_certifying") is not config.get("certifying"):
            raise ValidationFailure("result environment certification does not match config")
        receipt_event = next(
            (event for event in events if event["type"] == "driver_participation_receipt"),
            None,
        )
        if receipt_event is None or receipt_event["data"] != participation:
            raise ValidationFailure("event log participation receipt mismatch")
        participation_events = {
            event["hash"] for event in events if event["type"] == "driver_participation_event"
        }
        referenced = {
            event_hash
            for requirement in participation.get("requirements", [])
            for event_hash in requirement.get("evidence_event_hashes", [])
        }
        if not referenced.issubset(participation_events):
            raise ValidationFailure("participation receipt references unknown events")
        verifier = config.get("participation_verifier")
        signature_required = isinstance(verifier, dict) and verifier.get("required") is True
        signature_verified = not signature_required
        if participation_signature is not None:
            if not isinstance(participation_signature, dict) or set(participation_signature) != {
                "path",
                "digest",
                "key_id",
            }:
                raise ValidationFailure("participation signature descriptor is invalid")
            if not isinstance(verifier, dict):
                raise ValidationFailure("signed participation has no pinned verifier")
            if participation_signature["path"] != "artifacts/participation-receipt.sshsig.json":
                raise ValidationFailure("participation signature path is not canonical")
            artifact_path = (trial_dir / participation_signature["path"]).resolve()
            if not artifact_path.is_relative_to((trial_dir / "artifacts").resolve()):
                raise ValidationFailure("participation signature escapes artifacts")
            if (
                not artifact_path.is_file()
                or digest_file(artifact_path) != participation_signature["digest"]
            ):
                raise ValidationFailure("participation signature artifact digest mismatch")
            public_key = (trial_dir / verifier.get("path", "")).resolve()
            if not public_key.is_relative_to((trial_dir / "inputs").resolve()):
                raise ValidationFailure("participation verifier key escapes inputs")
            if not public_key.is_file() or digest_file(public_key) != verifier.get("digest"):
                raise ValidationFailure("participation verifier key digest mismatch")
            if key_id(public_key) != verifier.get("key_id") or participation_signature[
                "key_id"
            ] != verifier.get("key_id"):
                raise ValidationFailure("participation verifier key ID mismatch")
            artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
            verified_receipt = verify_receipt(
                artifact,
                trusted_public_key=public_key,
                expected_bindings=bindings,
            )
            if verified_receipt != participation:
                raise ValidationFailure("signed participation payload mismatch")
            signature_event = next(
                (event for event in events if event["type"] == "driver_participation_signature"),
                None,
            )
            if signature_event is None or signature_event["data"] != participation_signature:
                raise ValidationFailure("event log participation signature mismatch")
            signature_verified = True
        elif signature_required:
            signature_error = next(
                (event for event in events if event["type"] == "participation_signature_error"),
                None,
            )
            if signature_error is None:
                raise ValidationFailure("required participation signature is missing")
            signature_verified = False
        expected_certifying = (
            bool(result.get("environment_certifying"))
            and signature_verified
            and (
                not participation.get("required")
                or (
                    participation.get("passed") is True
                    and participation.get("observer", {}).get("trust") == "certifying"
                )
            )
        )
        if (
            config.get("environment_adapter") != "lume-macos-certifying"
            and policy_receipt is not None
        ):
            expected_certifying = bool(
                expected_certifying and policy_receipt.get("eligible") is True
            )
        if config.get("environment_adapter") == "lume-macos-certifying":
            if not isinstance(certification, dict):
                certification_error = next(
                    (event for event in events if event["type"] == "apparatus_certification_error"),
                    None,
                )
                if certification_error is None:
                    raise ValidationFailure(
                        "certifying Lume result has no apparatus certification receipt"
                    )
                expected_certifying = False
            else:
                verified_certification = verify_certification_receipt(certification)
                if verified_certification != certification:
                    raise ValidationFailure("apparatus certification receipt changed")
                receipt_event = next(
                    (
                        event
                        for event in events
                        if event["type"] == "apparatus_certification_receipt"
                    ),
                    None,
                )
                if receipt_event is None or receipt_event["data"] != certification:
                    raise ValidationFailure("event log apparatus certification receipt mismatch")
                certification_bindings = {
                    "trial_id": result.get("trial_id"),
                    "task_digest": config.get("task", {}).get("digest"),
                    "system_digest": config.get("system", {}).get("digest"),
                    "execution_policy_digest": config.get("execution_policy", {}).get("digest"),
                    "config_digest": actual_config,
                    "inputs_manifest_digest": actual_manifest,
                    "seed_provenance_digest": None,
                    "agent_digest": config.get("agent_command", {}).get("digest"),
                    "evaluator_digest": config.get("evaluator_command", {}).get("digest"),
                }
                lume_config_path = (trial_dir / "inputs/apparatus/lume/config.json").resolve()
                expected_lume_config = config.get("lume_configuration", {})
                if (
                    not lume_config_path.is_relative_to((trial_dir / "inputs").resolve())
                    or not lume_config_path.is_file()
                    or digest_file(lume_config_path) != expected_lume_config.get("digest")
                ):
                    raise ValidationFailure("pinned Lume configuration mismatch")
                try:
                    lume_config = json.loads(lume_config_path.read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError) as error:
                    raise ValidationFailure("pinned Lume configuration is invalid") from error
                certification_bindings["seed_provenance_digest"] = lume_config.get(
                    "seed_provenance_digest"
                )
                if certification.get("bindings") != certification_bindings:
                    raise ValidationFailure("apparatus certification bindings mismatch")
                evaluation = result.get("evaluation")
                expected_evaluation_digest = (
                    digest_json(evaluation) if evaluation is not None else None
                )
                if (
                    certification.get("outcome", {}).get("digest") != expected_evaluation_digest
                    or certification.get("apparatus", {}).get("evaluation_digest")
                    != expected_evaluation_digest
                ):
                    raise ValidationFailure("apparatus certification outcome binding mismatch")
                if evaluation is not None and (
                    certification.get("outcome", {}).get("passed") is not evaluation.get("passed")
                    or certification.get("outcome", {}).get("score") != evaluation.get("score")
                ):
                    raise ValidationFailure("apparatus certification outcome decision mismatch")
                if certification.get("participation", {}).get(
                    "receipt_digest"
                ) != participation.get("receipt_digest"):
                    raise ValidationFailure(
                        "apparatus certification participation binding mismatch"
                    )
                expected_policy_digest = (
                    policy_receipt.get("receipt_digest")
                    if isinstance(policy_receipt, dict)
                    else None
                )
                if (
                    certification.get("comparison", {}).get("receipt_digest")
                    != expected_policy_digest
                    or certification.get("apparatus", {}).get("execution_policy_receipt_digest")
                    != expected_policy_digest
                ):
                    raise ValidationFailure("apparatus certification policy binding mismatch")
                expected_participation_signature = (
                    participation_signature.get("digest")
                    if isinstance(participation_signature, dict)
                    else None
                )
                if (
                    certification.get("apparatus", {}).get("participation_signature_digest")
                    != expected_participation_signature
                ):
                    raise ValidationFailure(
                        "apparatus certification participation signature mismatch"
                    )
                if (
                    certification.get("apparatus", {}).get("execution_policy_integrity_passed")
                    is not policy_integrity["passed"]
                    or certification.get("apparatus", {}).get(
                        "execution_policy_integrity_violations"
                    )
                    != policy_integrity["violations"]
                ):
                    raise ValidationFailure("apparatus certification policy integrity mismatch")
                for relative, field in (
                    (
                        "artifacts/collection.manifest.json",
                        "collection_manifest_digest",
                    ),
                    (
                        "artifacts/protected-collection.manifest.json",
                        "protected_collection_manifest_digest",
                    ),
                    ("artifacts/protected/mediator.ndjson", "protected_log_digest"),
                ):
                    expected = certification.get("apparatus", {}).get(field)
                    artifact_path = (trial_dir / relative).resolve()
                    if expected is None:
                        if artifact_path.exists():
                            raise ValidationFailure(
                                f"apparatus certification artifact mismatch: {field}"
                            )
                        continue
                    if (
                        not isinstance(expected, str)
                        or not artifact_path.is_relative_to(trial_dir)
                        or not artifact_path.is_file()
                        or digest_file(artifact_path) != expected
                    ):
                        raise ValidationFailure(
                            f"apparatus certification artifact mismatch: {field}"
                        )
                _verify_provider_proxy_artifact(
                    trial_dir,
                    certification.get("apparatus", {}).get("provider_proxy"),
                )
                _verify_provider_proxy_configuration_binding(
                    trial_dir,
                    config,
                    certification.get("apparatus", {}).get("provider_proxy"),
                )
                _verify_mediator_seal_artifact(
                    trial_dir,
                    certification.get("apparatus", {}),
                )
                task_store_relative = certification.get("apparatus", {}).get("task_store_relative")
                if task_store_relative is not None and (
                    not isinstance(task_store_relative, str)
                    or not task_store_relative
                    or task_store_relative.startswith("/")
                    or ".." in Path(task_store_relative).parts
                    or "\\" in task_store_relative
                ):
                    raise ValidationFailure("apparatus certification task-store binding is invalid")
                if certification.get("apparatus", {}).get("collection_manifest_digest") is not None:
                    _verify_collection_manifest(
                        trial_dir / "artifacts/collection.manifest.json",
                        trial_dir / "artifacts/workspace",
                        remap=(
                            {task_store_relative: (trial_dir / "artifacts/untrusted-task-store")}
                            if task_store_relative is not None
                            else None
                        ),
                    )
                if (
                    certification.get("apparatus", {}).get("protected_collection_manifest_digest")
                    is not None
                ):
                    if task_store_relative is None:
                        raise ValidationFailure("protected collection has no task-store binding")
                    _verify_collection_manifest(
                        trial_dir / "artifacts/protected-collection.manifest.json",
                        trial_dir / "artifacts/protected",
                        remap={
                            task_store_relative: (
                                trial_dir / "artifacts/workspace" / task_store_relative
                            )
                        },
                    )
                certification_signature_verified = False
                if certification_signature is not None:
                    if not isinstance(certification_signature, dict) or set(
                        certification_signature
                    ) != {"path", "digest", "key_id"}:
                        raise ValidationFailure(
                            "apparatus certification signature descriptor is invalid"
                        )
                    canonical_path = "artifacts/apparatus-certification-receipt.sshsig.json"
                    if certification_signature["path"] != canonical_path:
                        raise ValidationFailure(
                            "apparatus certification signature path is not canonical"
                        )
                    signature_path = (trial_dir / certification_signature["path"]).resolve()
                    if (
                        not signature_path.is_relative_to((trial_dir / "artifacts").resolve())
                        or not signature_path.is_file()
                        or digest_file(signature_path) != certification_signature["digest"]
                    ):
                        raise ValidationFailure(
                            "apparatus certification signature artifact mismatch"
                        )
                    if not isinstance(verifier, dict):
                        raise ValidationFailure("apparatus certification has no pinned verifier")
                    public_key = (trial_dir / verifier.get("path", "")).resolve()
                    if (
                        not public_key.is_relative_to((trial_dir / "inputs").resolve())
                        or not public_key.is_file()
                        or digest_file(public_key) != verifier.get("digest")
                    ):
                        raise ValidationFailure("apparatus certification verifier key mismatch")
                    if certification_signature.get("key_id") != verifier.get("key_id") or key_id(
                        public_key
                    ) != verifier.get("key_id"):
                        raise ValidationFailure("apparatus certification verifier key ID mismatch")
                    signed_artifact = json.loads(signature_path.read_text(encoding="utf-8"))
                    signed_receipt = verify_certification_signature(
                        signed_artifact,
                        trusted_public_key=public_key,
                        expected_bindings=certification_bindings,
                    )
                    if signed_receipt != certification:
                        raise ValidationFailure("signed apparatus certification payload mismatch")
                    signature_event = next(
                        (
                            event
                            for event in events
                            if event["type"] == "apparatus_certification_signature"
                        ),
                        None,
                    )
                    if (
                        signature_event is None
                        or signature_event["data"] != certification_signature
                    ):
                        raise ValidationFailure(
                            "event log apparatus certification signature mismatch"
                        )
                    certification_signature_verified = True
                else:
                    signature_error = next(
                        (
                            event
                            for event in events
                            if event["type"] == "apparatus_certification_error"
                        ),
                        None,
                    )
                    if signature_error is None:
                        raise ValidationFailure(
                            "required apparatus certification signature is missing"
                        )
                expected_certifying = bool(
                    certification.get("eligible") is True and certification_signature_verified
                )
        if result.get("certifying") is not expected_certifying:
            raise ValidationFailure("certifying status contradicts participation receipt")
        certifying = expected_certifying
    if events[-1]["type"] != "state_enter" or events[-1]["data"] != {"state": "done"}:
        raise ValidationFailure("event log does not end in done state")
    states = [event["data"]["state"] for event in events if event["type"] == "state_enter"]
    return {
        "verified": True,
        "trial_id": result["trial_id"],
        "status": result["status"],
        "cleanup_ok": result["cleanup_ok"],
        "evaluation": result.get("evaluation"),
        "participation": participation,
        "participation_signature": participation_signature,
        "apparatus_certification": certification,
        "apparatus_certification_signature": certification_signature,
        "execution_policy": policy_receipt,
        "comparison_eligible": policy_eligible,
        "apparatus_check": bool(result.get("apparatus_check", False)),
        "debug_mode": debug_mode,
        "certifying": certifying,
        "states": states,
        "events": len(events),
    }


def narrative(report: dict[str, Any]) -> str:
    evaluation = report.get("evaluation")
    grade = "not graded"
    if evaluation is not None:
        grade = "passed" if evaluation["passed"] else "failed"
    states = " -> ".join(report["states"])
    participation_report = report.get("participation")
    participation = (
        participation_report["status"].replace("_", " ")
        if participation_report is not None
        else "not recorded (schema 0.1.0)"
    )
    policy_report = report.get("execution_policy")
    comparison = (
        policy_report["status"].replace("_", " ") if policy_report is not None else "not recorded"
    )
    return (
        f"Trial {report['trial_id']} is verified.\n"
        f"Status: {report['status']}; task: {grade}; cleanup: "
        f"{'complete' if report['cleanup_ok'] else 'failed'}; driver participation: "
        f"{participation}; comparison: {comparison}; certifying: "
        f"{'yes' if report['certifying'] else 'no'}; apparatus check: "
        f"{'yes' if report.get('apparatus_check') else 'no'}; debug mode: "
        f"{'yes' if report.get('debug_mode') else 'no'}.\n"
        f"Lifecycle: {states}."
    )
