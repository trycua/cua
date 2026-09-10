"""Deterministic comparison reports over frozen v0.3 trial manifests."""

from __future__ import annotations

import json
import math
import random
from collections import Counter, defaultdict
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from statistics import median
from typing import Any

from cua_bench_runtime.canon import digest_file, digest_json
from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.receipt_signing import key_id, verify_certification_signature
from cua_bench_runtime.schemas import validate_manifest


@dataclass(frozen=True)
class View:
    arm: str
    fixed: tuple[str, ...]
    paired: bool


VIEWS: dict[str, View] = {
    "system-track": View(
        "system_digest",
        (
            "task_digest",
            "variant",
            "dataset_freeze_digest",
            "execution_policy_digest",
            "resolved_seed_provenance_sha256",
            "apparatus_digest",
            "price_table_digest",
        ),
        False,
    ),
    "harness-comparison": View(
        "harness_digest",
        (
            "task_digest",
            "variant",
            "dataset_freeze_digest",
            "execution_policy_digest",
            "resolved_seed_provenance_sha256",
            "apparatus_digest",
            "price_table_digest",
            "model_routing_digest",
            "driver_candidate_digest",
            "driver_profile",
            "driver_tool_contract_digest",
            "driver_skill_mode",
        ),
        True,
    ),
    "model-comparison": View(
        "model_routing_digest",
        (
            "task_digest",
            "variant",
            "dataset_freeze_digest",
            "execution_policy_digest",
            "resolved_seed_provenance_sha256",
            "apparatus_digest",
            "price_table_digest",
            "harness_digest",
            "driver_candidate_digest",
            "driver_profile",
            "driver_tool_contract_digest",
            "driver_skill_mode",
        ),
        True,
    ),
    "driver-profile": View(
        "driver_profile",
        (
            "task_digest",
            "variant",
            "dataset_freeze_digest",
            "execution_policy_digest",
            "resolved_seed_provenance_sha256",
            "apparatus_digest",
            "price_table_digest",
            "harness_digest",
            "model_routing_digest",
            "driver_candidate_digest",
        ),
        True,
    ),
    "tool-surface": View(
        "driver_tool_contract_digest",
        (
            "task_digest",
            "variant",
            "dataset_freeze_digest",
            "execution_policy_digest",
            "resolved_seed_provenance_sha256",
            "apparatus_digest",
            "price_table_digest",
            "harness_digest",
            "model_routing_digest",
            "driver_candidate_digest",
            "driver_profile",
            "driver_skill_mode",
        ),
        True,
    ),
}


def _load_attached_json(
    trial_path: Path,
    artifact: Mapping[str, Any],
    label: str,
) -> Mapping[str, Any]:
    root = trial_path.parent.resolve()
    candidate = (root / str(artifact.get("path", ""))).resolve()
    if (
        not candidate.is_relative_to(root)
        or not candidate.is_file()
        or candidate.is_symlink()
        or digest_file(candidate).removeprefix("sha256:") != artifact.get("sha256")
    ):
        raise ValidationFailure(f"{trial_path}: {label} artifact mismatch")
    try:
        document = json.loads(candidate.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationFailure(f"{trial_path}: {label} is invalid JSON") from error
    if not isinstance(document, Mapping):
        raise ValidationFailure(f"{trial_path}: {label} is not an object")
    return document


INFRASTRUCTURE_TERMINATIONS = {"infrastructure_error", "reset_failure"}


def _bare_digest(value: Mapping[str, Any]) -> str:
    return digest_json(value).removeprefix("sha256:")


def load_systems(paths: Sequence[Path]) -> dict[str, dict[str, Any]]:
    systems: dict[str, dict[str, Any]] = {}
    for path in paths:
        system = validate_manifest(path, "system")
        raw_digest = digest_file(path.resolve()).removeprefix("sha256:")
        if raw_digest in systems:
            raise ValidationFailure(f"duplicate system manifest digest: {raw_digest}")
        systems[raw_digest] = system
    return systems


def load_policies(paths: Sequence[Path]) -> dict[str, dict[str, Any]]:
    policies: dict[str, dict[str, Any]] = {}
    for path in paths:
        policy = validate_manifest(path, "execution-policy")
        raw_digest = digest_file(path.resolve()).removeprefix("sha256:")
        if raw_digest in policies:
            raise ValidationFailure(f"duplicate execution policy digest: {raw_digest}")
        policies[raw_digest] = policy
    return policies


def _resolve_trial_declaration(
    path: Path,
    systems: Mapping[str, Mapping[str, Any]],
    policies: Mapping[str, Mapping[str, Any]],
) -> tuple[
    dict[str, Any],
    Mapping[str, Any],
    Mapping[str, Any],
    Mapping[str, Any],
    dict[str, Any],
]:
    trial = validate_manifest(path, "trial")
    if trial["schema_version"] != "0.3.0":
        raise ValidationFailure(f"{path}: reports require schema_version 0.3.0")
    if trial.get("apparatus_check") is True:
        raise ValidationFailure(f"{path}: apparatus-check trials cannot enter reports")
    system_digest = trial["system"]["manifest_sha256"]
    bindings = trial.get("bindings", {})
    if bindings.get("system_digest") != system_digest:
        raise ValidationFailure(f"{path}: system binding does not match reference")
    if bindings.get("execution_policy_digest") != trial["execution_policy"]["manifest_sha256"]:
        raise ValidationFailure(f"{path}: execution policy binding does not match reference")
    if bindings.get("dataset_freeze_digest") != trial["dataset"].get("freeze_digest"):
        raise ValidationFailure(f"{path}: dataset freeze binding does not match reference")
    system = systems.get(system_digest)
    if system is None:
        raise ValidationFailure(f"{path}: missing system manifest with digest {system_digest}")
    if str(system.get("id", "")).startswith("apparatus."):
        raise ValidationFailure(f"{path}: apparatus-check systems cannot enter benchmark reports")
    if system["id"] != trial["system"]["id"] or system["version"] != trial["system"]["version"]:
        raise ValidationFailure(f"{path}: system reference identity does not match manifest")
    policy_digest = trial["execution_policy"]["manifest_sha256"]
    policy = policies.get(policy_digest)
    if policy is None:
        raise ValidationFailure(
            f"{path}: missing execution policy manifest with digest {policy_digest}"
        )
    if (
        policy["id"] != trial["execution_policy"]["id"]
        or policy["version"] != trial["execution_policy"]["version"]
    ):
        raise ValidationFailure(
            f"{path}: execution policy reference identity does not match manifest"
        )

    driver = system["driver"]
    if trial["profile"] != driver["profile"]:
        raise ValidationFailure(f"{path}: trial profile does not match system driver profile")
    if trial.get("candidate") is not None and trial["candidate"] != driver["candidate"]:
        raise ValidationFailure(f"{path}: trial candidate does not match system driver candidate")
    if (
        trial.get("resolved_driver") is not None
        and trial["resolved_driver"]["tool_schemas_digest"] != driver["tool_contract_sha256"]
    ):
        raise ValidationFailure(f"{path}: resolved tool contract does not match system")

    policy_price_table = policy["accounting"]["model_price_table"]["sha256"]
    row = {
        "trial_id": trial["id"],
        "task_id": trial["task"]["id"],
        "task_digest": trial["task"]["manifest_sha256"],
        "dataset_freeze_digest": bindings.get(
            "dataset_freeze_digest", trial["dataset"].get("freeze_digest")
        ),
        "variant": trial["variant"],
        "pairing_key": trial["pairing_key"],
        "attempt_index": trial["attempt_index"],
        "system_id": trial["system"]["id"],
        "system_digest": system_digest,
        "execution_policy_digest": bindings.get("execution_policy_digest", policy_digest),
        "resolved_seed_provenance_sha256": trial["environment"]["resolved_seed_provenance_sha256"],
        "apparatus_digest": bindings.get("apparatus_digest"),
        "harness_id": system["harness"]["id"],
        "harness_digest": _bare_digest(system["harness"]),
        "model_routing_digest": _bare_digest(system["model_routing"]),
        "driver_candidate_digest": driver["candidate"]["manifest_sha256"],
        "driver_profile": (f"{driver['profile']['id']}@{driver['profile']['version']}"),
        "driver_tool_contract_digest": driver["tool_contract_sha256"],
        "driver_skill_mode": driver["skill_mode"],
        "price_table_digest": policy_price_table,
        "required_attempts_per_task": policy["limits"]["attempts_per_task"],
        "infrastructure_retries": policy["limits"]["infrastructure_retries"],
    }
    return trial, system, policy, driver, row


def normalize_trial(
    path: Path,
    systems: Mapping[str, Mapping[str, Any]],
    policies: Mapping[str, Mapping[str, Any]],
    *,
    certification_verifier_key: Path | None = None,
) -> dict[str, Any]:
    trial, system, policy, driver, declaration = _resolve_trial_declaration(path, systems, policies)
    system_digest = declaration["system_digest"]
    policy_digest = declaration["execution_policy_digest"]
    bindings = trial.get("bindings", {})

    observed = trial.get("execution", {}).get("observed", {})
    declared_routes = {
        route["id"]: {
            key: route[key] for key in ("role", "provider", "model", "snapshot", "service_tier")
        }
        for route in system["model_routing"]["routes"]
    }
    for call in observed.get("model_calls", []):
        actual = {
            key: call[key] for key in ("role", "provider", "model", "snapshot", "service_tier")
        }
        if declared_routes.get(call["route_id"]) != actual:
            raise ValidationFailure(
                f"{path}: observed model route {call['route_id']} does not match system"
            )
    policy_price_table = declaration["price_table_digest"]
    if (
        observed.get("model_price_table_sha256") is not None
        and observed["model_price_table_sha256"] != policy_price_table
    ):
        raise ValidationFailure(f"{path}: observed model price table does not match policy")
    observables = trial.get("observables", {})
    participation = trial.get("execution", {}).get("participation")
    certification = trial.get("execution", {}).get("certification")
    comparison_eligibility = trial.get("execution", {}).get("comparison_eligibility")
    termination = trial.get("termination_status", "not_run")
    outcome_available = isinstance(trial.get("outcomes"), dict)
    comparison_status = (
        comparison_eligibility.get("status")
        if isinstance(comparison_eligibility, Mapping)
        else "ineligible"
    )
    comparison_eligible = (
        trial["eligibility_status"] == "eligible"
        and comparison_status == "eligible"
        and outcome_available
    )
    outcomes = trial.get("outcomes", {})
    passed = (
        bool(outcomes.get("completion"))
        and not outcomes.get("forbidden_side_effects")
        and not outcomes.get("wrong_target_mutations")
        and not outcomes.get("leaked_input")
        if outcome_available
        else None
    )
    expected_certification_bindings = {
        "trial_id": trial["id"],
        "task_digest": trial["task"]["manifest_sha256"],
        "system_digest": system_digest,
        "execution_policy_digest": policy_digest,
        "resolved_seed_provenance_sha256": trial["environment"]["resolved_seed_provenance_sha256"],
        "apparatus_digest": bindings.get("apparatus_digest"),
    }
    if certification is None:
        certified = False
    elif not isinstance(certification, Mapping):
        raise ValidationFailure(f"{path}: apparatus certification is invalid")
    else:
        if certification.get("bindings", {}) != expected_certification_bindings:
            raise ValidationFailure(f"{path}: apparatus certification bindings mismatch")
        receipt_descriptor = certification.get("receipt")
        receipt = None
        if receipt_descriptor is not None:
            receipt = _load_attached_json(
                path,
                receipt_descriptor,
                "apparatus certification receipt",
            )
            if (
                receipt.get("bindings", {}).get("trial_id") != trial["id"]
                or (certification.get("certifying") is True and receipt.get("eligible") is not True)
                or receipt.get("apparatus_decision", {}).get("status")
                != certification.get("apparatus_status")
            ):
                raise ValidationFailure(
                    f"{path}: apparatus certification contradicts attached receipt"
                )
        declared_certifying = bool(
            certification.get("apparatus_status") == "passed"
            and certification.get("certifying") is True
        )
        if declared_certifying and termination != "completed":
            raise ValidationFailure(f"{path}: certifying apparatus requires completed termination")
        certified = False
        if declared_certifying:
            signature_descriptor = certification.get("signature")
            if (
                certification_verifier_key is not None
                and receipt is not None
                and isinstance(signature_descriptor, Mapping)
            ):
                artifact = _load_attached_json(
                    path,
                    signature_descriptor,
                    "apparatus certification signature",
                )
                expected_bindings = receipt.get("bindings")
                if not isinstance(expected_bindings, Mapping):
                    raise ValidationFailure(
                        f"{path}: apparatus certification receipt bindings are invalid"
                    )
                expected_bindings = {
                    **expected_bindings,
                    "trial_id": trial["id"],
                    "task_digest": "sha256:" + trial["task"]["manifest_sha256"],
                    "system_digest": "sha256:" + system_digest,
                    "execution_policy_digest": "sha256:" + policy_digest,
                    "seed_provenance_digest": (
                        "sha256:" + trial["environment"]["resolved_seed_provenance_sha256"]
                    ),
                }
                verified_receipt = verify_certification_signature(
                    artifact,
                    trusted_public_key=certification_verifier_key,
                    expected_bindings=expected_bindings,
                )
                if verified_receipt != receipt:
                    raise ValidationFailure(
                        f"{path}: signed apparatus certification receipt mismatch"
                    )
                signed_outcome = verified_receipt.get("outcome")
                if not isinstance(signed_outcome, Mapping):
                    raise ValidationFailure(
                        f"{path}: signed apparatus certification outcome is invalid"
                    )
                if signed_outcome.get("available") is not outcome_available:
                    raise ValidationFailure(
                        f"{path}: signed apparatus certification outcome availability mismatch"
                    )
                if signed_outcome.get("passed") is not passed:
                    raise ValidationFailure(
                        f"{path}: signed apparatus certification outcome mismatch"
                    )
                if certification.get("verifier_key_id") != key_id(certification_verifier_key):
                    raise ValidationFailure(
                        f"{path}: apparatus certification verifier key mismatch"
                    )
                certified = True
    if (
        certified
        and participation is not None
        and not (
            participation.get("status") == "passed"
            and participation.get("observer", {}).get("trust") == "certifying"
        )
    ):
        raise ValidationFailure(f"{path}: certifying apparatus contradicts driver participation")
    tokens = observables.get("tokens") if comparison_eligible else None
    token_total = None
    if isinstance(tokens, Mapping):
        if tokens.get("includes_subagents") is not True:
            raise ValidationFailure(f"{path}: comparison-eligible token totals omit subagents")
        token_values = [tokens.get(key) for key in ("input", "output", "cache_read", "cache_write")]
        if all(
            isinstance(value, int) and not isinstance(value, bool) and value >= 0
            for value in token_values
        ):
            token_total = sum(token_values)
    cost_usd = observables.get("cost_usd") if comparison_eligible else None
    if comparison_eligible and (token_total is None or cost_usd is None):
        raise ValidationFailure(f"{path}: comparison-eligible trial lacks complete accounting")
    return {
        **declaration,
        "eligible": comparison_eligible,
        "comparison_eligible": comparison_eligible,
        "eligibility_status": trial["eligibility_status"],
        "eligibility_evidence": list(trial["eligibility_evidence"]),
        "comparison_eligibility_status": comparison_status,
        "comparison_ineligibility_reasons": (
            list(comparison_eligibility.get("reasons", []))
            if isinstance(comparison_eligibility, Mapping)
            else ["comparison_eligibility_unavailable"]
        ),
        "execution_status": "executed" if outcome_available else "not_run",
        "participation_status": (
            participation.get("status") if isinstance(participation, Mapping) else None
        ),
        "participation_passed": (
            participation.get("passed") if isinstance(participation, Mapping) else None
        ),
        "participation_trust": (
            participation.get("observer", {}).get("trust")
            if isinstance(participation, Mapping)
            else None
        ),
        "participation_receipt_digest": (
            participation.get("receipt_digest") if isinstance(participation, Mapping) else None
        ),
        "apparatus_status": (
            certification.get("apparatus_status") if isinstance(certification, Mapping) else None
        ),
        "apparatus_certifying": certified,
        "apparatus_reasons": (
            list(certification.get("reasons", [])) if isinstance(certification, Mapping) else []
        ),
        "apparatus_receipt_sha256": (
            certification.get("receipt", {}).get("sha256")
            if isinstance(certification, Mapping)
            else None
        ),
        "apparatus_signature_sha256": (
            certification.get("signature", {}).get("sha256")
            if isinstance(certification, Mapping)
            else None
        ),
        "certified": certified,
        "passed": passed,
        "termination_status": termination,
        "infrastructure_failure": termination in INFRASTRUCTURE_TERMINATIONS,
        "wall_time_ms": observables.get("wall_time_ms"),
        "tokens": token_total,
        "cost_usd": cost_usd,
    }


def normalize_trials(
    trial_paths: Sequence[Path],
    system_paths: Sequence[Path],
    policy_paths: Sequence[Path],
    *,
    certification_verifier_key: Path | None = None,
) -> list[dict[str, Any]]:
    systems = load_systems(system_paths)
    policies = load_policies(policy_paths)
    rows = [
        normalize_trial(
            path,
            systems,
            policies,
            certification_verifier_key=certification_verifier_key,
        )
        for path in trial_paths
    ]
    rows.sort(key=lambda row: (row["task_id"], row["pairing_key"], row["trial_id"]))
    ids = [row["trial_id"] for row in rows]
    duplicate_ids = sorted(key for key, count in Counter(ids).items() if count > 1)
    if duplicate_ids:
        raise ValidationFailure(f"duplicate trial id: {duplicate_ids[0]}")
    return rows


def normalize_trial_templates(
    trial_paths: Sequence[Path],
    system_paths: Sequence[Path],
    policy_paths: Sequence[Path],
) -> list[dict[str, Any]]:
    systems = load_systems(system_paths)
    policies = load_policies(policy_paths)
    rows: list[dict[str, Any]] = []
    for path in trial_paths:
        trial, _system, _policy, _driver, declaration = _resolve_trial_declaration(
            path, systems, policies
        )
        if trial.get("pre_freeze") is not False:
            raise ValidationFailure(
                f"{path}: report preregistration requires a frozen trial template"
            )
        if trial.get("eligibility_status") != "eligible":
            raise ValidationFailure(
                f"{path}: report preregistration requires an eligible trial template"
            )
        certification = trial.get("execution", {}).get("certification")
        if certification is not None:
            if not isinstance(certification, Mapping):
                raise ValidationFailure(f"{path}: apparatus certification is invalid")
            expected_bindings = {
                "trial_id": trial["id"],
                "task_digest": trial["task"]["manifest_sha256"],
                "system_digest": declaration["system_digest"],
                "execution_policy_digest": declaration["execution_policy_digest"],
                "resolved_seed_provenance_sha256": declaration["resolved_seed_provenance_sha256"],
                "apparatus_digest": declaration["apparatus_digest"],
            }
            if certification.get("bindings", {}) != expected_bindings:
                raise ValidationFailure(f"{path}: apparatus certification bindings mismatch")
        rows.append(
            {
                **declaration,
                "eligible": False,
                "infrastructure_failure": False,
            }
        )
    rows.sort(key=lambda row: (row["task_id"], row["pairing_key"], row["trial_id"]))
    ids = [row["trial_id"] for row in rows]
    duplicate_ids = sorted(key for key, count in Counter(ids).items() if count > 1)
    if duplicate_ids:
        raise ValidationFailure(f"duplicate trial id: {duplicate_ids[0]}")
    return rows


def _factor_values(rows: Sequence[Mapping[str, Any]], factor: str) -> list[Any]:
    return sorted(
        {row.get(factor) for row in rows if row.get(factor) is not None},
        key=lambda value: str(value),
    )


def validate_comparison(
    rows: Sequence[Mapping[str, Any]],
    view_name: str,
    *,
    minimum_arms: int = 2,
) -> View:
    if view_name not in VIEWS:
        raise ValidationFailure(f"unknown report view: {view_name}")
    if not rows:
        raise ValidationFailure("report requires at least one trial")
    view = VIEWS[view_name]
    for factor in view.fixed:
        if factor in {"task_digest", "variant"}:
            continue
        values = _factor_values(rows, factor)
        scored_missing = any(
            row.get(factor) is None
            for row in rows
            if row["eligible"] and not row["infrastructure_failure"]
        )
        if scored_missing or not values:
            raise ValidationFailure(f"confounded {view_name}: fixed factor {factor} is missing")
        if len(values) != 1:
            raise ValidationFailure(
                f"confounded {view_name}: fixed factor {factor} has {len(values)} values"
            )
    arms = _factor_values(rows, view.arm)
    if len(arms) < minimum_arms:
        raise ValidationFailure(
            f"{view_name} requires at least {minimum_arms} values for varying factor {view.arm}"
        )
    expected = set(arms)
    task_freezes: dict[str, set[Any]] = defaultdict(set)
    for row in rows:
        task_freezes[str(row["task_id"])].add(row["task_digest"])
    mixed_task = next(
        (task_id for task_id, digests in sorted(task_freezes.items()) if len(digests) != 1),
        None,
    )
    if mixed_task is not None:
        raise ValidationFailure(
            f"confounded {view_name}: fixed factor task_digest has multiple values for task {mixed_task}"
        )
    by_pair: dict[str, list[Mapping[str, Any]]] = defaultdict(list)
    for row in rows:
        by_pair[str(row["pairing_key"])].append(row)
    for key, pair_rows in sorted(by_pair.items()):
        task_digests = {row["task_digest"] for row in pair_rows}
        if len(task_digests) != 1:
            raise ValidationFailure(
                f"confounded {view_name}: fixed factor task_digest differs within pairing_key {key}"
            )
        variants = {row["variant"] for row in pair_rows}
        if len(variants) != 1:
            raise ValidationFailure(
                f"confounded {view_name}: fixed factor variant differs within pairing_key {key}"
            )
        present = {row[view.arm] for row in pair_rows}
        if len(pair_rows) != len(present):
            raise ValidationFailure(
                f"duplicate {view_name}: pairing_key {key} contains an arm more than once"
            )
        if present != expected:
            raise ValidationFailure(
                f"unpaired {view_name}: pairing_key {key} does not contain every arm"
            )
    return view


def build_report_preregistration(
    trial_paths: Sequence[Path],
    system_paths: Sequence[Path],
    policy_paths: Sequence[Path],
    view_name: str,
    *,
    bootstrap_samples: int = 2000,
    seed: int = 0,
    pass_k: int = 5,
    max_infrastructure_failure_rate: float = 0.05,
) -> dict[str, Any]:
    rows = normalize_trial_templates(trial_paths, system_paths, policy_paths)
    view = validate_comparison(
        rows,
        view_name,
        minimum_arms=1 if view_name == "system-track" else 2,
    )
    if bootstrap_samples < 1 or pass_k < 1 or not 0 <= max_infrastructure_failure_rate <= 1:
        raise ValidationFailure(
            "bootstrap_samples and pass_k must be positive and infrastructure threshold must be in [0, 1]"
        )
    required_values = _factor_values(rows, "required_attempts_per_task")
    if len(required_values) != 1:
        raise ValidationFailure(
            f"confounded {view_name}: required attempts per task has {len(required_values)} values"
        )
    required_attempts = required_values[0]
    expected_attempts = list(range(required_attempts))
    arms = _factor_values(rows, view.arm)
    task_ids = sorted({str(row["task_id"]) for row in rows})
    for task_id in task_ids:
        for arm in arms:
            actual_attempts = sorted(
                row["attempt_index"]
                for row in rows
                if str(row["task_id"]) == task_id and row[view.arm] == arm
            )
            if actual_attempts != expected_attempts:
                raise ValidationFailure(
                    f"incomplete {view_name}: task {task_id} arm {arm} "
                    f"attempt_index values do not match 0..{required_attempts - 1}"
                )

    def descriptors(paths: Sequence[Path], kind: str) -> list[dict[str, str]]:
        output = []
        for path in paths:
            document = validate_manifest(path, kind)
            output.append(
                {
                    "id": str(document["id"]),
                    "sha256": digest_file(path.resolve()).removeprefix("sha256:"),
                }
            )
        return sorted(output, key=lambda item: (item["id"], item["sha256"]))

    body: dict[str, Any] = {
        "schema_version": 1,
        "view": view_name,
        "varying_factor": view.arm,
        "arms": arms,
        "fixed_factors": {factor: _factor_values(rows, factor) for factor in view.fixed},
        "required_attempts_per_task": required_attempts,
        "pairing_keys": sorted({str(row["pairing_key"]) for row in rows}),
        "trial_plan": sorted(
            (
                {
                    "trial_id": str(row["trial_id"]),
                    "task_id": str(row["task_id"]),
                    "task_digest": str(row["task_digest"]),
                    "pairing_key": str(row["pairing_key"]),
                    "attempt_index": int(row["attempt_index"]),
                    "system_digest": str(row["system_digest"]),
                    "execution_policy_digest": str(row["execution_policy_digest"]),
                }
                for row in rows
            ),
            key=lambda item: (
                item["task_id"],
                item["pairing_key"],
                item["trial_id"],
            ),
        ),
        "trial_templates": descriptors(trial_paths, "trial"),
        "systems": descriptors(system_paths, "system"),
        "execution_policies": descriptors(policy_paths, "execution-policy"),
        "report_parameters": {
            "bootstrap_samples": bootstrap_samples,
            "seed": seed,
            "pass_k": pass_k,
            "max_infrastructure_failure_rate": max_infrastructure_failure_rate,
        },
    }
    body["digest"] = digest_json(body)
    return body


def validate_report_preregistration(
    document: Mapping[str, Any],
    rows: Sequence[Mapping[str, Any]],
    system_paths: Sequence[Path],
    policy_paths: Sequence[Path],
    view_name: str,
    *,
    bootstrap_samples: int,
    seed: int,
    pass_k: int,
    max_infrastructure_failure_rate: float,
) -> None:
    """Bind report inputs and parameters to one signed frozen plan."""

    if not isinstance(document, Mapping):
        raise ValidationFailure("report preregistration is invalid")
    unsigned = dict(document)
    claimed_digest = unsigned.pop("digest", None)
    if claimed_digest != digest_json(unsigned):
        raise ValidationFailure("report preregistration digest mismatch")
    if document.get("view") != view_name:
        raise ValidationFailure("report view does not match preregistration")
    view = VIEWS.get(view_name)
    if view is None:
        raise ValidationFailure(f"unknown report view: {view_name}")
    if document.get("arms") != _factor_values(rows, view.arm):
        raise ValidationFailure("report arms do not match preregistration")
    if document.get("pairing_keys") != sorted({str(row["pairing_key"]) for row in rows}):
        raise ValidationFailure("report pairing keys do not match preregistration")
    expected_fixed = {factor: _factor_values(rows, factor) for factor in view.fixed}
    if document.get("fixed_factors") != expected_fixed:
        raise ValidationFailure("report fixed factors do not match preregistration")
    required_attempts = _factor_values(rows, "required_attempts_per_task")
    if (
        len(required_attempts) != 1
        or document.get("required_attempts_per_task") != required_attempts[0]
    ):
        raise ValidationFailure("report attempt policy does not match preregistration")
    expected_parameters = {
        "bootstrap_samples": bootstrap_samples,
        "seed": seed,
        "pass_k": pass_k,
        "max_infrastructure_failure_rate": max_infrastructure_failure_rate,
    }
    if document.get("report_parameters") != expected_parameters:
        raise ValidationFailure("report parameters do not match preregistration")

    actual_plan = sorted(
        (
            {
                "trial_id": str(row["trial_id"]),
                "task_id": str(row["task_id"]),
                "task_digest": str(row["task_digest"]),
                "pairing_key": str(row["pairing_key"]),
                "attempt_index": int(row["attempt_index"]),
                "system_digest": str(row["system_digest"]),
                "execution_policy_digest": str(row["execution_policy_digest"]),
            }
            for row in rows
        ),
        key=lambda item: (
            item["task_id"],
            item["pairing_key"],
            item["trial_id"],
        ),
    )
    if document.get("trial_plan") != actual_plan:
        raise ValidationFailure("report trials do not match preregistration")

    def descriptors(paths: Sequence[Path], kind: str) -> list[dict[str, str]]:
        output = []
        for path in paths:
            manifest = validate_manifest(path, kind)
            output.append(
                {
                    "id": str(manifest["id"]),
                    "sha256": digest_file(path.resolve()).removeprefix("sha256:"),
                }
            )
        return sorted(output, key=lambda item: (item["id"], item["sha256"]))

    if document.get("systems") != descriptors(system_paths, "system"):
        raise ValidationFailure("report systems do not match preregistration")
    if document.get("execution_policies") != descriptors(policy_paths, "execution-policy"):
        raise ValidationFailure("report execution policies do not match preregistration")


def _task_macro(rows: Sequence[Mapping[str, Any]]) -> float:
    by_task: dict[str, list[float]] = defaultdict(list)
    for row in rows:
        by_task[str(row["task_id"])].append(1.0 if row["passed"] else 0.0)
    return sum(sum(values) / len(values) for values in by_task.values()) / len(by_task)


def _bootstrap_ci(rows: Sequence[Mapping[str, Any]], *, samples: int, seed: int) -> list[float]:
    by_task: dict[str, list[Mapping[str, Any]]] = defaultdict(list)
    for row in rows:
        by_task[str(row["task_id"])].append(row)
    task_ids = sorted(by_task)
    rng = random.Random(seed)
    values: list[float] = []
    for _ in range(samples):
        sampled_means = []
        for _task in task_ids:
            task_id = rng.choice(task_ids)
            attempts = by_task[task_id]
            draw = [rng.choice(attempts) for _ in attempts]
            sampled_means.append(sum(1.0 if row["passed"] else 0.0 for row in draw) / len(draw))
        values.append(sum(sampled_means) / len(sampled_means))
    values.sort()
    return [_quantile(values, 0.025), _quantile(values, 0.975)]


def _quantile(values: Sequence[float], probability: float) -> float:
    if not values:
        raise ValidationFailure("cannot calculate a quantile without values")
    index = (len(values) - 1) * probability
    lower = math.floor(index)
    upper = math.ceil(index)
    if lower == upper:
        return values[lower]
    return values[lower] * (upper - index) + values[upper] * (index - lower)


def _metric_summary(rows: Sequence[Mapping[str, Any]], name: str) -> dict[str, Any]:
    values = [float(row[name]) for row in rows if row.get(name) is not None]
    return {"median": median(values) if values else None, "sample_count": len(values)}


def _paired_deltas(
    rows: Sequence[Mapping[str, Any]], view: View, *, samples: int, seed: int
) -> list[dict[str, Any]]:
    arms = _factor_values(rows, view.arm)
    by_pair: dict[str, dict[Any, Mapping[str, Any]]] = defaultdict(dict)
    for row in rows:
        by_pair[str(row["pairing_key"])][row[view.arm]] = row
    output = []
    for left_index, left in enumerate(arms):
        for right in arms[left_index + 1 :]:
            pairs = [
                (items[left], items[right])
                for _, items in sorted(by_pair.items())
                if left in items and right in items
            ]
            if not pairs:
                raise ValidationFailure(
                    f"paired comparison has no scored pairs for {left} and {right}"
                )
            by_task: dict[str, list[float]] = defaultdict(list)
            for a, b in pairs:
                if a["task_id"] != b["task_id"]:
                    raise ValidationFailure(
                        f"paired rows disagree on task for pairing_key {a['pairing_key']}"
                    )
                by_task[str(a["task_id"])].append(
                    (1.0 if b["passed"] else 0.0) - (1.0 if a["passed"] else 0.0)
                )
            task_ids = sorted(by_task)
            task_means = [sum(by_task[task]) / len(by_task[task]) for task in task_ids]
            rng = random.Random(f"{seed}:{left}:{right}")
            boot = []
            for _ in range(samples):
                sampled_task_means = []
                for _task in task_ids:
                    task = rng.choice(task_ids)
                    attempts = by_task[task]
                    draw = [rng.choice(attempts) for _ in attempts]
                    sampled_task_means.append(sum(draw) / len(draw))
                boot.append(sum(sampled_task_means) / len(sampled_task_means))
            boot.sort()
            output.append(
                {
                    "baseline": left,
                    "comparison": right,
                    "delta": sum(task_means) / len(task_means),
                    "confidence_interval": [_quantile(boot, 0.025), _quantile(boot, 0.975)],
                    "pair_count": len(pairs),
                }
            )
    return output


def build_report(
    rows: Sequence[Mapping[str, Any]],
    view_name: str,
    *,
    bootstrap_samples: int = 2000,
    seed: int = 0,
    pass_k: int = 5,
    max_infrastructure_failure_rate: float = 0.05,
) -> dict[str, Any]:
    if bootstrap_samples < 1 or pass_k < 1 or not 0 <= max_infrastructure_failure_rate <= 1:
        raise ValidationFailure(
            "bootstrap_samples and pass_k must be positive and infrastructure threshold must be in [0, 1]"
        )
    rows = sorted(
        rows,
        key=lambda row: (str(row["task_id"]), str(row["pairing_key"]), str(row["trial_id"])),
    )
    infrastructure_pairs = {
        str(row["pairing_key"]) for row in rows if row["infrastructure_failure"]
    }
    scored = [
        row
        for row in rows
        if row["eligible"]
        and str(row["pairing_key"]) not in infrastructure_pairs
        and row["passed"] is not None
    ]
    descriptive = [
        row
        for row in rows
        if not row["eligible"]
        and str(row["pairing_key"]) not in infrastructure_pairs
        and row["passed"] is not None
    ]
    single_arm_descriptive = (
        view_name == "system-track" and len(_factor_values(rows, VIEWS[view_name].arm)) == 1
    )
    ineligible_descriptive = not scored and view_name == "system-track" and bool(descriptive)
    descriptive_only = single_arm_descriptive or ineligible_descriptive
    view = validate_comparison(
        rows,
        view_name,
        minimum_arms=1 if descriptive_only else 2,
    )
    scored_trial_ids = {row["trial_id"] for row in scored}
    if not scored and not descriptive:
        raise ValidationFailure("report has no completed trials")
    if not scored and not descriptive_only:
        raise ValidationFailure("report has no eligible scored trials")
    reported = scored if scored else descriptive
    arms = _factor_values(rows, view.arm)
    summaries = []
    for index, arm in enumerate(arms):
        arm_rows = [row for row in reported if row[view.arm] == arm]
        if not arm_rows:
            qualifier = "completed descriptive" if descriptive_only else "eligible scored"
            raise ValidationFailure(f"report arm {arm} has no {qualifier} trials")
        task_rates: dict[str, float] = {}
        for task_id in sorted({str(row["task_id"]) for row in arm_rows}):
            task_rows = [row for row in arm_rows if str(row["task_id"]) == task_id]
            task_rates[task_id] = sum(1.0 if row["passed"] else 0.0 for row in task_rows) / len(
                task_rows
            )
        interval = _bootstrap_ci(arm_rows, samples=bootstrap_samples, seed=seed + index)
        summaries.append(
            {
                "arm": arm,
                "task_macro_success": _task_macro(arm_rows),
                "confidence_interval": interval,
                "pass_k": {
                    "k": pass_k,
                    "task_macro": sum(rate**pass_k for rate in task_rates.values())
                    / len(task_rates),
                },
                "trial_count": len(arm_rows),
                "task_count": len(task_rates),
                "wall_time_ms": _metric_summary(arm_rows, "wall_time_ms"),
                "tokens": _metric_summary(arm_rows, "tokens"),
                "cost_usd": _metric_summary(arm_rows, "cost_usd"),
                "termination_mix": dict(
                    sorted(Counter(str(row["termination_status"]) for row in arm_rows).items())
                ),
            }
        )
    if not descriptive_only:
        for item in summaries:
            low, high = item["confidence_interval"]
            item["rank_range"] = {
                "best": 1
                + sum(
                    other["confidence_interval"][0] > high
                    for other in summaries
                    if other is not item
                ),
                "worst": 1
                + sum(
                    other["confidence_interval"][1] >= low
                    for other in summaries
                    if other is not item
                ),
            }

    missing = Counter()
    for row in rows:
        if not row["eligible"]:
            missing["ineligible"] += 1
            missing["comparison_ineligible"] += 1
        if row["infrastructure_failure"]:
            missing["infrastructure_failure"] += 1
        if (
            row["eligible"]
            and not row["infrastructure_failure"]
            and row["passed"] is not None
            and not row["certified"]
        ):
            missing["uncertified_outcome"] += 1
        if row["passed"] is None:
            missing["outcome_missing"] += 1
        if str(row["pairing_key"]) in infrastructure_pairs and not row["infrastructure_failure"]:
            missing["paired_infrastructure_exclusion"] += 1

    infrastructure_rate = missing["infrastructure_failure"] / len(rows)
    required_attempts_values = _factor_values(rows, "required_attempts_per_task")
    retry_values = _factor_values(rows, "infrastructure_retries")
    if len(required_attempts_values) != 1 or len(retry_values) != 1:
        raise ValidationFailure(
            "comparison requires one preregistered attempt and infrastructure-retry policy"
        )
    required_attempts = int(required_attempts_values[0])
    infrastructure_retries = int(retry_values[0])
    violations = []
    if ineligible_descriptive:
        violations.append("no_comparison_eligible_trials")
    if single_arm_descriptive:
        violations.append("single_arm_descriptive_only")
    if infrastructure_rate > max_infrastructure_failure_rate:
        violations.append("infrastructure_failure_rate_exceeded")
    if missing["uncertified_outcome"]:
        violations.append("uncertified_outcomes_present")
    for arm in arms:
        task_ids = sorted({str(row["task_id"]) for row in rows if row[view.arm] == arm})
        for task_id in task_ids:
            cell = [row for row in rows if row[view.arm] == arm and str(row["task_id"]) == task_id]
            completed = sum(row["trial_id"] in scored_trial_ids for row in cell)
            infrastructure_failures = sum(row["infrastructure_failure"] for row in cell)
            if completed < required_attempts:
                violations.append("insufficient_attempts")
            if infrastructure_failures > infrastructure_retries:
                violations.append("infrastructure_retry_limit_exceeded")
    violations = sorted(set(violations))
    decision_bearing = not violations
    report = {
        "schema_version": "0.3.0",
        "view": view_name,
        "decision_bearing": decision_bearing,
        "decision_bearing_violations": violations,
        "varying_factor": view.arm,
        "fixed_factors": {
            factor: (
                _factor_values(rows, factor)[0]
                if len(_factor_values(rows, factor)) == 1
                else _factor_values(rows, factor)
            )
            for factor in view.fixed
        },
        "method": {
            "aggregation": "task-macro",
            "confidence_interval": "hierarchical-bootstrap-tasks-and-attempts",
            "bootstrap_samples": bootstrap_samples,
            "seed": seed,
            "paired": view.paired,
            "max_infrastructure_failure_rate": max_infrastructure_failure_rate,
            "required_attempts_per_task": required_attempts,
            "infrastructure_retries": infrastructure_retries,
        },
        "arms": summaries,
        "paired_deltas": _paired_deltas(scored, view, samples=bootstrap_samples, seed=seed)
        if view.paired
        else [],
        "missingness": {
            **dict(sorted(missing.items())),
            "input_trials": len(rows),
            "scored_trials": len(scored),
            "infrastructure_failure_rate": infrastructure_rate,
        },
    }
    if descriptive_only:
        report["report_type"] = "descriptive"
        report["missingness"]["descriptive_trials"] = len(descriptive)
    report["digest"] = digest_json(report)
    return report


def write_normalized_jsonl(rows: Iterable[Mapping[str, Any]], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    content = "".join(
        json.dumps(dict(row), sort_keys=True, separators=(",", ":")) + "\n" for row in rows
    )
    path.write_text(content, encoding="utf-8")
