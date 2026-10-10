"""Export a verified runtime trial into a frozen v0.3 trial manifest."""

from __future__ import annotations

import copy
import json
import os
import tempfile
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from cua_bench_runtime.canon import canonical_json, digest_file, sha256_bytes
from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.events import verify_event_log
from cua_bench_runtime.explain import inspect_trial
from cua_bench_runtime.report import normalize_trials
from cua_bench_runtime.schemas import load_json, validate_manifest


def _bare(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise ValidationFailure(f"{label} is missing")
    bare = value.removeprefix("sha256:")
    if len(bare) != 64 or any(character not in "0123456789abcdef" for character in bare):
        raise ValidationFailure(f"{label} is not a SHA-256 digest")
    return bare


def _require_equal(actual: object, expected: object, label: str) -> None:
    if actual != expected:
        raise ValidationFailure(f"trial template contradicts verified {label}")


def _safe_source(root: Path, relative: object, label: str) -> Path:
    if not isinstance(relative, str) or not relative or "\\" in relative:
        raise ValidationFailure(f"{label} artifact path is invalid")
    root = root.resolve()
    candidate = root / relative
    try:
        parts = candidate.relative_to(root).parts
    except ValueError as error:
        raise ValidationFailure(f"{label} artifact path is invalid") from error
    current = root
    for part in parts:
        current /= part
        if current.is_symlink():
            raise ValidationFailure(f"{label} artifact must not be a symlink")
    source = candidate.resolve()
    if not source.is_relative_to(root) or not source.is_file() or source.is_symlink():
        raise ValidationFailure(f"{label} artifact is unavailable: {relative}")
    return source


def _artifact(source: Path, out: Path, label: str) -> tuple[dict[str, str], tuple[Path, Path, str]]:
    digest = _bare(digest_file(source), f"{label} digest")
    safe_name = (
        "".join(
            character if character.isalnum() or character in ".-_" else "_"
            for character in source.name
        )
        or "artifact"
    )
    relative = Path(f"{out.name}.artifacts") / f"{digest}-{safe_name}"
    return (
        {"path": relative.as_posix(), "sha256": digest},
        (source, out.parent / relative, digest),
    )


def _rewrite_template_artifacts(
    value: Any,
    template_root: Path,
    out: Path,
    copies: list[tuple[Path | bytes, Path, str]],
) -> None:
    if isinstance(value, dict):
        if set(value) == {"path", "sha256"}:
            source = _safe_source(template_root, value["path"], "template")
            actual = _bare(digest_file(source), "template artifact digest")
            _require_equal(actual, value["sha256"], "template artifact hash")
            descriptor, copy_item = _artifact(source, out, "template")
            value.clear()
            value.update(descriptor)
            copies.append(copy_item)
            return
        for child in value.values():
            _rewrite_template_artifacts(child, template_root, out, copies)
    elif isinstance(value, list):
        for child in value:
            _rewrite_template_artifacts(child, template_root, out, copies)


def _write_generated(
    value: Mapping[str, Any], name: str, out: Path
) -> tuple[dict[str, str], tuple[bytes, Path, str]]:
    payload = canonical_json(dict(value)) + b"\n"
    digest = sha256_bytes(payload, prefix=False)
    safe_name = (
        "".join(
            character if character.isalnum() or character in ".-_" else "_" for character in name
        )
        or "artifact"
    )
    relative = Path(f"{out.name}.artifacts") / f"{digest}-{safe_name}"
    return (
        {"path": relative.as_posix(), "sha256": digest},
        (payload, out.parent / relative, digest),
    )


def _copy_atomic(source: Path | bytes, destination: Path, expected_digest: str) -> None:
    payload = source if isinstance(source, bytes) else source.read_bytes()
    actual_digest = sha256_bytes(payload, prefix=False)
    if actual_digest != expected_digest:
        raise ValidationFailure(f"export artifact source changed: {destination.name}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        if (
            destination.is_symlink()
            or not destination.is_file()
            or _bare(digest_file(destination), "existing artifact digest") != expected_digest
        ):
            raise ValidationFailure(f"export artifact destination conflicts: {destination}")
        return
    temporary_fd, temporary_name = tempfile.mkstemp(
        prefix=".cb-export-copy-", dir=destination.parent
    )
    os.close(temporary_fd)
    temporary = Path(temporary_name)
    try:
        temporary.write_bytes(payload)
        os.replace(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)


def _observed(policy_observed: Mapping[str, Any], declared: Mapping[str, Any]) -> dict[str, Any]:
    observed = copy.deepcopy(dict(policy_observed))
    observed.pop("cost_usd", None)
    observed.pop("credential_state_profile_sha256", None)
    observed.pop("enforcement", None)
    for field in ("fresh_harness_workspace", "target_reset"):
        descriptor = declared.get(field, {}).get("evidence")
        if not isinstance(descriptor, Mapping):
            raise ValidationFailure(f"trial template must predeclare {field} evidence")
        observed[field] = {
            "satisfied": policy_observed.get(field) is True,
            "evidence": copy.deepcopy(dict(descriptor)),
        }
    for field in ("cache_state", "persistent_state"):
        descriptor = declared.get(field, {}).get("evidence")
        if not isinstance(descriptor, Mapping):
            raise ValidationFailure(f"trial template must predeclare {field} evidence")
        observed[field] = {
            "mode": policy_observed.get(field, "unknown"),
            "evidence": copy.deepcopy(dict(descriptor)),
        }
    if observed.get("applied_network_mode") != "allowlist":
        observed.pop("applied_network_allowlist_sha256", None)
    return observed


def _termination(status: object) -> str:
    return {
        "completed": "completed",
        "timeout": "timeout",
        "cost_limit": "cost_limit",
        "reset_failure": "reset_failure",
        "agent_stop": "agent_stop",
    }.get(status, "infrastructure_error")


def export_trial(trial_dir: Path, template_path: Path, out: Path) -> dict[str, Any]:
    """Export *trial_dir* by overlaying verified runtime data on *template_path*."""

    trial_dir = trial_dir.resolve()
    template_path = template_path.resolve()
    requested_out = out.absolute()
    if requested_out.is_symlink():
        raise ValidationFailure(f"output must not be a symlink: {requested_out}")
    requested_out.parent.mkdir(parents=True, exist_ok=True)
    out = requested_out.parent.resolve() / requested_out.name
    artifact_dir = out.parent / f"{out.name}.artifacts"
    if out.exists() or artifact_dir.exists() or artifact_dir.is_symlink():
        raise ValidationFailure(
            "export destination and its artifact directory must not already exist"
        )

    verified = inspect_trial(trial_dir)
    config = load_json(trial_dir / "config.json")
    result = load_json(trial_dir / "result.json")
    template = load_json(template_path)
    if not isinstance(template, dict):
        raise ValidationFailure("trial template is not an object")
    if template.get("schema_version") != "0.3.0":
        raise ValidationFailure("trial template must use schema_version 0.3.0")
    if template.get("eligibility_status") != "eligible" or template.get("pre_freeze") is not False:
        raise ValidationFailure("trial template must be a frozen eligible execution template")
    if result.get("schema_version") != "0.3.0" or config.get("schema_version") != "0.3.0":
        raise ValidationFailure("export requires a v0.3 runtime trial")
    if result.get("apparatus_check") is True:
        raise ValidationFailure("apparatus-check trials cannot be exported for reports")
    if config.get("debug_mode") is True or result.get("debug_mode") is True:
        raise ValidationFailure("debug-mode trials cannot be exported for reports")

    _require_equal(template.get("id"), result.get("trial_id"), "trial id")
    _require_equal(config.get("trial_id"), result.get("trial_id"), "trial id")
    for field in ("task", "system", "execution_policy"):
        configured = config.get(field)
        declared = template.get(field)
        if not isinstance(configured, Mapping) or not isinstance(declared, Mapping):
            raise ValidationFailure(f"verified {field} binding is missing")
        _require_equal(declared.get("id"), configured.get("id"), f"{field} id")
        _require_equal(declared.get("version"), configured.get("version"), f"{field} version")
        _require_equal(
            declared.get("manifest_sha256"),
            _bare(configured.get("digest"), f"{field} digest"),
            f"{field} digest",
        )
    _require_equal(template.get("variant"), config.get("variant"), "variant")
    bindings = template.get("bindings")
    if not isinstance(bindings, Mapping):
        raise ValidationFailure("trial template has no frozen bindings")
    _require_equal(
        bindings.get("system_digest"),
        template["system"]["manifest_sha256"],
        "system binding",
    )
    _require_equal(
        bindings.get("execution_policy_digest"),
        template["execution_policy"]["manifest_sha256"],
        "execution-policy binding",
    )
    _require_equal(
        bindings.get("dataset_freeze_digest"),
        template.get("dataset", {}).get("freeze_digest"),
        "dataset freeze binding",
    )

    policy = verified.get("execution_policy")
    if not isinstance(policy, Mapping) or not isinstance(policy.get("observed"), Mapping):
        raise ValidationFailure("verified trial has no exportable execution-policy observations")
    eligible = policy.get("eligible") is True
    violations = policy.get("violations")
    if not isinstance(violations, list) or any(
        not isinstance(item, str) or not item for item in violations
    ):
        raise ValidationFailure("execution-policy violations are invalid")
    if eligible and violations:
        raise ValidationFailure("eligible execution-policy receipt has violations")
    if not eligible and not violations:
        raise ValidationFailure("ineligible execution-policy receipt has no violations")
    accounting = policy["observed"]
    accounting_tokens = accounting.get("tokens")
    if eligible and (
        not isinstance(accounting_tokens, Mapping)
        or accounting_tokens.get("includes_subagents") is not True
        or accounting.get("cost_usd") is None
        or any(call.get("tokens") is None for call in accounting.get("model_calls", []))
    ):
        raise ValidationFailure(
            "eligible trial is missing complete subagent-inclusive accounting telemetry"
        )

    manifest = copy.deepcopy(template)
    copies: list[tuple[Path | bytes, Path, str]] = []
    _rewrite_template_artifacts(manifest, template_path.parent, out, copies)
    policy_descriptor, policy_copy = _write_generated(policy, "execution-policy-receipt.json", out)
    copies.append(policy_copy)
    declared_execution = manifest.get("execution")
    if not isinstance(declared_execution, Mapping):
        raise ValidationFailure("trial template must contain a complete execution section")
    events = verify_event_log(trial_dir / "events.ndjson")
    event_descriptor, event_copy = _artifact(trial_dir / "events.ndjson", out, "event log")
    copies.append(event_copy)
    execution = copy.deepcopy(dict(declared_execution))
    execution.update(
        {
            "agent_execution_id": result["trial_id"],
            "started_at": events[0]["ts_utc"],
            "ended_at": events[-1]["ts_utc"],
            "event_log": event_descriptor,
            "observed": _observed(accounting, declared_execution.get("observed", {})),
            "comparison_eligibility": {
                "status": "eligible" if eligible else "ineligible",
                "reasons": [] if eligible else list(violations),
            },
        }
    )

    participation = verified.get("participation")
    if not isinstance(participation, Mapping):
        raise ValidationFailure("verified trial has no participation receipt")
    participation_value = copy.deepcopy(dict(participation))
    descriptor, copy_item = _write_generated(participation_value, "participation-receipt.json", out)
    participation_value["receipt_artifact"] = descriptor
    copies.append(copy_item)
    execution["participation"] = participation_value
    participation_signature_descriptor: dict[str, str] | None = None
    participation_signature = verified.get("participation_signature")
    if isinstance(participation_signature, Mapping):
        signature_source = _safe_source(
            trial_dir, participation_signature.get("path"), "participation signature"
        )
        _require_equal(
            digest_file(signature_source),
            participation_signature.get("digest"),
            "participation signature digest",
        )
        participation_signature_descriptor, signature_copy = _artifact(
            signature_source, out, "participation signature"
        )
        copies.append(signature_copy)

    certification = verified.get("apparatus_certification")
    declared_certification = declared_execution.get("certification")
    if not isinstance(declared_certification, Mapping):
        raise ValidationFailure("trial template must predeclare certification bindings")
    certification_value = copy.deepcopy(dict(declared_certification))
    certification_value["bindings"] = {
        "trial_id": result["trial_id"],
        "task_digest": template["task"]["manifest_sha256"],
        "system_digest": template["system"]["manifest_sha256"],
        "execution_policy_digest": template["execution_policy"]["manifest_sha256"],
        "resolved_seed_provenance_sha256": template["environment"][
            "resolved_seed_provenance_sha256"
        ],
        "apparatus_digest": bindings["apparatus_digest"],
    }
    if isinstance(certification, Mapping):
        receipt_bindings = certification.get("bindings")
        if not isinstance(receipt_bindings, Mapping):
            raise ValidationFailure("apparatus certification has no bindings")
        for runtime_field, expected, label in (
            ("trial_id", result["trial_id"], "trial id"),
            ("task_digest", config["task"]["digest"], "task digest"),
            ("system_digest", config["system"]["digest"], "system digest"),
            (
                "execution_policy_digest",
                config["execution_policy"]["digest"],
                "execution-policy digest",
            ),
            (
                "seed_provenance_digest",
                "sha256:" + template["environment"]["resolved_seed_provenance_sha256"],
                "seed provenance digest",
            ),
        ):
            _require_equal(receipt_bindings.get(runtime_field), expected, f"certification {label}")
        decision = certification.get("apparatus_decision", {})
        status = decision.get("status", "unavailable")
        certification_value.update(
            {
                "apparatus_status": status,
                "certifying": verified.get("certifying") is True,
                "reasons": (
                    []
                    if verified.get("certifying") is True
                    else list(decision.get("reasons") or ["apparatus_not_certifying"])
                ),
            }
        )
        receipt_descriptor, receipt_copy = _write_generated(
            certification, "apparatus-certification-receipt.json", out
        )
        certification_value["receipt"] = receipt_descriptor
        copies.append(receipt_copy)
        signature = verified.get("apparatus_certification_signature")
        if isinstance(signature, Mapping):
            signature_source = _safe_source(trial_dir, signature.get("path"), "apparatus signature")
            _require_equal(
                digest_file(signature_source),
                signature.get("digest"),
                "apparatus signature digest",
            )
            signature_descriptor, signature_copy = _artifact(
                signature_source, out, "apparatus signature"
            )
            certification_value["signature"] = signature_descriptor
            certification_value["verifier_key_id"] = signature.get("key_id")
            copies.append(signature_copy)
    else:
        certification_value.update(
            {
                "apparatus_status": "unavailable",
                "certifying": False,
                "reasons": ["apparatus_certification_unavailable"],
            }
        )
        for field in ("receipt", "signature", "verifier_key_id"):
            certification_value.pop(field, None)
    execution["certification"] = certification_value
    manifest["execution"] = execution

    evaluation = verified.get("evaluation")
    if not isinstance(evaluation, Mapping) or not isinstance(evaluation.get("passed"), bool):
        raise ValidationFailure("completed trial has no evaluation decision")
    termination_status = _termination(result.get("status"))
    manifest["termination_status"] = termination_status
    evaluation_descriptor, evaluation_copy = _write_generated(evaluation, "evaluation.json", out)
    copies.append(evaluation_copy)
    manifest["outcomes"] = {
        "completion": (
            evaluation["passed"] if termination_status != "infrastructure_error" else False
        ),
        "forbidden_side_effects": [],
        "wrong_target_mutations": [],
        "leaked_input": [],
    }
    declared_observables = manifest.get("observables")
    if not isinstance(declared_observables, Mapping):
        raise ValidationFailure("trial template must contain observables")
    observables = copy.deepcopy(dict(declared_observables))
    observables["wall_time_ms"] = result.get("elapsed_ms")
    observables["cost_usd"] = accounting.get("cost_usd") if eligible else None
    tokens = accounting_tokens
    if not eligible:
        observables["tokens"] = None
    else:
        supplements = observables.get("tokens")
        if not isinstance(supplements, Mapping):
            raise ValidationFailure("trial template must predeclare supplemental token accounting")
        observables["tokens"] = {
            **{field: tokens[field] for field in ("input", "output", "cache_read", "cache_write")},
            **{
                field: supplements[field]
                for field in ("tool_schema", "driver_skill", "neutral_guidance")
            },
            "includes_subagents": tokens["includes_subagents"],
        }
    manifest["observables"] = observables
    manifest["truncated"] = result.get("status") in {"timeout", "cost_limit"}

    evidence = list(manifest.get("evidence", []))
    evidence.extend([event_descriptor, policy_descriptor, descriptor, evaluation_descriptor])
    if participation_signature_descriptor is not None:
        evidence.append(participation_signature_descriptor)
    evidence.extend(
        item
        for item in (
            certification_value.get("receipt"),
            certification_value.get("signature"),
        )
        if isinstance(item, dict)
    )
    manifest["evidence"] = list(
        {(item["path"], item["sha256"]): item for item in evidence}.values()
    )

    staging = tempfile.TemporaryDirectory(prefix=".cb-export-stage-", dir=out.parent)
    staging_root = Path(staging.name)
    staged_out = staging_root / out.name
    staged_artifact_dir = staging_root / artifact_dir.name
    staged_copies = [
        (source, staging_root / destination.relative_to(out.parent), digest)
        for source, destination, digest in copies
    ]
    published_artifacts = False
    try:
        staged_out.write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        validate_manifest(staged_out, "trial")
        for source, destination, expected_digest in staged_copies:
            _copy_atomic(source, destination, expected_digest)
        normalize_trials(
            [staged_out],
            [trial_dir / "inputs/system.cuabench.json"],
            [trial_dir / "inputs/execution-policy.cuabench.json"],
        )
        if not staged_artifact_dir.is_dir() or staged_artifact_dir.is_symlink():
            raise ValidationFailure("staged export artifact directory is unavailable")
        os.replace(staged_artifact_dir, artifact_dir)
        published_artifacts = True
        try:
            os.replace(staged_out, out)
        except OSError:
            os.replace(artifact_dir, staged_artifact_dir)
            published_artifacts = False
            raise
    finally:
        if published_artifacts and not out.exists():
            try:
                os.replace(artifact_dir, staged_artifact_dir)
            except OSError:
                pass
        staging.cleanup()
    return manifest
