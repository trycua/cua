#!/usr/bin/env python3
# SPDX-License-Identifier: MIT

"""Validate Cua-Bench schemas, examples, negative cases, and freeze digests."""

from __future__ import annotations

import copy
import hashlib
import json
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker
from jsonschema.exceptions import SchemaError
from referencing import Registry, Resource

import rfc8785


ROOT = Path(__file__).resolve().parent
SCHEMA_ROOT = ROOT / "schemas_data"
SCHEMA_DIR = SCHEMA_ROOT / "v0.1.0"
SCHEMA_V02_DIR = SCHEMA_ROOT / "v0.2.0"
SCHEMA_V03_DIR = SCHEMA_ROOT / "v0.3.0"
EXAMPLE_DIR = SCHEMA_ROOT / "examples"
FIXTURE_DIR = SCHEMA_ROOT / "fixtures"


def load_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def pointer(parts: Any) -> str:
    escaped = [str(part).replace("~", "~0").replace("/", "~1") for part in parts]
    return "/" + "/".join(escaped) if escaped else "/"


def resolve_pointer(document: Any, value: str) -> tuple[Any, str]:
    parts = value.removeprefix("/").split("/") if value != "/" else []
    parts = [part.replace("~1", "/").replace("~0", "~") for part in parts]
    current = document
    for part in parts[:-1]:
        current = current[int(part)] if isinstance(current, list) else current[part]
    return current, parts[-1]


def read_pointer(document: Any, value: str) -> Any:
    parts = value.removeprefix("/").split("/") if value != "/" else []
    parts = [part.replace("~1", "/").replace("~0", "~") for part in parts]
    current = document
    for part in parts:
        current = current[int(part)] if isinstance(current, list) else current[part]
    return current


def schema_required_groups(value: Any, location: str = "#", inside_not: bool = False) -> set[str]:
    if isinstance(value, list):
        groups: set[str] = set()
        for index, item in enumerate(value):
            groups |= schema_required_groups(item, f"{location}/{index}", inside_not)
        return groups
    if not isinstance(value, dict):
        return set()

    groups = set()
    if isinstance(value.get("required"), list) and not inside_not:
        groups.add(f"{location}/required")
    for key, item in value.items():
        groups |= schema_required_groups(item, f"{location}/{key}", inside_not or key == "not")
    return groups


def build_registry(schemas: list[dict[str, Any]]) -> Registry:
    registry = Registry()
    for schema in schemas:
        Draft202012Validator.check_schema(schema)
        registry = registry.with_resource(schema["$id"], Resource.from_contents(schema))
    return registry


def flatten_errors(errors: list[Any]) -> list[Any]:
    flattened = []
    for error in errors:
        flattened.append(error)
        flattened.extend(flatten_errors(list(error.context)))
    return flattened


def validate_negative_cases(
    validators: dict[str, Draft202012Validator],
    schemas: dict[str, dict[str, Any]],
) -> int:
    total = 0
    for declaration_path in sorted(FIXTURE_DIR.glob("*-negative-cases.json")):
        declaration = load_json(declaration_path)
        if "schema" not in declaration:
            continue
        schema_name = declaration["schema"]
        validator = validators[schema_name]
        covered: set[str] = set()

        for case in declaration["cases"]:
            base_path = FIXTURE_DIR / case.get("base", declaration["base"])
            instance = copy.deepcopy(load_json(base_path.resolve()))
            for location, value in case.get("replace", {}).items():
                parent, key = resolve_pointer(instance, location)
                if isinstance(parent, list):
                    parent[int(key)] = value
                else:
                    parent[key] = value
            for location in case.get("remove", []):
                parent, key = resolve_pointer(instance, location)
                if isinstance(parent, list):
                    index = int(key)
                    if index >= len(parent) and case.get("allow_missing_remove"):
                        continue
                    del parent[index]
                else:
                    if key not in parent and case.get("allow_missing_remove"):
                        continue
                    del parent[key]

            errors = sorted(validator.iter_errors(instance), key=lambda error: list(error.path))
            if not errors:
                raise AssertionError(f"negative case {case['name']!r} unexpectedly passed")
            all_errors = flatten_errors(errors)

            expected = case["expect"]
            if not any(
                pointer(error.absolute_path) == expected["instance"]
                and error.validator == expected["validator"]
                for error in all_errors
            ):
                actual = ", ".join(
                    f"{pointer(error.absolute_path)}:{error.validator}" for error in all_errors
                )
                raise AssertionError(
                    f"negative case {case['name']!r} did not fail as declared; got {actual}"
                )
            if "covers" in case:
                covered.add(case["covers"])
            for location in case.get("remove", []):
                parent_pointer, missing_name = location.rsplit("/", 1)
                parent_pointer = parent_pointer or "/"
                if not any(
                    pointer(error.absolute_path) == parent_pointer
                    and error.validator == "required"
                    and missing_name in error.validator_value
                    and error.message == f"'{missing_name}' is a required property"
                    for error in all_errors
                ):
                    raise AssertionError(
                        f"negative case {case['name']!r} does not prove removed "
                        f"field {location!r} is required"
                    )
            total += 1

        required_groups = schema_required_groups(schemas[schema_name])
        unknown = covered - required_groups
        if unknown:
            raise AssertionError(
                f"{schema_name} negative cases name unknown required groups: "
                + ", ".join(sorted(unknown))
            )
        missing = required_groups - covered
        if missing:
            raise AssertionError(
                f"{schema_name} negative cases do not cover required groups: "
                + ", ".join(sorted(missing))
            )
    return total


def artifact_entries(inputs: dict[str, Any]) -> list[dict[str, str]]:
    return [
        *inputs["task_manifests"],
        *inputs["fixture_artifacts"],
        *inputs["evaluator_artifacts"],
        inputs["normalized_facade_contract"],
        inputs["neutral_instructions"],
    ]


def canonical_digest(value: Any) -> str:
    encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True).encode(
        "utf-8"
    )
    return hashlib.sha256(encoded).hexdigest()


def json_schema_digest(value: Any) -> str:
    return hashlib.sha256(rfc8785.dumps(value)).hexdigest()


def resolve_relative(root: Path, value: str) -> Path:
    resolved_root = root.resolve()
    resolved = (resolved_root / value).resolve()
    if not resolved.is_relative_to(resolved_root):
        raise AssertionError(f"artifact path escapes its manifest directory: {value}")
    return resolved


def validate_unique_task_ids(task: dict[str, Any], source: str) -> None:
    fields = [
        ("variants", task["variants"]),
        ("reset.failure_fixtures", task["reset"]["failure_fixtures"]),
    ]
    if "participation_requirements" in task:
        fields.append(("participation_requirements", task["participation_requirements"]))
    for field, values in fields:
        ids = [value["id"] for value in values]
        duplicates = sorted({value for value in ids if ids.count(value) > 1})
        if duplicates:
            raise AssertionError(f"{source} has duplicate {field} ids: " + ", ".join(duplicates))


def validate_declared_artifact(
    manifest_dir: Path,
    artifact: dict[str, str],
    frozen: set[tuple[Path, str]],
    category: str,
) -> None:
    path = resolve_relative(manifest_dir, artifact["path"])
    key = (path, artifact["sha256"])
    if key not in frozen:
        raise AssertionError(f"task artifact {artifact['path']} is absent from freeze {category}")
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != artifact["sha256"]:
        raise AssertionError(f"task artifact digest mismatch for {artifact['path']}: {actual}")


def validate_freeze(
    dataset_path: Path,
    dataset: dict[str, Any],
    task_validator: Draft202012Validator,
    *,
    check_mutation: bool = True,
) -> None:
    inputs = dataset["freeze"]["inputs"]
    for artifact in artifact_entries(inputs):
        content = resolve_relative(dataset_path.parent, artifact["path"]).read_bytes()
        actual = hashlib.sha256(content).hexdigest()
        if actual != artifact["sha256"]:
            raise AssertionError(f"freeze input digest mismatch for {artifact['path']}: {actual}")

    selected = [task["manifest"] for task in dataset["tasks"]]
    if selected != inputs["task_manifests"]:
        raise AssertionError("freeze task manifests do not match ordered dataset tasks")
    for selection in dataset["tasks"]:
        manifest_path = resolve_relative(dataset_path.parent, selection["manifest"]["path"])
        manifest = load_json(manifest_path)
        task_validator.validate(manifest)
        validate_unique_task_ids(manifest, selection["manifest"]["path"])
        if (selection["id"], selection["version"]) != (
            manifest["id"],
            manifest["version"],
        ):
            raise AssertionError(
                f"dataset selection does not match {selection['manifest']['path']} identity"
            )
        manifest_variants = {variant["id"] for variant in manifest["variants"]}
        unknown_variants = set(selection["variants"]) - manifest_variants
        if unknown_variants:
            raise AssertionError(
                "dataset selects unknown variants: " + ", ".join(sorted(unknown_variants))
            )
        frozen_fixtures = {
            (
                resolve_relative(dataset_path.parent, artifact["path"]),
                artifact["sha256"],
            )
            for artifact in inputs["fixture_artifacts"]
        }
        frozen_evaluators = {
            (
                resolve_relative(dataset_path.parent, artifact["path"]),
                artifact["sha256"],
            )
            for artifact in inputs["evaluator_artifacts"]
        }
        selected_variants = {variant["id"]: variant for variant in manifest["variants"]}
        for variant_id in selection["variants"]:
            for artifact in selected_variants[variant_id]["fixture_artifacts"]:
                validate_declared_artifact(
                    manifest_path.parent,
                    artifact,
                    frozen_fixtures,
                    "fixture_artifacts",
                )
        for failure in manifest["reset"]["failure_fixtures"]:
            validate_declared_artifact(
                manifest_path.parent,
                failure["artifact"],
                frozen_fixtures,
                "fixture_artifacts",
            )
        evaluator_paths = {artifact["path"] for artifact in manifest["evaluator"]["inputs"]}
        if manifest["evaluator"]["entrypoint"] not in evaluator_paths:
            raise AssertionError("evaluator entrypoint is absent from evaluator inputs")
        for artifact in manifest["evaluator"]["inputs"]:
            validate_declared_artifact(
                manifest_path.parent,
                artifact,
                frozen_evaluators,
                "evaluator_artifacts",
            )

    actual = canonical_digest(inputs)
    if actual != dataset["freeze"]["digest"]:
        raise AssertionError(f"dataset freeze digest mismatch: {actual}")

    if not check_mutation:
        return

    fixture = load_json(FIXTURE_DIR / "dataset-freeze-digest.json")
    if (FIXTURE_DIR / fixture["dataset"]).resolve() != dataset_path.resolve():
        raise AssertionError("freeze mutation fixture names a different dataset")
    changed_input_digest = hashlib.sha256(fixture["changed_bytes_utf8"].encode("utf-8")).hexdigest()
    if changed_input_digest != fixture["expected_input_sha256"]:
        raise AssertionError("changed freeze input digest does not match its fixture")
    changed = copy.deepcopy(inputs)
    changed[fixture["changed_input"]]["sha256"] = changed_input_digest
    changed_digest = canonical_digest(changed)
    if changed_digest != fixture["expected_freeze_digest"]:
        raise AssertionError("changed aggregate freeze digest does not match its fixture")
    if changed_digest == actual:
        raise AssertionError("changing a freeze input did not change the freeze digest")


def validate_artifact_bytes(manifest_path: Path, artifact: dict[str, str], label: str) -> Path:
    path = resolve_relative(manifest_path.parent, artifact["path"])
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != artifact["sha256"]:
        raise AssertionError(f"{label} digest mismatch for {artifact['path']}: {actual}")
    return path


def artifact_identity(manifest_path: Path, artifact: dict[str, str]) -> tuple[Path, str]:
    return (
        resolve_relative(manifest_path.parent, artifact["path"]),
        artifact["sha256"],
    )


def validate_driver(driver_path: Path, driver: dict[str, Any]) -> None:
    tools = driver["interface"]["tools"]
    names = [tool["name"] for tool in tools]
    if len(names) != len(set(names)):
        raise AssertionError("driver tool names must be unique")
    actual = json_schema_digest(tools)
    if actual != driver["interface"]["tool_schemas_digest"]:
        raise AssertionError(f"driver tool schema digest mismatch: {actual}")
    for tool in tools:
        try:
            Draft202012Validator.check_schema(tool["input_schema"])
            if "output_schema" in tool:
                Draft202012Validator.check_schema(tool["output_schema"])
        except SchemaError as error:
            raise AssertionError("driver embeds an invalid JSON Schema") from error
    if "skill" in driver:
        validate_artifact_bytes(driver_path, driver["skill"]["artifact"], "skill")


def validate_profile(profile_path: Path, profile: dict[str, Any]) -> None:
    validate_artifact_bytes(
        profile_path, profile["harness"]["inventory"], "profile harness inventory"
    )
    if "artifact" in profile["guidance"]:
        validate_artifact_bytes(profile_path, profile["guidance"]["artifact"], "profile guidance")


def validate_delivery(trial_path: Path, delivery: dict[str, Any], label: str) -> None:
    if delivery["status"] != "delivered":
        return
    validate_artifact_bytes(trial_path, delivery["source_artifact"], f"{label} source")
    content_path = validate_artifact_bytes(
        trial_path, delivery["content_artifact"], f"{label} content"
    )
    content = content_path.read_bytes()
    if len(content) != delivery["bytes"]:
        raise AssertionError(f"{label} byte count mismatch")
    actual = hashlib.sha256(content).hexdigest()
    if actual != delivery["content_sha256"]:
        raise AssertionError(f"{label} content digest mismatch: {actual}")


def validate_trial(
    trial_path: Path,
    trial: dict[str, Any],
    task_path: Path,
    task: dict[str, Any],
    dataset_path: Path,
    dataset: dict[str, Any],
    profiles: dict[str, tuple[Path, dict[str, Any]]],
    driver_path: Path,
    driver: dict[str, Any],
) -> None:
    if trial["task"] != {
        "id": task["id"],
        "version": task["version"],
        "manifest_sha256": hashlib.sha256(task_path.read_bytes()).hexdigest(),
    }:
        raise AssertionError(f"{trial_path.name} task reference mismatch")
    expected_dataset = {
        "id": dataset["id"],
        "version": dataset["version"],
        "manifest_sha256": hashlib.sha256(dataset_path.read_bytes()).hexdigest(),
    }
    if not trial["pre_freeze"]:
        expected_dataset["freeze_digest"] = dataset["freeze"]["digest"]
    if trial["dataset"] != expected_dataset:
        raise AssertionError(f"{trial_path.name} dataset reference mismatch")
    selections = [
        selection
        for selection in dataset["tasks"]
        if (selection["id"], selection["version"]) == (task["id"], task["version"])
    ]
    if len(selections) != 1:
        raise AssertionError(f"{trial_path.name} task selection is not unique")
    if trial["variant"] not in selections[0]["variants"]:
        raise AssertionError(f"{trial_path.name} selects a variant absent from dataset")

    profile_id = trial["profile"]["id"]
    if profile_id not in profiles:
        raise AssertionError(f"{trial_path.name} references an unknown profile")
    profile_path, profile = profiles[profile_id]
    if trial["profile"]["version"] != profile["version"]:
        raise AssertionError(f"{trial_path.name} profile version mismatch")
    if artifact_identity(trial_path, trial["harness"]["inventory"]) != (
        artifact_identity(profile_path, profile["harness"]["inventory"])
    ):
        raise AssertionError(f"{trial_path.name} harness inventory differs from profile")
    validate_artifact_bytes(trial_path, trial["harness"]["build"], "harness build")
    validate_artifact_bytes(trial_path, trial["harness"]["inventory"], "resolved harness inventory")
    validate_artifact_bytes(trial_path, trial["environment"]["image"], "environment image")
    validate_artifact_bytes(trial_path, trial["environment"]["observer"], "environment observer")

    if not trial["pre_freeze"]:
        if trial["bindings"]["dataset_freeze_digest"] != dataset["freeze"]["digest"]:
            raise AssertionError(f"{trial_path.name} dataset binding mismatch")

    if profile_id != "harness-native-reference":
        expected_candidate = {
            "id": driver["id"],
            "version": driver["version"],
            "manifest_sha256": hashlib.sha256(driver_path.read_bytes()).hexdigest(),
        }
        if trial["candidate"] != expected_candidate:
            raise AssertionError(f"{trial_path.name} candidate reference mismatch")
        missing = sorted(set(task["required_capabilities"]) - set(driver["provided_capabilities"]))
        if (
            trial["support_status"] != ("unsupported" if missing else "supported")
            or sorted(trial["missing_capabilities"]) != missing
        ):
            raise AssertionError(f"{trial_path.name} support status mismatch")

        resolved = trial["resolved_driver"]
        if profile_id in {
            "native-bundle",
            "bare-driver",
            "driver-only-diagnostic",
        }:
            if resolved["tool_schemas_digest"] != driver["interface"]["tool_schemas_digest"]:
                raise AssertionError(f"{trial_path.name} native tool digest mismatch")
            if resolved["tool_names"] != [tool["name"] for tool in driver["interface"]["tools"]]:
                raise AssertionError(f"{trial_path.name} native tool order mismatch")
        else:
            facade_artifact = dataset["freeze"]["inputs"]["normalized_facade_contract"]
            facade_path = validate_artifact_bytes(
                dataset_path, facade_artifact, "normalized facade"
            )
            facade = load_json(facade_path)
            if facade["tool_schemas_digest_algorithm"] != "sha256-rfc8785":
                raise AssertionError("facade uses an unsupported tool digest algorithm")
            for tool in facade["tools"]:
                try:
                    Draft202012Validator.check_schema(tool["input_schema"])
                    if "output_schema" in tool:
                        Draft202012Validator.check_schema(tool["output_schema"])
                except SchemaError as error:
                    raise AssertionError("facade embeds an invalid JSON Schema") from error
            if resolved["tool_schemas_digest"] != json_schema_digest(facade["tools"]):
                raise AssertionError(f"{trial_path.name} facade digest mismatch")
            if resolved["tool_names"] != [tool["name"] for tool in facade["tools"]]:
                raise AssertionError(f"{trial_path.name} facade tool order mismatch")

        skill_delivery = resolved["skill_delivery"]
        validate_delivery(trial_path, skill_delivery, "driver skill delivery")
        if profile_id == "native-bundle":
            if "skill" in driver:
                if skill_delivery["status"] != "delivered":
                    raise AssertionError("native bundle did not deliver its declared skill")
                if artifact_identity(
                    trial_path, skill_delivery["source_artifact"]
                ) != artifact_identity(driver_path, driver["skill"]["artifact"]):
                    raise AssertionError("delivered skill differs from driver manifest")
                if driver["skill"].get("generator_version") != skill_delivery.get(
                    "generator_version"
                ):
                    raise AssertionError("delivered skill generator version mismatch")
            elif skill_delivery["status"] != "not-applicable":
                raise AssertionError("skill-free driver must record not-applicable")
        elif skill_delivery["status"] != (
            "not-delivered" if "skill" in driver else "not-applicable"
        ):
            raise AssertionError("removed driver skill has inconsistent status")
        if "neutral_guidance_delivery" in resolved:
            validate_delivery(
                trial_path,
                resolved["neutral_guidance_delivery"],
                "neutral guidance delivery",
            )
            neutral = dataset["freeze"]["inputs"]["neutral_instructions"]
            if artifact_identity(
                trial_path,
                resolved["neutral_guidance_delivery"]["source_artifact"],
            ) != artifact_identity(dataset_path, neutral):
                raise AssertionError("neutral guidance differs from dataset freeze")

    if trial["eligibility_status"] == "eligible":
        driver_skill_tokens = 0
        neutral_guidance_tokens = 0
        if "resolved_driver" in trial:
            skill_delivery = trial["resolved_driver"]["skill_delivery"]
            if skill_delivery["status"] == "delivered":
                driver_skill_tokens = skill_delivery["token_count"]
            neutral_delivery = trial["resolved_driver"].get("neutral_guidance_delivery")
            if neutral_delivery and neutral_delivery["status"] == "delivered":
                neutral_guidance_tokens = neutral_delivery["token_count"]
        tokens = trial["observables"]["tokens"]
        if tokens["driver_skill"] != driver_skill_tokens:
            raise AssertionError("driver skill token accounting mismatch")
        if tokens["neutral_guidance"] != neutral_guidance_tokens:
            raise AssertionError("neutral guidance token accounting mismatch")

    if trial["eligibility_status"] == "eligible":
        validate_artifact_bytes(trial_path, trial["execution"]["event_log"], "trial event log")
        for artifact in trial["evidence"]:
            validate_artifact_bytes(trial_path, artifact, "trial evidence")
        started = datetime.fromisoformat(trial["execution"]["started_at"].replace("Z", "+00:00"))
        ended = datetime.fromisoformat(trial["execution"]["ended_at"].replace("Z", "+00:00"))
        if ended < started:
            raise AssertionError(f"{trial_path.name} ends before it starts")


def validate_v03_system(system_path: Path, system: dict[str, Any]) -> None:
    for label, artifact in (
        ("system harness build", system["harness"]["build"]),
        ("system harness configuration", system["harness"]["configuration"]),
        ("system tool inventory", system["capability_inventory"]["tools"]),
        ("system skill inventory", system["capability_inventory"]["skills"]),
    ):
        validate_artifact_bytes(system_path, artifact, label)
    route_ids = [route["id"] for route in system["model_routing"]["routes"]]
    if len(route_ids) != len(set(route_ids)):
        raise AssertionError("system model route ids must be unique")


def validate_v03_policy(policy_path: Path, policy: dict[str, Any]) -> None:
    validate_artifact_bytes(
        policy_path,
        policy["accounting"]["model_price_table"],
        "execution policy model price table",
    )
    validate_artifact_bytes(
        policy_path,
        policy["credential_state_profile"],
        "execution policy credential state profile",
    )


def validate_v03_trial(
    trial_path: Path,
    trial: dict[str, Any],
    system_path: Path,
    system: dict[str, Any],
    policy_path: Path,
    policy: dict[str, Any],
) -> None:
    system_digest = hashlib.sha256(system_path.read_bytes()).hexdigest()
    policy_digest = hashlib.sha256(policy_path.read_bytes()).hexdigest()
    expected_system = {
        "id": system["id"],
        "version": system["version"],
        "manifest_sha256": system_digest,
    }
    expected_policy = {
        "id": policy["id"],
        "version": policy["version"],
        "manifest_sha256": policy_digest,
    }
    if trial["system"] != expected_system:
        raise AssertionError("trial system reference mismatch")
    if trial["execution_policy"] != expected_policy:
        raise AssertionError("trial execution policy reference mismatch")
    if trial["bindings"]["system_digest"] != system_digest:
        raise AssertionError("trial system digest binding mismatch")
    if trial["bindings"]["execution_policy_digest"] != policy_digest:
        raise AssertionError("trial execution policy digest binding mismatch")
    execution = trial.get("execution")
    if execution is None:
        return
    certification = execution.get("certification")
    if certification is not None:
        expected_certification_bindings = {
            "trial_id": trial["id"],
            "task_digest": trial["task"]["manifest_sha256"],
            "system_digest": system_digest,
            "execution_policy_digest": policy_digest,
            "resolved_seed_provenance_sha256": trial["environment"][
                "resolved_seed_provenance_sha256"
            ],
            "apparatus_digest": trial["bindings"]["apparatus_digest"],
        }
        if certification["bindings"] != expected_certification_bindings:
            raise AssertionError("trial apparatus certification bindings mismatch")
        if certification["certifying"] and certification["apparatus_status"] != "passed":
            raise AssertionError("certifying trial has non-passing apparatus")
        receipt = certification.get("receipt")
        signature = certification.get("signature")
        if receipt is not None:
            validate_artifact_bytes(trial_path, receipt, "apparatus certification receipt")
            receipt_document = load_json((trial_path.parent / receipt["path"]).resolve())
            if receipt_document.get("bindings", {}).get("trial_id") != trial["id"]:
                raise AssertionError("apparatus receipt trial binding mismatch")
            if (
                receipt_document.get("eligible") is not certification["certifying"]
                or receipt_document.get("apparatus_decision", {}).get("status")
                != certification["apparatus_status"]
            ):
                raise AssertionError("trial apparatus certification contradicts attached receipt")
        if signature is not None:
            validate_artifact_bytes(trial_path, signature, "apparatus certification signature")

    observed = execution["observed"]
    declared_routes = {
        route["id"]: (
            route["role"],
            route["provider"],
            route["model"],
            route["snapshot"],
            route["service_tier"],
        )
        for route in system["model_routing"]["routes"]
    }
    for call in observed["model_calls"]:
        identity = (
            call["role"],
            call["provider"],
            call["model"],
            call["snapshot"],
            call["service_tier"],
        )
        if declared_routes.get(call["route_id"]) != identity:
            raise AssertionError("observed undeclared or mismatched model route")

    if observed["human_interventions"] > policy["autonomy"]["max_human_interventions"]:
        raise AssertionError("observed human intervention exceeds execution policy")
    if observed["approval_prompts"]:
        raise AssertionError("observed approval prompt violates autonomous execution")
    if not observed["fresh_harness_workspace"]["satisfied"]:
        raise AssertionError("fresh harness workspace was not observed")
    if not observed["target_reset"]["satisfied"]:
        raise AssertionError("target reset was not observed")
    if observed["applied_network_mode"] != policy["network"]["mode"]:
        raise AssertionError("applied network mode differs from execution policy")
    if observed.get("applied_network_allowlist_sha256") != policy["network"].get(
        "allowlist_sha256"
    ):
        raise AssertionError("applied network allowlist differs from execution policy")
    if observed["applied_permission_policy_sha256"] != policy["permissions"]["policy_sha256"]:
        raise AssertionError("applied permission policy digest mismatch")
    if observed["cost_basis"] != policy["accounting"]["cost_basis"]:
        raise AssertionError("observed cost basis differs from execution policy")
    if observed["model_price_table_sha256"] != policy["accounting"]["model_price_table"]["sha256"]:
        raise AssertionError("observed model price table digest mismatch")
    observed_tokens = observed["tokens"]
    graded_tokens = trial["observables"]["tokens"]
    route_tokens = [call["tokens"] for call in observed["model_calls"]]
    comparison = execution.get("comparison_eligibility")
    comparison_eligible = (
        comparison.get("status") == "eligible" if isinstance(comparison, dict) else True
    )
    token_telemetry_available = observed_tokens is not None
    if comparison_eligible:
        if (
            not token_telemetry_available
            or graded_tokens is None
            or any(tokens is None for tokens in route_tokens)
        ):
            raise AssertionError("comparison-eligible trial has incomplete token telemetry")
        if observed_tokens["includes_subagents"] is not True:
            raise AssertionError("observed token totals omit subagents")
        if trial["observables"]["cost_usd"] is None:
            raise AssertionError("comparison-eligible trial has no comparable cost")
    elif graded_tokens is not None or trial["observables"]["cost_usd"] is not None:
        raise AssertionError("comparison-ineligible trial exposes comparable accounting")
    if token_telemetry_available:
        if all(tokens is not None for tokens in route_tokens):
            route_totals = {
                field: sum(tokens[field] for tokens in route_tokens)
                for field in ("input", "output", "cache_read", "cache_write")
            }
            if any(observed_tokens[field] != value for field, value in route_totals.items()):
                raise AssertionError("observed token totals differ from model routes")
        if graded_tokens is not None and any(
            graded_tokens[field] != observed_tokens[field]
            for field in ("input", "output", "cache_read", "cache_write")
        ):
            raise AssertionError("graded token totals differ from observed execution")
        if observed_tokens["input"] + observed_tokens["output"] > policy["limits"]["total_tokens"]:
            raise AssertionError("observed token total exceeds execution policy")
    if trial["observables"]["wall_time_ms"] > policy["limits"]["wall_time_ms"]:
        raise AssertionError("observed wall time exceeds execution policy")
    if (
        trial["observables"]["cost_usd"] is not None
        and trial["observables"]["cost_usd"] > policy["limits"]["cost_usd"]
    ):
        raise AssertionError("observed cost exceeds execution policy")
    if trial["attempt_index"] >= policy["limits"]["attempts_per_task"]:
        raise AssertionError("attempt index exceeds execution policy")
    if (
        policy["isolation"]["persistent_cache"] == "forbidden"
        and observed["cache_state"]["mode"] != "empty"
    ):
        raise AssertionError("observed cache state violates execution policy")
    if (
        policy["isolation"]["package_installation"] == "forbidden"
        and observed["package_installations"]
    ):
        raise AssertionError("observed package installation violates execution policy")
    expected_persistent = (
        "absent" if policy["isolation"]["persistent_cache"] == "forbidden" else "declared_read_only"
    )
    if observed["persistent_state"]["mode"] != expected_persistent:
        raise AssertionError("observed persistent state violates execution policy")

    for label, fact in (
        ("fresh harness workspace evidence", observed["fresh_harness_workspace"]),
        ("target reset evidence", observed["target_reset"]),
    ):
        validate_artifact_bytes(trial_path, fact["evidence"], label)
    validate_artifact_bytes(trial_path, observed["cache_state"]["evidence"], "cache state evidence")
    validate_artifact_bytes(
        trial_path,
        observed["persistent_state"]["evidence"],
        "persistent state evidence",
    )


def validate_v03_cases(
    validators: dict[str, Draft202012Validator],
    system_path: Path,
    system: dict[str, Any],
    policy_path: Path,
    policy: dict[str, Any],
) -> tuple[int, int]:
    declaration = load_json(FIXTURE_DIR / "v03-cases.json")
    structural = 0
    semantic = 0
    for case in declaration["cases"]:
        path = (FIXTURE_DIR / case["base"]).resolve()
        instance = copy.deepcopy(load_json(path))
        for location, value in case.get("replace", {}).items():
            parent, key = resolve_pointer(instance, location)
            if isinstance(parent, list):
                parent[int(key)] = value
            else:
                parent[key] = value
        for location in case.get("remove", []):
            parent, key = resolve_pointer(instance, location)
            if isinstance(parent, list):
                del parent[int(key)]
            else:
                del parent[key]
        kind = case["kind"]
        errors = list(validators[f"{kind}.schema.json"].iter_errors(instance))
        if case["class"] == "structural":
            if not errors:
                raise AssertionError(f"v0.3 structural case {case['name']!r} unexpectedly passed")
            structural += 1
            continue
        if errors:
            raise AssertionError(f"v0.3 semantic case {case['name']!r} failed schema validation")
        try:
            if kind == "system":
                validate_v03_system(path, instance)
            elif kind == "execution-policy":
                validate_v03_policy(path, instance)
            elif kind == "trial":
                validate_v03_trial(path, instance, system_path, system, policy_path, policy)
            else:
                raise AssertionError(f"unknown v0.3 case kind: {kind}")
        except AssertionError as error:
            if case["error_contains"] not in str(error):
                raise AssertionError(
                    f"v0.3 semantic case {case['name']!r} failed for the wrong reason: {error}"
                ) from error
        else:
            raise AssertionError(f"v0.3 semantic case {case['name']!r} unexpectedly passed")
        semantic += 1
    return structural, semantic


def apparatus_artifacts(apparatus: dict[str, Any]) -> list[dict[str, str]]:
    return [
        *apparatus["harness_builds"],
        *apparatus["environment_images"],
        *apparatus["observers"],
        *apparatus["evaluators"],
        apparatus["normalized_facade_contract"],
        apparatus["neutral_instructions"],
    ]


def release_artifacts(inputs: dict[str, Any]) -> list[dict[str, str]]:
    return [
        inputs["runtime"]["artifact"],
        inputs["dataset"]["manifest"],
        *apparatus_artifacts(inputs["apparatus"]),
        *(profile["manifest"] for profile in inputs["profiles"]),
        *(candidate["manifest"] for candidate in inputs["candidates"]),
        *(trial["manifest"] for trial in inputs["trials"]),
        *inputs["report_inputs"],
        inputs["tool_surface_study"]["backend"]["manifest"],
        *(
            artifact
            for arm in inputs["tool_surface_study"]["arms"]
            for artifact in (arm["contract"], arm["trials"], arm["result"])
        ),
    ]


def trial_coordinates(trial: dict[str, Any]) -> dict[str, Any]:
    return {
        "task": trial["task"],
        "dataset": trial["dataset"],
        "variant": trial["variant"],
        "pairing_key": trial["pairing_key"],
        "attempt_index": trial["attempt_index"],
        "model": trial["model"],
        "provider": trial["provider"],
        "harness": trial["harness"],
        "environment": trial["environment"],
    }


def validate_release(
    release_path: Path,
    release: dict[str, Any],
    validators: dict[str, Draft202012Validator],
) -> None:
    inputs = release["inputs"]
    for artifact in release_artifacts(inputs):
        validate_artifact_bytes(release_path, artifact, "release input")

    runtime_path = validate_artifact_bytes(
        release_path, inputs["runtime"]["artifact"], "runtime artifact"
    )
    runtime = load_json(runtime_path)
    if runtime.get("revision") != inputs["runtime"]["source_revision"]:
        raise AssertionError("runtime artifact revision mismatch")

    apparatus_preimage = {
        key: value for key, value in inputs["apparatus"].items() if key != "digest"
    }
    actual_apparatus = canonical_digest(apparatus_preimage)
    if actual_apparatus != inputs["apparatus"]["digest"]:
        raise AssertionError(f"release apparatus digest mismatch: {actual_apparatus}")

    dataset_path = resolve_relative(release_path.parent, inputs["dataset"]["manifest"]["path"])
    dataset = load_json(dataset_path)
    validators["dataset.schema.json"].validate(dataset)
    validate_freeze(
        dataset_path,
        dataset,
        validators["task.schema.json"],
        check_mutation=False,
    )
    if (
        inputs["dataset"]["id"],
        inputs["dataset"]["version"],
        inputs["dataset"]["freeze_digest"],
    ) != (dataset["id"], dataset["version"], dataset["freeze"]["digest"]):
        raise AssertionError("release dataset reference mismatch")

    profile_entries = inputs["profiles"]
    profile_ids = [entry["id"] for entry in profile_entries]
    if len(profile_ids) != len(set(profile_ids)):
        raise AssertionError("release profile ids must be unique")
    required_profiles = {
        "native-bundle",
        "bare-driver",
        "harness-native-reference",
        "normalized-facade",
    }
    if not required_profiles.issubset(profile_ids):
        raise AssertionError("release omits a required decision-bearing profile")
    profiles: dict[str, tuple[Path, dict[str, Any]]] = {}
    for entry in profile_entries:
        path = resolve_relative(release_path.parent, entry["manifest"]["path"])
        profile = load_json(path)
        validators["profile.schema.json"].validate(profile)
        validate_profile(path, profile)
        if (entry["id"], entry["version"]) != (
            profile["id"],
            profile["version"],
        ):
            raise AssertionError("release profile reference mismatch")
        profiles[profile["id"]] = (path, profile)

    normalized_path, normalized_profile = profiles["normalized-facade"]
    if artifact_identity(
        normalized_path, normalized_profile["guidance"]["artifact"]
    ) != artifact_identity(dataset_path, dataset["freeze"]["inputs"]["neutral_instructions"]):
        raise AssertionError("normalized profile guidance differs from dataset freeze")

    candidates: dict[str, tuple[Path, dict[str, Any]]] = {}
    for entry in inputs["candidates"]:
        if entry["id"] in candidates:
            raise AssertionError("release candidate ids must be unique")
        path = resolve_relative(release_path.parent, entry["manifest"]["path"])
        driver = load_json(path)
        validators["driver.schema.json"].validate(driver)
        validate_driver(path, driver)
        if (entry["id"], entry["version"]) != (driver["id"], driver["version"]):
            raise AssertionError("release candidate reference mismatch")
        candidates[driver["id"]] = (path, driver)

    study = inputs["tool_surface_study"]
    backend = study["backend"]
    if backend["id"] not in candidates:
        raise AssertionError("tool-surface backend is absent from release candidates")
    candidate_entry = next(entry for entry in inputs["candidates"] if entry["id"] == backend["id"])
    if backend != candidate_entry:
        raise AssertionError("tool-surface backend reference mismatch")
    _, backend_driver = candidates[backend["id"]]
    if study["capabilities"] != backend_driver["provided_capabilities"]:
        raise AssertionError("tool-surface capabilities differ from backend")
    if canonical_digest(study["capabilities"]) != study["capability_digest"]:
        raise AssertionError("tool-surface capability digest mismatch")

    apparatus = inputs["apparatus"]
    if {artifact_identity(release_path, artifact) for artifact in apparatus["evaluators"]} != {
        artifact_identity(dataset_path, artifact)
        for artifact in dataset["freeze"]["inputs"]["evaluator_artifacts"]
    }:
        raise AssertionError("apparatus evaluators differ from dataset freeze")
    for field in ("normalized_facade_contract", "neutral_instructions"):
        if artifact_identity(release_path, apparatus[field]) != artifact_identity(
            dataset_path, dataset["freeze"]["inputs"][field]
        ):
            raise AssertionError(f"apparatus {field} differs from dataset freeze")

    tasks: dict[tuple[str, str], tuple[Path, dict[str, Any]]] = {}
    for selection in dataset["tasks"]:
        task_path = resolve_relative(dataset_path.parent, selection["manifest"]["path"])
        task = load_json(task_path)
        validators["task.schema.json"].validate(task)
        tasks[(task["id"], task["version"])] = (task_path, task)
    groups: dict[tuple[str, str], dict[str, dict[str, Any]]] = defaultdict(dict)
    trial_ids: set[str] = set()
    for entry in inputs["trials"]:
        if entry["id"] in trial_ids:
            raise AssertionError("release trial ids must be unique")
        trial_ids.add(entry["id"])
        candidate_id = entry["comparison_candidate_id"]
        if candidate_id not in candidates:
            raise AssertionError("trial comparison candidate is absent from release")
        path = resolve_relative(release_path.parent, entry["manifest"]["path"])
        trial = load_json(path)
        validators["trial.schema.json"].validate(trial)
        task_key = (trial["task"]["id"], trial["task"]["version"])
        if task_key not in tasks:
            raise AssertionError("release trial task is absent from dataset")
        task_path, task = tasks[task_key]
        driver_path, driver = candidates[candidate_id]
        validate_trial(
            path,
            trial,
            task_path,
            task,
            dataset_path,
            dataset,
            profiles,
            driver_path,
            driver,
        )
        if artifact_identity(path, trial["harness"]["build"]) not in {
            artifact_identity(release_path, artifact) for artifact in apparatus["harness_builds"]
        }:
            raise AssertionError("trial harness build is absent from apparatus")
        if artifact_identity(path, trial["environment"]["image"]) not in {
            artifact_identity(release_path, artifact)
            for artifact in apparatus["environment_images"]
        }:
            raise AssertionError("trial environment image is absent from apparatus")
        if artifact_identity(path, trial["environment"]["observer"]) not in {
            artifact_identity(release_path, artifact) for artifact in apparatus["observers"]
        }:
            raise AssertionError("trial observer is absent from apparatus")
        if (entry["id"], entry["profile_id"], entry["pairing_key"]) != (
            trial["id"],
            trial["profile"]["id"],
            trial["pairing_key"],
        ):
            raise AssertionError("release trial reference mismatch")
        if trial["pre_freeze"]:
            raise AssertionError("decision-bearing release includes a pre-freeze trial")
        if trial["bindings"] != {
            "dataset_freeze_digest": inputs["dataset"]["freeze_digest"],
            "apparatus_digest": inputs["apparatus"]["digest"],
        }:
            raise AssertionError("release trial bindings mismatch")
        if (
            trial["profile"]["id"] != "harness-native-reference"
            and trial["candidate"]["id"] != candidate_id
        ):
            raise AssertionError("trial candidate differs from comparison candidate")
        if entry["profile_id"] in required_profiles:
            group = groups[(candidate_id, entry["pairing_key"])]
            if entry["profile_id"] in group:
                raise AssertionError("duplicate profile trial in pairing group")
            group[entry["profile_id"]] = trial

    covered_candidates = {candidate_id for candidate_id, _ in groups}
    if covered_candidates != set(candidates):
        raise AssertionError("each release candidate needs a complete pairing group")
    for (candidate_id, pairing_key), group in groups.items():
        if set(group) != required_profiles:
            raise AssertionError(
                f"pairing group {candidate_id}/{pairing_key} lacks required profiles"
            )
        coordinates = [canonical_digest(trial_coordinates(trial)) for trial in group.values()]
        if len(set(coordinates)) != 1:
            raise AssertionError(
                f"pairing group {candidate_id}/{pairing_key} changes paired coordinates"
            )

    arms = [arm["id"] for arm in inputs["tool_surface_study"]["arms"]]
    if set(arms) != {"consolidated", "factored", "fine-grained"} or len(set(arms)) != 3:
        raise AssertionError("tool-surface study must contain the three canonical arms")
    for arm in inputs["tool_surface_study"]["arms"]:
        for field in ("contract", "trials", "result"):
            artifact_path = resolve_relative(release_path.parent, arm[field]["path"])
            artifact = load_json(artifact_path)
            if artifact.get("arm") != arm["id"]:
                raise AssertionError(f"tool-surface {field} does not identify arm {arm['id']}")
            if field == "trials" and not artifact.get("trial_ids"):
                raise AssertionError("tool-surface arm has no trial inventory")

    actual_release = canonical_digest(inputs)
    if actual_release != release["digest"]["value"]:
        raise AssertionError(f"release digest mismatch: {actual_release}")

    fixture = load_json(FIXTURE_DIR / "release-digest.json")
    if (FIXTURE_DIR / fixture["release"]).resolve() != release_path.resolve():
        raise AssertionError("release mutation fixture names a different release")
    changed_bytes = fixture["changed_bytes_utf8"].encode("utf-8")
    changed_sha = hashlib.sha256(changed_bytes).hexdigest()
    if changed_sha != fixture["expected_input_sha256"]:
        raise AssertionError("changed release input digest does not match fixture")
    changed = copy.deepcopy(inputs)
    parent, key = resolve_pointer(changed, fixture["changed_input"])
    parent[int(key)]["sha256"] = changed_sha
    changed_release = canonical_digest(changed)
    if changed_release != fixture["expected_release_digest"]:
        raise AssertionError("changed release digest does not match fixture")
    if changed_release == actual_release:
        raise AssertionError("changing a release input did not change release digest")


def validate_semantic_negative_cases(
    validators: dict[str, Draft202012Validator],
    task_path: Path,
    task: dict[str, Any],
    dataset_path: Path,
    dataset: dict[str, Any],
    profiles: dict[str, tuple[Path, dict[str, Any]]],
    driver_path: Path,
    driver: dict[str, Any],
) -> int:
    declaration = load_json(FIXTURE_DIR / "semantic-negative-cases.json")
    for case in declaration["cases"]:
        path = (FIXTURE_DIR / case["base"]).resolve()
        instance = copy.deepcopy(load_json(path))
        for destination, source in case.get("copy", {}).items():
            parent, key = resolve_pointer(instance, destination)
            value = copy.deepcopy(read_pointer(instance, source))
            if isinstance(parent, list):
                parent[int(key)] = value
            else:
                parent[key] = value
        for location, value in case.get("replace", {}).items():
            parent, key = resolve_pointer(instance, location)
            if isinstance(parent, list):
                parent[int(key)] = value
            else:
                parent[key] = value
        for location in case.get("remove", []):
            parent, key = resolve_pointer(instance, location)
            if isinstance(parent, list):
                del parent[int(key)]
            else:
                del parent[key]

        target = case["target"]
        try:
            if target == "driver":
                validators["driver.schema.json"].validate(instance)
                validate_driver(path, instance)
            elif target == "trial":
                validators["trial.schema.json"].validate(instance)
                validate_trial(
                    path,
                    instance,
                    task_path,
                    task,
                    dataset_path,
                    dataset,
                    profiles,
                    driver_path,
                    driver,
                )
            elif target == "release":
                validators["release.schema.json"].validate(instance)
                validate_release(path, instance, validators)
            else:
                raise AssertionError(f"unknown semantic fixture target: {target}")
        except AssertionError as error:
            if case["error_contains"] not in str(error):
                raise AssertionError(
                    f"semantic case {case['name']!r} failed for the wrong reason: {error}"
                ) from error
        else:
            raise AssertionError(f"semantic case {case['name']!r} unexpectedly passed")
    return len(declaration["cases"])


def main() -> int:
    schema_paths = sorted(SCHEMA_DIR.glob("*.schema.json"))
    schemas = [load_json(path) for path in schema_paths]
    registry = build_registry(schemas)
    by_name = {path.name: schema for path, schema in zip(schema_paths, schemas)}
    validators = {
        name: Draft202012Validator(schema, registry=registry, format_checker=FormatChecker())
        for name, schema in by_name.items()
    }

    task_example = load_json(EXAMPLE_DIR / "task.cuabench.json")
    validators["task.schema.json"].validate(task_example)
    validate_unique_task_ids(task_example, "task.cuabench.json")
    dataset_path = EXAMPLE_DIR / "dataset.cuabench.json"
    dataset = load_json(dataset_path)
    validators["dataset.schema.json"].validate(dataset)
    validate_freeze(dataset_path, dataset, validators["task.schema.json"])

    driver_path = EXAMPLE_DIR / "driver.cuabench.json"
    driver = load_json(driver_path)
    validators["driver.schema.json"].validate(driver)
    validate_driver(driver_path, driver)

    profiles: dict[str, tuple[Path, dict[str, Any]]] = {}
    for profile_path in sorted(EXAMPLE_DIR.glob("profile.*.cuabench.json")):
        profile = load_json(profile_path)
        validators["profile.schema.json"].validate(profile)
        validate_profile(profile_path, profile)
        if profile["id"] in profiles:
            raise AssertionError(f"duplicate profile example: {profile['id']}")
        profiles[profile["id"]] = (profile_path, profile)

    trial_count = 0
    for trial_path in sorted(EXAMPLE_DIR.glob("trial.*.cuabench.json")):
        trial = load_json(trial_path)
        if trial.get("schema_version") != "0.1.0":
            continue
        validators["trial.schema.json"].validate(trial)
        validate_trial(
            trial_path,
            trial,
            EXAMPLE_DIR / "task.cuabench.json",
            task_example,
            dataset_path,
            dataset,
            profiles,
            driver_path,
            driver,
        )
        trial_count += 1

    release_path = EXAMPLE_DIR / "release.cuabench.json"
    release = load_json(release_path)
    validators["release.schema.json"].validate(release)
    validate_release(release_path, release, validators)
    negative_count = validate_negative_cases(validators, by_name)
    semantic_count = validate_semantic_negative_cases(
        validators,
        EXAMPLE_DIR / "task.cuabench.json",
        task_example,
        dataset_path,
        dataset,
        profiles,
        driver_path,
        driver,
    )

    v02_schema_paths = sorted(SCHEMA_V02_DIR.glob("*.schema.json"))
    v02_schemas = [load_json(path) for path in v02_schema_paths]
    v02_registry = build_registry(v02_schemas)
    v02_validators = {
        path.name: Draft202012Validator(
            schema, registry=v02_registry, format_checker=FormatChecker()
        )
        for path, schema in zip(v02_schema_paths, v02_schemas)
    }
    participation_task_path = EXAMPLE_DIR / "task.cuabench.json"
    participation_task = copy.deepcopy(task_example)
    participation_task["schema_version"] = "0.2.0"
    participation_task["participation_requirements"] = [
        {
            "id": "participation.synthetic.write-note",
            "target": {
                "application_id": "application.synthetic-notes",
                "surface_id": "surface.synthetic-note",
            },
            "sequence": [
                {"kind": "observe"},
                {
                    "kind": "act",
                    "required_facts": {"action": "write-note"},
                },
                {
                    "kind": "readback",
                    "required_facts": {"state": "saved"},
                },
            ],
        }
    ]
    v02_validators["task.schema.json"].validate(participation_task)
    validate_unique_task_ids(participation_task, str(participation_task_path))

    participation_trial = copy.deepcopy(
        load_json(EXAMPLE_DIR / "trial.native-bundle.cuabench.json")
    )
    participation_trial["schema_version"] = "0.2.0"
    participation_body = {
        "required": True,
        "status": "passed",
        "passed": True,
        "observer": {"name": "fixture observer", "trust": "certifying"},
        "requirements": [
            {
                "requirement_id": "participation.fixture.observation",
                "status": "satisfied",
                "evidence_event_hashes": ["1" * 64],
            }
        ],
        "bindings": {
            "trial_id": participation_trial["id"],
            "task_digest": "sha256:" + "2" * 64,
            "config_digest": "sha256:" + "3" * 64,
        },
    }
    participation_trial["execution"]["participation"] = {
        **participation_body,
        "receipt_digest": "sha256:" + json_schema_digest(participation_body),
    }
    v02_validators["trial.schema.json"].validate(participation_trial)

    for alternate_body in (
        {
            **participation_body,
            "status": "failed",
            "passed": False,
            "observer": {
                "name": "fixture observer",
                "trust": "non_certifying",
            },
            "requirements": [
                {
                    "requirement_id": "participation.fixture.observation",
                    "status": "unsatisfied",
                    "evidence_event_hashes": [],
                    "reason": "no matching event",
                }
            ],
        },
        {
            **participation_body,
            "status": "unavailable",
            "passed": None,
            "observer": {"name": "fixture observer", "trust": "unavailable"},
            "requirements": [
                {
                    "requirement_id": "participation.fixture.observation",
                    "status": "unavailable",
                    "evidence_event_hashes": [],
                    "reason": "observer unavailable",
                }
            ],
        },
        {
            **participation_body,
            "required": False,
            "status": "not_required",
            "passed": None,
            "observer": {"name": "fixture observer", "trust": "unavailable"},
            "requirements": [],
        },
    ):
        alternate_trial = copy.deepcopy(participation_trial)
        alternate_trial["execution"]["participation"] = {
            **alternate_body,
            "receipt_digest": "sha256:" + json_schema_digest(alternate_body),
        }
        v02_validators["trial.schema.json"].validate(alternate_trial)

    scored_participation = copy.deepcopy(participation_trial)
    scored_participation["outcomes"]["participation"] = True
    if not list(v02_validators["trial.schema.json"].iter_errors(scored_participation)):
        raise AssertionError("trial outcomes unexpectedly accept participation")

    inconsistent_participation = copy.deepcopy(participation_trial)
    inconsistent_participation["execution"]["participation"]["passed"] = False
    if not list(v02_validators["trial.schema.json"].iter_errors(inconsistent_participation)):
        raise AssertionError("participation status unexpectedly disagrees with passed")

    missing_bindings = copy.deepcopy(participation_trial)
    del missing_bindings["execution"]["participation"]["bindings"]
    if not list(v02_validators["trial.schema.json"].iter_errors(missing_bindings)):
        raise AssertionError("participation receipt unexpectedly omits bindings")

    unprefixed_receipt_digest = copy.deepcopy(participation_trial)
    unprefixed_receipt_digest["execution"]["participation"]["receipt_digest"] = "4" * 64
    if not list(v02_validators["trial.schema.json"].iter_errors(unprefixed_receipt_digest)):
        raise AssertionError("participation receipt unexpectedly accepts bare digest")

    unprefixed_binding_digest = copy.deepcopy(participation_trial)
    unprefixed_binding_digest["execution"]["participation"]["bindings"]["task_digest"] = "5" * 64
    if not list(v02_validators["trial.schema.json"].iter_errors(unprefixed_binding_digest)):
        raise AssertionError("participation binding unexpectedly accepts bare digest")

    prefixed_event_hash = copy.deepcopy(participation_trial)
    prefixed_event_hash["execution"]["participation"]["requirements"][0][
        "evidence_event_hashes"
    ] = ["sha256:" + "6" * 64]
    if not list(v02_validators["trial.schema.json"].iter_errors(prefixed_event_hash)):
        raise AssertionError("participation event hash unexpectedly accepts prefix")

    invalid_observer_trust = copy.deepcopy(participation_trial)
    invalid_observer_trust["execution"]["participation"]["observer"]["trust"] = "non-certifying"
    if not list(v02_validators["trial.schema.json"].iter_errors(invalid_observer_trust)):
        raise AssertionError("participation receipt unexpectedly accepts hyphenated trust")

    v03_schema_paths = sorted(SCHEMA_V03_DIR.glob("*.schema.json"))
    v03_schemas = [load_json(path) for path in v03_schema_paths]
    v03_registry = build_registry(v03_schemas)
    v03_validators = {
        path.name: Draft202012Validator(
            schema, registry=v03_registry, format_checker=FormatChecker()
        )
        for path, schema in zip(v03_schema_paths, v03_schemas)
    }
    system_path = EXAMPLE_DIR / "system.cuabench.json"
    system = load_json(system_path)
    v03_validators["system.schema.json"].validate(system)
    validate_v03_system(system_path, system)
    policy_path = EXAMPLE_DIR / "execution-policy.cuabench.json"
    policy = load_json(policy_path)
    v03_validators["execution-policy.schema.json"].validate(policy)
    validate_v03_policy(policy_path, policy)
    system_trial_path = EXAMPLE_DIR / "trial.system-track.cuabench.json"
    system_trial = load_json(system_trial_path)
    v03_validators["trial.schema.json"].validate(system_trial)
    validate_v03_trial(system_trial_path, system_trial, system_path, system, policy_path, policy)

    executed_ineligible = copy.deepcopy(system_trial)
    executed_ineligible["execution"]["comparison_eligibility"] = {
        "status": "ineligible",
        "reasons": ["model_telemetry_unavailable"],
    }
    executed_ineligible["execution"]["observed"]["tokens"] = None
    for call in executed_ineligible["execution"]["observed"]["model_calls"]:
        call["tokens"] = None
    executed_ineligible["observables"]["tokens"] = None
    executed_ineligible["observables"]["cost_usd"] = None
    v03_validators["trial.schema.json"].validate(executed_ineligible)
    validate_v03_trial(
        system_trial_path,
        executed_ineligible,
        system_path,
        system,
        policy_path,
        policy,
    )

    partial_accounting_ineligible = copy.deepcopy(system_trial)
    partial_accounting_ineligible["execution"]["comparison_eligibility"] = {
        "status": "ineligible",
        "reasons": ["model_telemetry_unavailable"],
    }
    partial_accounting_ineligible["execution"]["observed"]["tokens"]["includes_subagents"] = False
    partial_accounting_ineligible["observables"]["tokens"] = None
    partial_accounting_ineligible["observables"]["cost_usd"] = None
    v03_validators["trial.schema.json"].validate(partial_accounting_ineligible)
    validate_v03_trial(
        system_trial_path,
        partial_accounting_ineligible,
        system_path,
        system,
        policy_path,
        policy,
    )

    eligible_with_partial_accounting = copy.deepcopy(system_trial)
    eligible_with_partial_accounting["execution"]["observed"]["tokens"]["includes_subagents"] = (
        False
    )
    try:
        validate_v03_trial(
            system_trial_path,
            eligible_with_partial_accounting,
            system_path,
            system,
            policy_path,
            policy,
        )
    except AssertionError as error:
        if "omit subagents" not in str(error):
            raise
    else:
        raise AssertionError(
            "comparison-eligible trial unexpectedly accepts partial token accounting"
        )

    pre_execution_ineligible = copy.deepcopy(system_trial)
    pre_execution_ineligible["eligibility_status"] = "ineligible"
    pre_execution_ineligible["eligibility_evidence"] = [
        "Required environment capability is unavailable."
    ]
    for field in (
        "execution",
        "termination_status",
        "outcomes",
        "observables",
        "evidence",
    ):
        del pre_execution_ineligible[field]
    pre_execution_ineligible["truncated"] = False
    v03_validators["trial.schema.json"].validate(pre_execution_ineligible)

    eligible_without_accounting = copy.deepcopy(executed_ineligible)
    eligible_without_accounting["execution"]["comparison_eligibility"] = {
        "status": "eligible",
        "reasons": [],
    }
    if not list(v03_validators["trial.schema.json"].iter_errors(eligible_without_accounting)):
        raise AssertionError(
            "comparison-eligible trial unexpectedly accepts unavailable accounting"
        )

    partially_executed_ineligible = copy.deepcopy(pre_execution_ineligible)
    partially_executed_ineligible["termination_status"] = "completed"
    if not list(v03_validators["trial.schema.json"].iter_errors(partially_executed_ineligible)):
        raise AssertionError("ineligible trial unexpectedly accepts a partial execution record")
    v03_structural, v03_semantic = validate_v03_cases(
        v03_validators, system_path, system, policy_path, policy
    )

    example_count = 12 + len(profiles) + trial_count

    print(
        f"Validated {len(schemas) + len(v02_schemas) + len(v03_schemas)} schemas, "
        f"{example_count} examples, {negative_count + v03_structural} structural "
        f"and {semantic_count + v03_semantic} semantic negative "
        "cases, dataset and release digests, "
        "and cross-manifest bindings."
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, KeyError, OSError, ValueError) as error:
        print(f"schema validation failed: {error}", file=sys.stderr)
        raise SystemExit(1)
