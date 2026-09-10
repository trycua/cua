"""Trajectory-neutral verification of required computer-use participation."""

from __future__ import annotations

import re
import sys
from collections.abc import Mapping, Sequence
from typing import Any, Callable

from cua_bench_runtime.canon import digest_json
from cua_bench_runtime.errors import ValidationFailure
from cua_bench_runtime.model import ObserverReport

_DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
_KINDS = frozenset({"observe", "act", "readback"})
_CAPABILITY_CLASSES = frozenset(
    {
        "accessibility_observation",
        "visual_observation",
        "pointer_input",
        "keyboard_input",
        "window_control",
        "browser_control",
        "driver_skill",
        "composite",
    }
)
_TRUST = frozenset({"certifying", "non_certifying", "unavailable"})


def required_fact_digests(facts: Mapping[str, Any]) -> dict[str, str]:
    """Hash public task predicates into the privacy-preserving event form."""

    return {key: digest_json(value) for key, value in sorted(facts.items())}


def _nonempty(value: object, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValidationFailure(f"participation event {field} must be non-empty")
    return value


def normalize_event(raw: Mapping[str, Any]) -> dict[str, Any]:
    """Validate and copy an observer event without accepting screen content."""

    allowed = {
        "provider",
        "capability_class",
        "target",
        "kind",
        "correlation_id",
        "fact_digests",
    }
    extra = set(raw) - allowed
    if extra:
        raise ValidationFailure(
            "participation event contains unsupported fields: " + ", ".join(sorted(extra))
        )
    provider = raw.get("provider")
    target = raw.get("target")
    facts = raw.get("fact_digests", {})
    if not isinstance(provider, Mapping):
        raise ValidationFailure("participation event provider must be an object")
    if set(provider) - {"id", "version"}:
        raise ValidationFailure("participation event provider has unsupported fields")
    if not isinstance(target, Mapping):
        raise ValidationFailure("participation event target must be an object")
    if set(target) - {
        "application_id",
        "surface_id",
        "platform_application_id",
        "process_id",
        "window_id",
    }:
        raise ValidationFailure("participation event target has unsupported fields")
    if not isinstance(facts, Mapping):
        raise ValidationFailure("participation event fact_digests must be an object")
    capability = _nonempty(raw.get("capability_class"), "capability_class")
    if capability not in _CAPABILITY_CLASSES:
        raise ValidationFailure(f"unsupported participation capability class: {capability}")
    kind = _nonempty(raw.get("kind"), "kind")
    if kind not in _KINDS:
        raise ValidationFailure(f"unsupported participation event kind: {kind}")
    normalized_target: dict[str, Any] = {
        "application_id": _nonempty(target.get("application_id"), "target.application_id"),
        "surface_id": _nonempty(target.get("surface_id"), "target.surface_id"),
        "platform_application_id": _nonempty(
            target.get("platform_application_id"),
            "target.platform_application_id",
        ),
        "window_id": _nonempty(target.get("window_id"), "target.window_id"),
    }
    process_id = target.get("process_id")
    if not isinstance(process_id, int) or isinstance(process_id, bool) or process_id <= 0:
        raise ValidationFailure("participation event target.process_id must be a positive integer")
    normalized_target["process_id"] = process_id
    normalized_facts: dict[str, str] = {}
    for key, value in sorted(facts.items()):
        _nonempty(key, "fact_digests key")
        if not isinstance(value, str) or not _DIGEST.fullmatch(value):
            raise ValidationFailure(
                f"participation event fact digest for {key} must be sha256:<hex>"
            )
        normalized_facts[key] = value
    normalized_provider = {"id": _nonempty(provider.get("id"), "provider.id")}
    if "version" in provider:
        normalized_provider["version"] = _nonempty(provider["version"], "provider.version")
    return {
        "provider": normalized_provider,
        "capability_class": capability,
        "target": normalized_target,
        "kind": kind,
        "correlation_id": _nonempty(raw.get("correlation_id"), "correlation_id"),
        "fact_digests": normalized_facts,
    }


def _matches_step(event: Mapping[str, Any], step: Mapping[str, Any]) -> bool:
    if event["kind"] != step["kind"]:
        return False
    required = required_fact_digests(step.get("required_facts", {}))
    actual = event["fact_digests"]
    return all(actual.get(key) == value for key, value in required.items())


def _match_requirement(
    requirement: Mapping[str, Any],
    events: Sequence[tuple[dict[str, Any], str]],
    platform: str | None = None,
) -> dict[str, Any]:
    target = requirement["target"]
    if platform is None:
        platform = (
            "macos"
            if sys.platform == "darwin"
            else "windows"
            if sys.platform == "win32"
            else "linux"
        )
    platform_application_ids = target.get("platform_application_ids", {})
    accepted_platform_ids = platform_application_ids.get(platform)
    if platform_application_ids and not accepted_platform_ids:
        return {
            "requirement_id": requirement["id"],
            "status": "unsatisfied",
            "evidence_event_hashes": [],
            "reason": f"the task declares no application identity for {platform}",
        }
    candidates = [
        (event, event_hash)
        for event, event_hash in events
        if event["target"]["application_id"] == target["application_id"]
        and event["target"]["surface_id"] == target["surface_id"]
        and (
            not platform_application_ids
            or event["target"]["platform_application_id"] in accepted_platform_ids
        )
    ]
    steps = requirement["sequence"]
    for start, (first, first_hash) in enumerate(candidates):
        if not _matches_step(first, steps[0]):
            continue
        correlation = first["correlation_id"]
        hashes = [first_hash]
        position = start + 1
        for step in steps[1:]:
            found = False
            while position < len(candidates):
                event, event_hash = candidates[position]
                position += 1
                if event["correlation_id"] != correlation:
                    continue
                if _matches_step(event, step):
                    hashes.append(event_hash)
                    found = True
                    break
            if not found:
                break
        else:
            return {
                "requirement_id": requirement["id"],
                "status": "satisfied",
                "evidence_event_hashes": hashes,
            }
    return {
        "requirement_id": requirement["id"],
        "status": "unsatisfied",
        "evidence_event_hashes": [],
        "reason": "no fresh ordered event sequence matched the declared target and facts",
    }


def evaluate_participation(
    requirements: Sequence[Mapping[str, Any]],
    report: ObserverReport,
    append_event: Callable[[str, dict[str, Any]], str],
    *,
    bindings: Mapping[str, str],
    platform: str | None = None,
) -> dict[str, Any]:
    """Persist normalized evidence and return a separate, non-scoring receipt."""

    if report.trust not in _TRUST:
        raise ValidationFailure(f"unsupported observer trust level: {report.trust}")
    if not requirements:
        body = {
            "required": False,
            "status": "not_required",
            "passed": None,
            "observer": {"name": report.name, "trust": report.trust},
            "requirements": [],
            "bindings": dict(bindings),
        }
        return {**body, "receipt_digest": digest_json(body)}
    if report.trust == "unavailable":
        rows = [
            {
                "requirement_id": requirement["id"],
                "status": "unavailable",
                "evidence_event_hashes": [],
                "reason": report.detail or "no benchmark-owned driver observer was available",
            }
            for requirement in requirements
        ]
        body = {
            "required": True,
            "status": "unavailable",
            "passed": None,
            "observer": {"name": report.name, "trust": report.trust},
            "requirements": rows,
            "bindings": dict(bindings),
        }
        return {**body, "receipt_digest": digest_json(body)}

    normalized: list[tuple[dict[str, Any], str]] = []
    for raw in report.events:
        event = normalize_event(raw)
        event_hash = append_event("driver_participation_event", event)
        normalized.append((event, event_hash))
    rows = [_match_requirement(requirement, normalized, platform) for requirement in requirements]
    passed = all(row["status"] == "satisfied" for row in rows)
    body = {
        "required": True,
        "status": "passed" if passed else "failed",
        "passed": passed,
        "observer": {"name": report.name, "trust": report.trust},
        "requirements": rows,
        "bindings": dict(bindings),
    }
    return {**body, "receipt_digest": digest_json(body)}
