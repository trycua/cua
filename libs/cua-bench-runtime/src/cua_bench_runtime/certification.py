"""Pure apparatus-certification composition for runtime and verification."""

from __future__ import annotations

import re
import math
from collections.abc import Mapping
from typing import Any

from cua_bench_runtime.canon import digest_json
from cua_bench_runtime.errors import ValidationFailure


SCHEMA_VERSION = 1
_DIGEST = re.compile(r"^sha256:[a-f0-9]{64}$")
_HEX_DIGEST = re.compile(r"^[a-f0-9]{64}$")

APPARATUS_FIELDS = frozenset(
    {
        "environment_adapter",
        "environment_declared_certifying",
        "apparatus_check",
        "cleanup_ok",
        "seed_provenance_digest",
        "pristine_fingerprint",
        "post_run_fingerprint",
        "post_run_matches_pristine",
        "initial_driver_identity_digest",
        "post_run_driver_identity_digest",
        "fresh_harness_workspace",
        "target_reset",
        "task_store_relative",
        "agent_processes_frozen",
        "network_mode",
        "network_enforcer",
        "network_evidence_digest",
        "post_run_network_evidence_digest",
        "production_harness",
        "provider_proxy",
        "human_input_channel",
        "human_input_enforcer",
        "human_input_evidence_digest",
        "post_run_human_input_evidence_digest",
        "protected_endpoint_binding_digest",
        "sealed_endpoint_binding_digest",
        "vm_stopped_before_collection",
        "protected_collection_read_only",
        "collection_manifest_digest",
        "protected_collection_manifest_digest",
        "protected_log_digest",
        "protected_report_digest",
        "protected_log_tail",
        "protected_log_records",
        "protected_transport_integrity",
        "protected_evidence_complete",
        "protected_off_target_activity",
        "protected_tool_contract_validated",
        "observed_daemon_tool_schemas_sha256",
        "expected_daemon_tool_list_envelope_sha256",
        "observed_daemon_tool_list_envelope_sha256",
        "protected_daemon_tool_list_envelope_validated",
        "inputs_unchanged",
        "evaluation_digest",
        "participation_receipt_digest",
        "participation_signature_digest",
        "execution_policy_receipt_digest",
        "execution_policy_integrity_passed",
        "execution_policy_integrity_violations",
    }
)
_V1_LEGACY_APPARATUS_FIELDS = APPARATUS_FIELDS - {
    "protected_report_digest",
    "protected_tool_contract_validated",
    "observed_daemon_tool_schemas_sha256",
    "expected_daemon_tool_list_envelope_sha256",
    "observed_daemon_tool_list_envelope_sha256",
    "protected_daemon_tool_list_envelope_validated",
}

PROVIDER_PROXY_FIELDS = frozenset(
    {
        "enforcer",
        "schema_version",
        "trial_id",
        "endpoint",
        "allowed_client_ip",
        "provider_allowlist_sha256",
        "initial_client_binding_digest",
        "sealed_client_binding_digest",
        "initial_implementation_digest",
        "sealed_implementation_digest",
        "implementation_identity",
        "initial_evidence_digest",
        "sealed_evidence_digest",
        "artifact_sha256",
        "accepted_connections",
        "rejected_connections",
        "bytes_guest_to_provider",
        "bytes_provider_to_guest",
        "transcript_chain_digest",
        "active",
        "sealed",
    }
)

BINDING_FIELDS = frozenset(
    {
        "trial_id",
        "task_digest",
        "system_digest",
        "execution_policy_digest",
        "config_digest",
        "inputs_manifest_digest",
        "seed_provenance_digest",
        "agent_digest",
        "evaluator_digest",
    }
)


def _is_digest(value: object) -> bool:
    return isinstance(value, str) and _DIGEST.fullmatch(value) is not None


def _check(
    checks: list[dict[str, str]],
    reasons: list[str],
    check_id: str,
    passed: bool,
    *,
    unavailable: bool = False,
) -> None:
    status = "passed" if passed else ("unavailable" if unavailable else "failed")
    checks.append({"id": check_id, "status": status})
    if not passed:
        reasons.append(check_id)


def normalize_bindings(value: Mapping[str, Any]) -> dict[str, str | None]:
    """Validate exact final-receipt bindings without inventing missing systems."""

    if not isinstance(value, Mapping) or set(value) != BINDING_FIELDS:
        raise ValidationFailure("certification receipt bindings have unsupported fields")
    trial_id = value.get("trial_id")
    if not isinstance(trial_id, str) or not trial_id:
        raise ValidationFailure("certification trial binding must be non-empty")
    normalized: dict[str, str | None] = {"trial_id": trial_id}
    for field in sorted(BINDING_FIELDS - {"trial_id"}):
        item = value.get(field)
        if field in {"system_digest", "execution_policy_digest"} and item is None:
            normalized[field] = None
            continue
        if not _is_digest(item):
            raise ValidationFailure(f"certification binding {field} must be sha256:<hex>")
        normalized[field] = str(item)
    return normalized


def evaluate_apparatus(apparatus: Mapping[str, Any]) -> dict[str, Any]:
    """Return the deterministic apparatus decision and stable reason codes."""

    if not isinstance(apparatus, Mapping) or frozenset(apparatus) not in {
        APPARATUS_FIELDS,
        _V1_LEGACY_APPARATUS_FIELDS,
    }:
        raise ValidationFailure("certification apparatus has unsupported fields")
    checks: list[dict[str, str]] = []
    reasons: list[str] = []

    _check(
        checks,
        reasons,
        "environment_adapter_not_certifying",
        apparatus.get("environment_adapter") == "lume-macos-certifying"
        and apparatus.get("environment_declared_certifying") is True,
    )
    _check(
        checks,
        reasons,
        "apparatus_check_trial",
        apparatus.get("apparatus_check") is False,
    )
    _check(checks, reasons, "cleanup_incomplete", apparatus.get("cleanup_ok") is True)
    _check(
        checks,
        reasons,
        "seed_provenance_binding_unavailable",
        _is_digest(apparatus.get("seed_provenance_digest")),
        unavailable=apparatus.get("seed_provenance_digest") is None,
    )
    pristine = apparatus.get("pristine_fingerprint")
    post_run = apparatus.get("post_run_fingerprint")
    drift_available = _is_digest(pristine) and _is_digest(post_run)
    _check(
        checks,
        reasons,
        "post_run_drift_unavailable",
        drift_available,
        unavailable=not drift_available,
    )
    _check(
        checks,
        reasons,
        "post_run_drift_detected",
        bool(
            drift_available
            and pristine == post_run
            and apparatus.get("post_run_matches_pristine") is True
        ),
        unavailable=not drift_available,
    )
    initial_driver = apparatus.get("initial_driver_identity_digest")
    post_driver = apparatus.get("post_run_driver_identity_digest")
    driver_available = _is_digest(initial_driver) and _is_digest(post_driver)
    _check(
        checks,
        reasons,
        "driver_identity_unavailable",
        driver_available,
        unavailable=not driver_available,
    )
    _check(
        checks,
        reasons,
        "driver_identity_changed",
        bool(driver_available and initial_driver == post_driver),
        unavailable=not driver_available,
    )
    _check(
        checks,
        reasons,
        "workspace_not_fresh",
        apparatus.get("fresh_harness_workspace") is True,
    )
    _check(
        checks,
        reasons,
        "target_reset_unverified",
        apparatus.get("target_reset") is True,
    )
    task_store_relative = apparatus.get("task_store_relative")
    _check(
        checks,
        reasons,
        "task_store_binding_unavailable",
        bool(
            isinstance(task_store_relative, str)
            and task_store_relative
            and not task_store_relative.startswith("/")
            and ".." not in task_store_relative.split("/")
            and "\\" not in task_store_relative
        ),
        unavailable=task_store_relative is None,
    )
    _check(
        checks,
        reasons,
        "agent_freeze_unverified",
        apparatus.get("agent_processes_frozen") is True,
    )

    network_digests = (
        apparatus.get("network_evidence_digest"),
        apparatus.get("post_run_network_evidence_digest"),
    )
    network_available = all(_is_digest(value) for value in network_digests)
    _check(
        checks,
        reasons,
        "network_enforcement_unavailable",
        bool(
            apparatus.get("network_mode") in {"none", "allowlist"}
            and apparatus.get("network_enforcer") == "guest-root-pf-anchor"
            and network_available
        ),
        unavailable=not network_available,
    )
    _check(
        checks,
        reasons,
        "network_enforcement_changed",
        bool(network_available and network_digests[0] == network_digests[1]),
        unavailable=not network_available,
    )

    production_harness = apparatus.get("production_harness")
    production_supported = production_harness in {
        None,
        "codex",
        "claude-code",
        "opencode",
    }
    _check(
        checks,
        reasons,
        "production_harness_unsupported",
        production_supported,
    )
    provider_proxy = apparatus.get("provider_proxy")
    if production_harness is None:
        _check(
            checks,
            reasons,
            "unexpected_provider_proxy",
            provider_proxy is None,
        )
    else:
        provider_available = isinstance(provider_proxy, Mapping)
        provider_valid = bool(
            provider_available
            and set(provider_proxy) == PROVIDER_PROXY_FIELDS
            and provider_proxy.get("enforcer") == "host-cdb-connect-proxy"
            and provider_proxy.get("schema_version") == 1
            and isinstance(provider_proxy.get("trial_id"), str)
            and bool(provider_proxy.get("trial_id"))
            and isinstance(provider_proxy.get("endpoint"), str)
            and provider_proxy["endpoint"].startswith("http://")
            and isinstance(provider_proxy.get("allowed_client_ip"), str)
            and bool(provider_proxy.get("allowed_client_ip"))
            and provider_proxy.get("implementation_identity") == "cb.provider-connect-proxy/v2"
            and provider_proxy.get("active") is False
            and provider_proxy.get("sealed") is True
            and all(
                _is_digest(provider_proxy.get(field))
                for field in (
                    "provider_allowlist_sha256",
                    "initial_client_binding_digest",
                    "sealed_client_binding_digest",
                    "initial_implementation_digest",
                    "sealed_implementation_digest",
                    "initial_evidence_digest",
                    "sealed_evidence_digest",
                    "artifact_sha256",
                    "transcript_chain_digest",
                )
            )
            and provider_proxy.get("initial_client_binding_digest")
            == provider_proxy.get("sealed_client_binding_digest")
            and provider_proxy.get("initial_implementation_digest")
            == provider_proxy.get("sealed_implementation_digest")
            and all(
                isinstance(provider_proxy.get(field), int)
                and not isinstance(provider_proxy[field], bool)
                and provider_proxy[field] >= 0
                for field in (
                    "accepted_connections",
                    "rejected_connections",
                    "bytes_guest_to_provider",
                    "bytes_provider_to_guest",
                )
            )
            and provider_proxy.get("accepted_connections", 0) >= 1
            and provider_proxy.get("bytes_guest_to_provider", 0) >= 1
            and provider_proxy.get("bytes_provider_to_guest", 0) >= 1
        )
        _check(
            checks,
            reasons,
            "provider_proxy_unavailable",
            provider_valid,
            unavailable=not provider_available,
        )
        report_digest = apparatus.get("protected_report_digest")
        observed_tool_digest = apparatus.get("observed_daemon_tool_schemas_sha256")
        expected_list_result_digest = apparatus.get("expected_daemon_tool_list_envelope_sha256")
        observed_list_result_digest = apparatus.get("observed_daemon_tool_list_envelope_sha256")
        tool_contract_available = bool(
            _is_digest(report_digest)
            and apparatus.get("protected_tool_contract_validated") is True
            and isinstance(observed_tool_digest, str)
            and _HEX_DIGEST.fullmatch(observed_tool_digest)
            and apparatus.get("protected_daemon_tool_list_envelope_validated") is True
            and isinstance(expected_list_result_digest, str)
            and _HEX_DIGEST.fullmatch(expected_list_result_digest)
            and observed_list_result_digest == expected_list_result_digest
        )
        _check(
            checks,
            reasons,
            "mediator_tool_contract_unavailable",
            tool_contract_available,
            unavailable=(
                report_digest is None
                or apparatus.get("protected_tool_contract_validated") is None
                or observed_tool_digest is None
                or apparatus.get("protected_daemon_tool_list_envelope_validated") is None
                or expected_list_result_digest is None
                or observed_list_result_digest is None
            ),
        )

    human_digests = (
        apparatus.get("human_input_evidence_digest"),
        apparatus.get("post_run_human_input_evidence_digest"),
    )
    human_available = all(_is_digest(value) for value in human_digests)
    _check(
        checks,
        reasons,
        "human_input_enforcement_unavailable",
        bool(
            apparatus.get("human_input_channel") == "closed-no-vnc"
            and apparatus.get("human_input_enforcer") == "host-lume-vnc-disabled"
            and human_available
        ),
        unavailable=not human_available,
    )
    _check(
        checks,
        reasons,
        "human_input_enforcement_changed",
        bool(human_available and human_digests[0] == human_digests[1]),
        unavailable=not human_available,
    )

    endpoint_digests = (
        apparatus.get("protected_endpoint_binding_digest"),
        apparatus.get("sealed_endpoint_binding_digest"),
    )
    endpoint_available = all(_is_digest(value) for value in endpoint_digests)
    _check(
        checks,
        reasons,
        "protected_endpoint_unavailable",
        endpoint_available,
        unavailable=not endpoint_available,
    )
    _check(
        checks,
        reasons,
        "protected_endpoint_changed",
        bool(endpoint_available and endpoint_digests[0] == endpoint_digests[1]),
        unavailable=not endpoint_available,
    )
    _check(
        checks,
        reasons,
        "vm_not_stopped_before_collection",
        apparatus.get("vm_stopped_before_collection") is True,
    )
    _check(
        checks,
        reasons,
        "collection_not_read_only",
        apparatus.get("protected_collection_read_only") is True,
    )
    for field, reason in (
        ("collection_manifest_digest", "collection_manifest_unavailable"),
        (
            "protected_collection_manifest_digest",
            "protected_collection_manifest_unavailable",
        ),
        ("protected_log_digest", "protected_log_unavailable"),
    ):
        _check(
            checks,
            reasons,
            reason,
            _is_digest(apparatus.get(field)),
            unavailable=apparatus.get(field) is None,
        )

    integrity_violations = apparatus.get("execution_policy_integrity_violations")
    integrity_shape = (
        isinstance(integrity_violations, list)
        and all(isinstance(value, str) and value for value in integrity_violations)
        and len(integrity_violations) == len(set(integrity_violations))
    )
    _check(
        checks,
        reasons,
        "execution_policy_integrity_failed",
        bool(
            integrity_shape
            and apparatus.get("execution_policy_integrity_passed") is True
            and not integrity_violations
        ),
        unavailable=not integrity_shape,
    )
    log_tail = apparatus.get("protected_log_tail")
    records = apparatus.get("protected_log_records")
    _check(
        checks,
        reasons,
        "protected_log_chain_unavailable",
        bool(
            isinstance(log_tail, str)
            and _HEX_DIGEST.fullmatch(log_tail)
            and isinstance(records, int)
            and not isinstance(records, bool)
            and records > 0
        ),
        unavailable=log_tail is None or records is None,
    )
    _check(
        checks,
        reasons,
        "protected_transport_integrity_failed",
        apparatus.get("protected_transport_integrity") is True,
    )
    _check(
        checks,
        reasons,
        "protected_evidence_incomplete",
        apparatus.get("protected_evidence_complete") is True,
    )
    _check(
        checks,
        reasons,
        "off_target_driver_activity",
        apparatus.get("protected_off_target_activity") is False,
    )
    _check(
        checks,
        reasons,
        "runtime_inputs_changed",
        apparatus.get("inputs_unchanged") is True,
    )
    for field, reason in (
        ("evaluation_digest", "evaluation_unavailable"),
        ("participation_receipt_digest", "participation_receipt_unavailable"),
        ("participation_signature_digest", "participation_signature_unavailable"),
        (
            "execution_policy_receipt_digest",
            "execution_policy_receipt_unavailable",
        ),
    ):
        _check(
            checks,
            reasons,
            reason,
            _is_digest(apparatus.get(field)),
            unavailable=apparatus.get(field) is None,
        )

    return {
        "status": (
            "passed"
            if not reasons
            else (
                "failed" if any(check["status"] == "failed" for check in checks) else "unavailable"
            )
        ),
        "eligible": not reasons,
        "reasons": reasons,
        "checks": checks,
    }


def evaluate_bindings(bindings: Mapping[str, Any], apparatus: Mapping[str, Any]) -> dict[str, Any]:
    """Require decision-bearing bindings and the same seed provenance fact."""

    normalized = normalize_bindings(bindings)
    checks: list[dict[str, str]] = []
    reasons: list[str] = []
    _check(
        checks,
        reasons,
        "system_binding_unavailable",
        _is_digest(normalized.get("system_digest")),
        unavailable=normalized.get("system_digest") is None,
    )
    _check(
        checks,
        reasons,
        "execution_policy_binding_unavailable",
        _is_digest(normalized.get("execution_policy_digest")),
        unavailable=normalized.get("execution_policy_digest") is None,
    )
    _check(
        checks,
        reasons,
        "seed_provenance_binding_mismatch",
        normalized.get("seed_provenance_digest") == apparatus.get("seed_provenance_digest"),
    )
    provider_proxy = apparatus.get("provider_proxy")
    if apparatus.get("production_harness") is not None:
        _check(
            checks,
            reasons,
            "provider_proxy_trial_binding_mismatch",
            isinstance(provider_proxy, Mapping)
            and provider_proxy.get("trial_id") == normalized["trial_id"],
        )
    return {
        "status": (
            "passed"
            if not reasons
            else (
                "failed" if any(check["status"] == "failed" for check in checks) else "unavailable"
            )
        ),
        "eligible": not reasons,
        "reasons": reasons,
        "checks": checks,
    }


def build_certification_receipt(
    *,
    bindings: Mapping[str, Any],
    apparatus: Mapping[str, Any],
    outcome: Mapping[str, Any] | None,
    participation: Mapping[str, Any],
    comparison: Mapping[str, Any] | None,
) -> dict[str, Any]:
    """Compose all four decisions without allowing one to rewrite another."""

    normalized_bindings = normalize_bindings(bindings)
    apparatus_body = dict(apparatus)
    apparatus_decision = evaluate_apparatus(apparatus_body)
    binding_decision = evaluate_bindings(normalized_bindings, apparatus_body)
    participation_eligible = bool(
        participation.get("required") is not True
        or (
            participation.get("passed") is True
            and participation.get("observer", {}).get("trust") == "certifying"
        )
    )
    participation_decision = {
        "required": participation.get("required"),
        "status": participation.get("status", "unavailable"),
        "receipt_passed": participation.get("passed"),
        "observer_trust": participation.get("observer", {}).get("trust"),
        "eligible": participation_eligible,
        "receipt_digest": participation.get("receipt_digest"),
    }
    outcome_decision = {
        "available": outcome is not None,
        "passed": outcome.get("passed") if outcome is not None else None,
        "score": outcome.get("score") if outcome is not None else None,
        "digest": digest_json(outcome) if outcome is not None else None,
    }
    comparison_decision = {
        "eligible": bool(comparison and comparison.get("eligible") is True),
        "status": comparison.get("status", "unavailable")
        if comparison is not None
        else "unavailable",
        "receipt_digest": comparison.get("receipt_digest") if comparison is not None else None,
    }
    body = {
        "schema_version": SCHEMA_VERSION,
        "bindings": normalized_bindings,
        "apparatus": apparatus_body,
        "binding_decision": binding_decision,
        "apparatus_decision": apparatus_decision,
        "outcome": outcome_decision,
        "participation": participation_decision,
        "comparison": comparison_decision,
        "eligible": (
            binding_decision["eligible"]
            and apparatus_decision["eligible"]
            and participation_eligible
        ),
    }
    return {**body, "receipt_digest": digest_json(body)}


def verify_certification_receipt(receipt: Mapping[str, Any]) -> dict[str, Any]:
    """Recompute the signed composition without trusting its decision fields."""

    if not isinstance(receipt, Mapping):
        raise ValidationFailure("certification receipt is not an object")
    expected_fields = {
        "schema_version",
        "bindings",
        "apparatus",
        "binding_decision",
        "apparatus_decision",
        "outcome",
        "participation",
        "comparison",
        "eligible",
        "receipt_digest",
    }
    if set(receipt) != expected_fields or receipt.get("schema_version") != SCHEMA_VERSION:
        raise ValidationFailure("certification receipt shape is unsupported")
    body = {key: value for key, value in receipt.items() if key != "receipt_digest"}
    if receipt.get("receipt_digest") != digest_json(body):
        raise ValidationFailure("certification receipt digest mismatch")
    normalized_bindings = normalize_bindings(receipt["bindings"])
    decision = evaluate_apparatus(receipt["apparatus"])
    if decision != receipt.get("apparatus_decision"):
        raise ValidationFailure("certification apparatus decision mismatch")
    binding_decision = evaluate_bindings(normalized_bindings, receipt["apparatus"])
    if binding_decision != receipt.get("binding_decision"):
        raise ValidationFailure("certification binding decision mismatch")
    outcome = receipt.get("outcome")
    if (
        not isinstance(outcome, Mapping)
        or set(outcome)
        != {
            "available",
            "passed",
            "score",
            "digest",
        }
        or not isinstance(outcome.get("available"), bool)
    ):
        raise ValidationFailure("certification outcome decision is invalid")
    if outcome["available"]:
        if (
            not isinstance(outcome.get("passed"), bool)
            or not isinstance(outcome.get("score"), (int, float))
            or isinstance(outcome.get("score"), bool)
            or not math.isfinite(outcome["score"])
            or not _is_digest(outcome.get("digest"))
        ):
            raise ValidationFailure("certification outcome decision is invalid")
    elif any(outcome.get(field) is not None for field in ("passed", "score", "digest")):
        raise ValidationFailure("unavailable certification outcome has values")
    if receipt["apparatus"].get("evaluation_digest") != outcome.get("digest"):
        raise ValidationFailure("certification outcome digest binding mismatch")
    participation = receipt.get("participation")
    if (
        not isinstance(participation, Mapping)
        or set(participation)
        != {
            "required",
            "status",
            "receipt_passed",
            "observer_trust",
            "eligible",
            "receipt_digest",
        }
        or not isinstance(participation.get("required"), bool)
        or participation.get("status") not in {"passed", "failed", "unavailable", "not_required"}
        or not (
            isinstance(participation.get("receipt_passed"), bool)
            or participation.get("receipt_passed") is None
        )
        or participation.get("observer_trust")
        not in {"certifying", "non_certifying", "unavailable"}
        or not isinstance(participation.get("eligible"), bool)
        or not _is_digest(participation.get("receipt_digest"))
    ):
        raise ValidationFailure("certification participation decision is invalid")
    expected_receipt_passed = {
        "passed": True,
        "failed": False,
        "unavailable": None,
        "not_required": None,
    }[participation["status"]]
    if participation["receipt_passed"] is not expected_receipt_passed:
        raise ValidationFailure("certification participation status mismatch")
    expected_participation_eligible = bool(
        participation["required"] is not True
        or (
            participation["receipt_passed"] is True
            and participation["observer_trust"] == "certifying"
        )
    )
    if participation["eligible"] is not expected_participation_eligible:
        raise ValidationFailure("certification participation eligibility mismatch")
    if receipt["apparatus"].get("participation_receipt_digest") != participation["receipt_digest"]:
        raise ValidationFailure("certification participation digest binding mismatch")
    comparison = receipt.get("comparison")
    if (
        not isinstance(comparison, Mapping)
        or set(comparison) != {"eligible", "status", "receipt_digest"}
        or not isinstance(comparison.get("eligible"), bool)
        or comparison.get("status") not in {"passed", "failed", "unavailable"}
        or (
            comparison.get("receipt_digest") is not None
            and not _is_digest(comparison.get("receipt_digest"))
        )
        or (comparison.get("eligible") is not (comparison.get("status") == "passed"))
    ):
        raise ValidationFailure("certification comparison decision is invalid")
    if receipt["apparatus"].get("execution_policy_receipt_digest") != comparison.get(
        "receipt_digest"
    ):
        raise ValidationFailure("certification comparison digest binding mismatch")
    expected_eligible = (
        binding_decision["eligible"] and decision["eligible"] and participation["eligible"]
    )
    if receipt.get("eligible") is not expected_eligible:
        raise ValidationFailure("certification eligibility mismatch")
    return dict(receipt)
