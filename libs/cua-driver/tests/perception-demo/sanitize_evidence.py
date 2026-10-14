#!/usr/bin/env python3
"""Build redacted demo evidence from Driver-measured installed state."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


SHA40 = re.compile(r"^[0-9a-f]{40}$")
SHA64 = re.compile(r"^[0-9a-f]{64}$")
SAFE_ID = re.compile(r"^[A-Za-z0-9._:/-]{1,96}$")
CHOOSER_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$")
SAFE_TEXT = re.compile(r"^[^\x00-\x1f\x7f]{1,256}$")
MAX_REGIONS = 64
MAX_REGION_TEXT = 128


def sha256_file(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def require_file(path: Path, label: str, maximum: int) -> int:
    if not path.is_file():
        raise ValueError(f"{label} must be a regular file")
    size = path.stat().st_size
    if not 0 < size <= maximum:
        raise ValueError(f"{label} must be nonempty and at most {maximum} bytes")
    return size


def load_json(path: Path, label: str, maximum: int = 1024 * 1024) -> dict:
    require_file(path, label, maximum)
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def measure_source_sha(repo_root: Path, expected_source_sha: str) -> str:
    if not SHA40.fullmatch(expected_source_sha):
        raise ValueError("expected source SHA must be 40 lowercase hexadecimal characters")
    result = subprocess.run(
        ["git", "-C", str(repo_root), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    measured = result.stdout.strip()
    if measured != expected_source_sha:
        raise ValueError("checked-out source does not match the approved workflow candidate")
    return measured


def measure_platform(platform_name: str = sys.platform, environment: dict = os.environ) -> str:
    if platform_name == "win32":
        return "windows"
    if platform_name == "darwin":
        return "macos"
    if platform_name.startswith("linux") and environment.get("DISPLAY"):
        return "linux-x11"
    raise ValueError("evidence sanitizer requires macOS, Windows, or an active Linux X11 session")


def probe_recording(path: Path) -> dict:
    result = subprocess.run(
        [
            "ffprobe", "-v", "error", "-select_streams", "v:0",
            "-show_entries", "stream=width,height,avg_frame_rate:format=duration",
            "-of", "json", str(path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    value = json.loads(result.stdout)
    streams = value.get("streams")
    if not isinstance(streams, list) or len(streams) != 1:
        raise ValueError("recording must contain exactly one measured video stream")
    stream = streams[0]
    try:
        numerator, denominator = (int(part) for part in stream["avg_frame_rate"].split("/", 1))
        width, height = int(stream["width"]), int(stream["height"])
        duration_ms = round(float(value["format"]["duration"]) * 1000)
    except (KeyError, TypeError, ValueError, ZeroDivisionError) as error:
        raise ValueError("recording metadata is incomplete") from error
    if min(width, height, numerator, denominator, duration_ms) <= 0:
        raise ValueError("recording metadata must be positive")
    return {
        "width": width,
        "height": height,
        "frame_rate": {"numerator": numerator, "denominator": denominator},
        "duration_ms": duration_ms,
    }


def load_extension_status(path: Path) -> dict:
    value = load_json(path, "Driver extension status")
    if (
        value.get("id") != "cua-perception"
        or value.get("installed") is not True
        or value.get("healthy") is not True
        or value.get("trust") != "review-only-publisher-verified"
    ):
        raise ValueError("Driver did not report a healthy review-only publisher-verified perception extension")
    for field in ("active_version", "publisher_id", "publisher_key_id"):
        if not isinstance(value.get(field), str) or not SAFE_ID.fullmatch(value[field]):
            raise ValueError(f"Driver extension status contains an invalid {field}")
    if type(value.get("catalog_version")) is not int or value["catalog_version"] <= 0:
        raise ValueError("Driver extension status contains an invalid catalog_version")
    if type(value.get("protocol_version")) is not int or value["protocol_version"] <= 0:
        raise ValueError("Driver extension status contains an invalid protocol_version")
    return value


def load_oracle(path: Path) -> dict:
    value = load_json(path, "fixture oracle", 64 * 1024)
    expected = {"fixture", "ready", "selected", "action_count"}
    if set(value) != expected:
        raise ValueError(f"fixture oracle keys must be exactly {sorted(expected)}")
    if value["fixture"] != "visual-only-canvas/v1" or value["ready"] is not True:
        raise ValueError("fixture oracle is not a ready visual-only canvas")
    if value["selected"] not in {"save", "send", "cancel"}:
        raise ValueError("fixture oracle contains an unexpected selection")
    if type(value["action_count"]) is not int or not 1 <= value["action_count"] <= 8:
        raise ValueError("fixture oracle action_count must be an integer from 1 through 8")
    return value


def load_parser_result(path: Path) -> dict:
    value = load_json(path, "parser result")
    model_id = value.get("parser", {}).get("model_id")
    if value.get("schema") != "cua.visual_regions_v1" or not isinstance(model_id, str) or not SAFE_ID.fullmatch(model_id):
        raise ValueError("parser result does not contain safe measured model metadata")
    return value


def load_chooser_result(path: Path) -> dict:
    value = load_json(path, "chooser result", 64 * 1024)
    expected = {"schema", "selected_id", "model", "confidence", "probabilities"}
    if set(value) != expected or value["schema"] != "cua.jev_choice_v1":
        raise ValueError("chooser result does not match cua.jev_choice_v1")
    model = value.get("model")
    if model is not None and (not isinstance(model, str) or not SAFE_ID.fullmatch(model)):
        raise ValueError("chooser result contains invalid model metadata")
    if not isinstance(value["selected_id"], str) or not CHOOSER_ID.fullmatch(value["selected_id"]):
        raise ValueError("chooser result contains an unsafe selected_id")
    confidence = value["confidence"]
    probabilities = value["probabilities"]
    if type(confidence) not in (int, float) or not 0 <= confidence <= 1:
        raise ValueError("chooser result contains an invalid confidence")
    if not isinstance(probabilities, dict):
        raise ValueError("chooser probabilities must be an object")
    if any(not isinstance(key, str) or not CHOOSER_ID.fullmatch(key) for key in probabilities):
        raise ValueError("chooser probabilities contain an unsafe candidate ID")
    if any(type(item) not in (int, float) or not 0 <= item <= 1 for item in probabilities.values()):
        raise ValueError("chooser result contains an invalid probability")
    return value


def validate_compact_regions(value: object, width: int, height: int, label: str) -> list[dict]:
    if not isinstance(value, list) or not 1 <= len(value) <= MAX_REGIONS:
        raise ValueError(f"{label} must be a nonempty bounded region list")
    regions = []
    for region in value:
        if not isinstance(region, dict):
            raise ValueError(f"{label} contains an invalid compact region")
        required = {"id", "kind", "bounds", "confidence", "interactive"}
        optional = {"text", "label"}
        if not required <= set(region) or not set(region) <= required | optional:
            raise ValueError(f"{label} contains an invalid compact region")
        region_id = region["id"]
        kind = region["kind"]
        if (
            not isinstance(region_id, str)
            or not CHOOSER_ID.fullmatch(region_id)
            or len(region_id) > 64 - len("region:")
            or kind not in {"text", "icon"}
        ):
            raise ValueError(f"{label} contains unsafe region identity")
        for field in optional & set(region):
            text = region[field]
            if (
                not isinstance(text, str)
                or text != text.strip()
                or len(text.encode("utf-8")) > MAX_REGION_TEXT
                or not SAFE_TEXT.fullmatch(text)
            ):
                raise ValueError(f"{label} contains unsafe region text or label")
        if (kind == "text" and "text" not in region) or (kind == "icon" and "label" not in region):
            raise ValueError(f"{label} contains content missing for its region kind")
        confidence = region["confidence"]
        if (
            type(confidence) not in (int, float)
            or not math.isfinite(confidence)
            or not 0 <= confidence <= 1
            or type(region["interactive"]) is not bool
        ):
            raise ValueError(f"{label} contains invalid confidence or interactivity")
        bounds = region["bounds"]
        if not isinstance(bounds, dict) or set(bounds) != {"x", "y", "width", "height"}:
            raise ValueError(f"{label} contains invalid region bounds")
        if (
            any(type(bounds[field]) is not int for field in bounds)
            or bounds["x"] < 0
            or bounds["y"] < 0
            or bounds["width"] <= 0
            or bounds["height"] <= 0
            or bounds["x"] + bounds["width"] > width
            or bounds["y"] + bounds["height"] > height
        ):
            raise ValueError(f"{label} contains region bounds outside the observation")
        regions.append(region)
    if len({region["id"] for region in regions}) != len(regions):
        raise ValueError(f"{label} contains duplicate region IDs")
    return regions


def load_candidate_measurements(path: Path) -> dict:
    value = load_json(path, "candidate measurements", 64 * 1024)
    expected = {
        "review_only",
        "source_sha",
        "target",
        "supplied_model_asset_id",
        "supplied_model_sha256",
        "supplied_model_size",
        "public_key_base64",
        "public_key_sha256",
        "key_id",
        "publisher_id",
        "signature_algorithm",
        "catalog_sha256",
        "archive_sha256",
        "review_driver_relative_path",
        "review_driver_build_profile",
        "review_driver_sha256",
        "code_signing",
        "review_driver_version",
        "extension_version",
        "protocol_version",
        "worker_sha256",
        "models",
        "onnx_runtime",
        "self_test",
        "sealed_artifact_manifest_sha256",
        "sealed_extension_manifest_sha256",
    }
    if (
        set(value) != expected
        or value["review_only"] is not True
        or value["signature_algorithm"] != "ed25519"
        or value["key_id"] != "review-only-build-override"
        or value["publisher_id"] != "cua-review-only"
        or value["review_driver_build_profile"] != "debug-review-trust-root"
    ):
        raise ValueError("candidate measurements do not match the signed artifact contract")
    for field in (
        "supplied_model_sha256",
        "public_key_sha256",
        "catalog_sha256",
        "archive_sha256",
        "review_driver_sha256",
        "worker_sha256",
        "sealed_artifact_manifest_sha256",
        "sealed_extension_manifest_sha256",
    ):
        if not isinstance(value[field], str) or not SHA64.fullmatch(value[field]):
            raise ValueError(f"candidate measurements contain an invalid {field}")
    if not isinstance(value["source_sha"], str) or not SHA40.fullmatch(value["source_sha"]):
        raise ValueError("candidate measurements contain an invalid source_sha")
    for field in ("target", "review_driver_relative_path"):
        if not isinstance(value[field], str) or not SAFE_ID.fullmatch(value[field]):
            raise ValueError(f"candidate measurements contain an invalid {field}")
    for field in ("review_driver_version", "extension_version"):
        if not isinstance(value[field], str) or not SAFE_ID.fullmatch(value[field]):
            raise ValueError(f"candidate measurements contain an invalid {field}")
    if type(value["protocol_version"]) is not int or value["protocol_version"] <= 0:
        raise ValueError("candidate measurements contain an invalid protocol_version")
    code_signing = value["code_signing"]
    if not isinstance(code_signing, dict) or set(code_signing) != {
        "status", "format", "identity", "certificate_sha256", "designated_requirement"
    }:
        raise ValueError("candidate measurements contain invalid code-signing evidence")
    if value["target"] == "aarch64-apple-darwin":
        if (
            code_signing.get("status") != "verified"
            or code_signing.get("format") != "apple-codesign"
            or code_signing.get("identity") != "ephemeral-self-signed-review-only"
            or not isinstance(code_signing.get("certificate_sha256"), str)
            or not SHA64.fullmatch(code_signing["certificate_sha256"])
            or not isinstance(code_signing.get("designated_requirement"), str)
            or not 1 <= len(code_signing["designated_requirement"]) <= 4096
            or not any(marker in code_signing["designated_requirement"] for marker in ("certificate leaf", "certificate root"))
        ):
            raise ValueError("macOS candidate lacks verified certificate-backed review signing")
    elif value["target"] in {"x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"}:
        if code_signing != {
            "status": "not-applicable",
            "format": "none",
            "identity": "none",
            "certificate_sha256": None,
            "designated_requirement": None,
        }:
            raise ValueError("non-macOS candidate has misleading code-signing evidence")
    else:
        raise ValueError("candidate measurements contain an unsupported target")
    self_test = value["self_test"]
    if (
        not isinstance(self_test, dict)
        or set(self_test) != {"status", "mismatch_rejection", "evidence_sha256"}
        or self_test.get("status") != "passed"
        or self_test.get("mismatch_rejection") is not True
        or not isinstance(self_test.get("evidence_sha256"), str)
        or not SHA64.fullmatch(self_test["evidence_sha256"])
    ):
        raise ValueError("candidate measurements contain invalid self-test evidence")
    runtime = value["onnx_runtime"]
    if set(runtime) != {"revision", "sha256", "license"} or not SAFE_ID.fullmatch(runtime.get("revision", "")) or not SHA64.fullmatch(runtime.get("sha256", "")) or not SAFE_ID.fullmatch(runtime.get("license", "")):
        raise ValueError("candidate measurements contain invalid ONNX Runtime evidence")
    models = value["models"]
    if (
        not isinstance(models, list)
        or any(not isinstance(item, dict) for item in models)
        or [item.get("role") for item in models]
        != ["icon-detect", "ocr-detect", "ocr-recognize"]
    ):
        raise ValueError("candidate measurements must contain all three ordered model identities")
    for model in models:
        if set(model) != {"role", "id", "revision", "original_sha256", "converted_sha256", "license"}:
            raise ValueError("candidate measurements contain an invalid model record")
        if any(not isinstance(model[field], str) or not SAFE_ID.fullmatch(model[field]) for field in ("role", "id", "revision", "license")):
            raise ValueError("candidate measurements contain unsafe model metadata")
        if any(not SHA64.fullmatch(model[field]) for field in ("original_sha256", "converted_sha256")):
            raise ValueError("candidate measurements contain invalid model hashes")
    for field in ("supplied_model_asset_id", "supplied_model_size"):
        if type(value[field]) is not int or value[field] <= 0:
            raise ValueError(f"candidate measurements contain an invalid {field}")
    try:
        public_key = base64.b64decode(value["public_key_base64"], validate=True)
    except (ValueError, TypeError) as error:
        raise ValueError("candidate measurements contain invalid public_key_base64") from error
    if len(public_key) != 32 or hashlib.sha256(public_key).hexdigest() != value["public_key_sha256"]:
        raise ValueError("candidate measurements contain a mismatched Ed25519 public key")
    return value


def build_manifest(
    *,
    source_sha: str,
    platform: str,
    jev_source_sha: str,
    session_label: str,
    os_name: str,
    os_version: str,
    os_arch: str,
    chooser_mode: str,
    candidate_measurements: Path,
    driver_binary: Path,
    extension_status: Path,
    parser_result: Path,
    chooser_result: Path,
    model: Path,
    oracle: Path,
    raw_evidence: Path,
    recording: Path,
) -> dict:
    if not SHA40.fullmatch(source_sha):
        raise ValueError("source_sha must be 40 lowercase hexadecimal characters")
    if platform not in {"windows", "linux-x11", "macos"}:
        raise ValueError("platform must be windows, linux-x11, or macos")
    if not SHA40.fullmatch(jev_source_sha):
        raise ValueError("jev_source_sha must be 40 lowercase hexadecimal characters")
    for label, value in (
        ("session_label", session_label),
        ("os_name", os_name),
        ("os_version", os_version),
        ("os_arch", os_arch),
    ):
        if not SAFE_ID.fullmatch(value):
            raise ValueError(f"{label} must be a safe bounded identifier")
    if chooser_mode not in {"mock", "live"}:
        raise ValueError("chooser_mode must be mock or live")
    status = load_extension_status(extension_status)
    parser = load_parser_result(parser_result)
    chooser = load_chooser_result(chooser_result)
    oracle_value = load_oracle(oracle)
    measurements = load_candidate_measurements(candidate_measurements)
    if measurements["source_sha"] != source_sha:
        raise ValueError("candidate measurements do not match source_sha")
    expected_target = {
        "windows": "x86_64-pc-windows-msvc",
        "linux-x11": "x86_64-unknown-linux-gnu",
        "macos": "aarch64-apple-darwin",
    }[platform]
    if measurements["target"] != expected_target:
        raise ValueError("candidate measurements do not match the measured platform")
    if (
        status["publisher_id"] != measurements["publisher_id"]
        or status["publisher_key_id"] != measurements["key_id"]
    ):
        raise ValueError("Driver status does not match candidate publisher identity")
    if status["active_version"] != measurements["extension_version"] or status.get("protocol_version") != measurements["protocol_version"]:
        raise ValueError("Driver status does not match sealed extension version or protocol")
    if chooser_mode == "live" and chooser["model"] is None:
        raise ValueError("live chooser did not return its model identity")
    raw = load_json(raw_evidence, "raw evidence", 8 * 1024 * 1024)
    expected_raw_keys = {
        "schema", "source_sha", "jev_source_sha", "platform", "capture_ids",
        "observation", "fixture_oracle", "extension_status", "parser", "chooser",
        "host_validated_regions", "resolved_action", "verification", "timeline", "recording",
    }
    if set(raw) != expected_raw_keys or raw.get("schema") != "cua-visual-perception-demo-raw/v2":
        raise ValueError("raw evidence does not match the closed live demo contract")
    if (
        raw["source_sha"] != source_sha
        or raw["jev_source_sha"] != jev_source_sha
        or raw["platform"] != platform
    ):
        raise ValueError("raw evidence does not match the approved sources and measured platform")
    if raw["fixture_oracle"] != oracle_value:
        raise ValueError("raw evidence fixture oracle differs from the measured fixture result")
    if raw["extension_status"] != status:
        raise ValueError("raw evidence extension status differs from Driver measured state")
    if raw["parser"] != parser["parser"]:
        raise ValueError("raw evidence parser identity differs from the measured parser result")
    chooser_record = raw.get("chooser")
    if (
        not isinstance(chooser_record, dict)
        or set(chooser_record) != {"mode", "request", "response"}
        or chooser_record["mode"] != chooser_mode
        or chooser_record["response"] != chooser
    ):
        raise ValueError("raw evidence chooser result differs from the measured chooser result")
    is_desktop = raw.get("observation", {}).get("input_scope") == "desktop"
    expected_route_proof = True if is_desktop else None
    expected_verification = {
        "oracle": "passed",
        "background_desktop_refused": expected_route_proof,
        "capture_preserved_after_refusal": expected_route_proof,
        "stale_capture_refused": True,
    }
    if raw.get("verification") != expected_verification:
        raise ValueError(
            "raw evidence does not prove the fixture result, desktop/background refusal, "
            "capture preservation, and stale-capture refusal"
        )
    background_event = (
        "background_desktop_refused" if is_desktop else "background_refusal_not_applicable"
    )
    timeline = raw.get("timeline")
    if (
        not isinstance(timeline, dict)
        or set(timeline) != {"duration_ms", "events"}
        or type(timeline["duration_ms"]) is not int
        or timeline["duration_ms"] <= 0
        or timeline["events"] != [
            "observed", "parsed", "chosen", background_event, "clicked", "oracle_verified",
            "stale_capture_refused", "reobserved",
        ]
    ):
        raise ValueError("raw evidence timeline does not prove the complete live demo sequence")
    captures = raw.get("capture_ids")
    if (
        not isinstance(captures, dict)
        or set(captures) != {"acted", "fresh"}
        or any(not isinstance(value, str) or not SAFE_ID.fullmatch(value) for value in captures.values())
    ):
        raise ValueError("raw evidence must contain exact safe acted and fresh capture IDs")
    require_file(driver_binary, "Driver binary", 512 * 1024 * 1024)
    driver_binary_sha256 = sha256_file(driver_binary)
    if driver_binary_sha256 != measurements["review_driver_sha256"]:
        raise ValueError("Driver binary hash does not match candidate measurements")
    require_file(model, "model", 8 * 1024 * 1024 * 1024)
    model_sha256 = sha256_file(model)
    if model_sha256 != measurements["supplied_model_sha256"]:
        raise ValueError("model hash is absent from candidate measurements")
    if model_sha256 != measurements["models"][0]["converted_sha256"]:
        raise ValueError("model hash differs from the sealed OmniParser identity")
    require_file(raw_evidence, "raw evidence", 8 * 1024 * 1024)
    video_size = require_file(recording, "recording.mp4", 100 * 1024 * 1024)
    recording_probe = probe_recording(recording)
    observation = raw.get("observation")
    if not isinstance(observation, dict) or set(observation) != {
        "input_scope", "capture_kind", "capture_source", "width", "height",
        "desktop_session", "runner_identity_class", "delivery_mode",
    }:
        raise ValueError("raw evidence must contain exact measured observation context")
    for field in ("input_scope", "capture_kind", "capture_source", "desktop_session", "runner_identity_class", "delivery_mode"):
        if not isinstance(observation[field], str) or not SAFE_ID.fullmatch(observation[field]):
            raise ValueError(f"raw evidence contains invalid observation {field}")
    accepted_routes = {
        ("window", "get_window_state", "background"),
        ("desktop", "get_desktop_state", "foreground"),
    }
    route = (
        observation["input_scope"],
        observation["capture_kind"],
        observation["delivery_mode"],
    )
    if route not in accepted_routes or observation["capture_source"] != "driver-screenshot":
        raise ValueError("raw evidence observation context differs from the executed demo")
    if any(type(observation[field]) is not int or observation[field] <= 0 for field in ("width", "height")):
        raise ValueError("raw evidence contains invalid capture dimensions")
    request = chooser_record["request"]
    if not isinstance(request, dict):
        raise ValueError("raw evidence chooser request must be an object")
    host_regions = validate_compact_regions(
        raw.get("host_validated_regions"), observation["width"], observation["height"],
        "raw evidence host_validated_regions",
    )
    request_regions = validate_compact_regions(
        request.get("regions"), observation["width"], observation["height"],
        "raw evidence chooser request regions",
    )
    host_regions_by_id = {region["id"]: region for region in host_regions}
    if any(host_regions_by_id.get(region["id"]) != region for region in request_regions):
        raise ValueError("chooser request regions are not a subset of host_validated_regions")
    candidate_values = request.get("candidates")
    if not isinstance(candidate_values, list) or not 2 <= len(candidate_values) <= 18:
        raise ValueError("raw evidence must contain the bounded chooser candidates")
    candidates = []
    for candidate in candidate_values:
        if (
            not isinstance(candidate, dict)
            or set(candidate) != {"id", "description"}
            or not CHOOSER_ID.fullmatch(candidate.get("id", ""))
            or not SAFE_TEXT.fullmatch(candidate.get("description", ""))
        ):
            raise ValueError("raw evidence contains an unsafe candidate")
        candidates.append(candidate)
    if len({candidate["id"] for candidate in candidates}) != len(candidates):
        raise ValueError("raw evidence contains duplicate candidate IDs")
    executable_region_ids = {
        candidate["id"][len("region:"):]
        for candidate in candidates
        if candidate["id"].startswith("region:")
    }
    if {region["id"] for region in request_regions} != executable_region_ids:
        raise ValueError("chooser request regions do not match executable candidate-linked regions")
    if chooser["selected_id"] not in {candidate["id"] for candidate in candidates}:
        raise ValueError("selected candidate was not in the bounded candidate set")
    selected_candidate_id = chooser["selected_id"]
    if not selected_candidate_id.startswith("region:"):
        raise ValueError("selected candidate does not resolve to an executable region action")
    selected_region = {
        region["id"]: region for region in request_regions
    }.get(selected_candidate_id[len("region:"):])
    if selected_region is None:
        raise ValueError("selected executable candidate has no exact chooser request region")
    resolved_action = raw.get("resolved_action")
    if (
        not isinstance(resolved_action, dict)
        or set(resolved_action) != {"candidate_id", "x", "y"}
        or resolved_action["candidate_id"] != selected_candidate_id
        or any(type(resolved_action[field]) not in (int, float) for field in ("x", "y"))
    ):
        raise ValueError("raw evidence resolved action differs from the selected candidate")
    action_x = resolved_action["x"]
    action_y = resolved_action["y"]
    if (
        not math.isfinite(action_x)
        or not math.isfinite(action_y)
        or not 0 <= action_x < observation["width"]
        or not 0 <= action_y < observation["height"]
    ):
        raise ValueError("raw evidence resolved action coordinates are outside the observation")
    bounds = selected_region["bounds"]
    expected_x = float(bounds["x"]) + float(bounds["width"]) / 2.0
    expected_y = float(bounds["y"]) + float(bounds["height"]) / 2.0
    if (
        not math.isclose(action_x, expected_x, rel_tol=0.0, abs_tol=math.ulp(expected_x))
        or not math.isclose(action_y, expected_y, rel_tol=0.0, abs_tol=math.ulp(expected_y))
    ):
        raise ValueError("raw evidence resolved action does not match the selected region center")
    recording_sha256 = sha256_file(recording)
    raw_recording = raw.get("recording")
    if (
        not isinstance(raw_recording, dict)
        or set(raw_recording) != {"local_path", "sha256", "metadata"}
        or not isinstance(raw_recording["local_path"], str)
        or raw_recording["sha256"] != recording_sha256
        or raw_recording["metadata"] != recording_probe
    ):
        raise ValueError("raw evidence recording differs from the decoded recording")
    capture_trace_sha256 = hashlib.sha256(
        f"{captures['acted']}\0{captures['fresh']}".encode()
    ).hexdigest()
    return {
        "schema": "cua-visual-perception-demo-evidence/v3",
        "platform": platform,
        "raw_evidence_sha256": sha256_file(raw_evidence),
        "fixture": {
            "id": oracle_value["fixture"],
            "oracle": {
                "source": "fixture-journal",
                "initial_ready": True,
                "selected": oracle_value["selected"],
                "action_count": oracle_value["action_count"],
                "result": "passed",
            },
        },
        "runtime": {
            "driver": {
                "source_sha": source_sha,
                "version": measurements["review_driver_version"],
                "binary_sha256": driver_binary_sha256,
                "build": {
                    "profile": measurements["review_driver_build_profile"],
                    "target": measurements["target"],
                    "sealed_artifact_manifest_sha256": measurements["sealed_artifact_manifest_sha256"],
                    "sealed_extension_manifest_sha256": measurements["sealed_extension_manifest_sha256"],
                },
            },
            "perception": {
                "extension_id": status["id"],
                "extension_version": status["active_version"],
                "trust": status["trust"],
                "publisher_id": status["publisher_id"],
                "publisher_key_id": status["publisher_key_id"],
                "catalog_version": status["catalog_version"],
                "signature_algorithm": measurements["signature_algorithm"],
                "signed_extension_archive_sha256": measurements["archive_sha256"],
                "signing_key_sha256": measurements["public_key_sha256"],
                "signed_catalog_sha256": measurements["catalog_sha256"],
                "protocol_version": measurements["protocol_version"],
                "worker_sha256": measurements["worker_sha256"],
                "self_test": measurements["self_test"],
                "models": measurements["models"],
                "onnx_runtime": measurements["onnx_runtime"],
                "parser_model_id": parser["parser"]["model_id"],
            },
            "chooser": {
                "mode": chooser_mode,
                "provider": "typesafe" if chooser_mode == "live" else "fixture",
                "model_id": chooser["model"],
                "adapter_source_sha": source_sha,
                "source_sha": jev_source_sha,
            },
        },
        "observation": {
            "session_label": session_label,
            "input_scope": observation["input_scope"],
            "capture_kind": observation["capture_kind"],
            "capture_source": observation["capture_source"],
            "dimensions": {"width": observation["width"], "height": observation["height"]},
            "acted_capture_id_sha256": hashlib.sha256(captures["acted"].encode()).hexdigest(),
            "fresh_capture_id_sha256": hashlib.sha256(captures["fresh"].encode()).hexdigest(),
            "capture_trace_sha256": capture_trace_sha256,
            "candidates": candidates,
        },
        "environment": {
            "os": {"name": os_name, "version": os_version, "arch": os_arch},
            "desktop_session": observation["desktop_session"],
            "runner_identity_class": observation["runner_identity_class"],
            "delivery_mode": observation["delivery_mode"],
        },
        "result": {
            "status": "passed",
            "selected_candidate": chooser["selected_id"],
            "background_desktop_refused": raw["verification"]["background_desktop_refused"],
            "capture_preserved_after_refusal": raw["verification"]["capture_preserved_after_refusal"],
            "stale_capture_refused": True,
        },
        "recording": {
            "original_dimensions": {"width": recording_probe["width"], "height": recording_probe["height"]},
            "delivered_dimensions": {"width": recording_probe["width"], "height": recording_probe["height"]},
            "frame_rate": recording_probe["frame_rate"],
            "cursor": {"agent_overlay": False, "system_cursor": "recorder-default"},
            "edit_operations": [{"operation": "none", "speed": "1x"}],
            "shots": [{"source_sha256": recording_sha256, "start_ms": 0, "end_ms": recording_probe["duration_ms"]}],
            "final_sha256": recording_sha256,
            "size_bytes": video_size,
        },
        "artifacts": [{
            "kind": "video",
            "path": "recording.mp4",
            "sha256": recording_sha256,
            "size_bytes": video_size,
        }],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--expected-source-sha", required=True)
    parser.add_argument("--chooser-mode", choices=("mock", "live"), required=True)
    parser.add_argument("--jev-source-sha", required=True)
    parser.add_argument("--session-label", required=True)
    parser.add_argument("--os-name", required=True)
    parser.add_argument("--os-version", required=True)
    parser.add_argument("--os-arch", required=True)
    parser.add_argument("--candidate-measurements", type=Path, required=True)
    parser.add_argument("--driver-binary", type=Path, required=True)
    parser.add_argument("--extension-status", type=Path, required=True)
    parser.add_argument("--parser-result", type=Path, required=True)
    parser.add_argument("--chooser-result", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--oracle", type=Path, required=True)
    parser.add_argument("--raw-evidence", type=Path, required=True)
    parser.add_argument("--recording", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    manifest = build_manifest(
        source_sha=measure_source_sha(args.repo_root, args.expected_source_sha),
        platform=measure_platform(),
        jev_source_sha=args.jev_source_sha,
        session_label=args.session_label,
        os_name=args.os_name,
        os_version=args.os_version,
        os_arch=args.os_arch,
        chooser_mode=args.chooser_mode,
        candidate_measurements=args.candidate_measurements,
        driver_binary=args.driver_binary,
        extension_status=args.extension_status,
        parser_result=args.parser_result,
        chooser_result=args.chooser_result,
        model=args.model,
        oracle=args.oracle,
        raw_evidence=args.raw_evidence,
        recording=args.recording,
    )
    if args.output_dir.exists() and any(args.output_dir.iterdir()):
        raise ValueError("output directory must be empty before evidence publication")
    args.output_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    shutil.copyfile(args.recording, args.output_dir / "recording.mp4")
    (args.output_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
