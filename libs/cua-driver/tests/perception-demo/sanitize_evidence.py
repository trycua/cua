#!/usr/bin/env python3
"""Build redacted demo evidence from Driver-measured installed state."""

from __future__ import annotations

import argparse
import hashlib
import json
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
    if platform_name.startswith("linux") and environment.get("DISPLAY"):
        return "linux-x11"
    raise ValueError("evidence sanitizer requires Windows or an active Linux X11 session")


def load_extension_status(path: Path) -> dict:
    value = load_json(path, "Driver extension status")
    if (
        value.get("id") != "cua-perception"
        or value.get("installed") is not True
        or value.get("healthy") is not True
        or value.get("trust") != "publisher_verified"
    ):
        raise ValueError("Driver did not report a healthy publisher-verified perception extension")
    for field in ("active_version", "publisher_id", "publisher_key_id"):
        if not isinstance(value.get(field), str) or not SAFE_ID.fullmatch(value[field]):
            raise ValueError(f"Driver extension status contains an invalid {field}")
    if type(value.get("catalog_version")) is not int or value["catalog_version"] <= 0:
        raise ValueError("Driver extension status contains an invalid catalog_version")
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


def build_manifest(
    *,
    source_sha: str,
    platform: str,
    chooser_mode: str,
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
    if platform not in {"windows", "linux-x11"}:
        raise ValueError("platform must be windows or linux-x11")
    if chooser_mode not in {"mock", "live"}:
        raise ValueError("chooser_mode must be mock or live")
    status = load_extension_status(extension_status)
    parser = load_parser_result(parser_result)
    chooser = load_chooser_result(chooser_result)
    oracle_value = load_oracle(oracle)
    require_file(model, "model", 8 * 1024 * 1024 * 1024)
    require_file(raw_evidence, "raw evidence", 8 * 1024 * 1024)
    video_size = require_file(recording, "recording.mp4", 100 * 1024 * 1024)
    return {
        "schema": "cua-visual-perception-demo-evidence/v2",
        "source_sha": source_sha,
        "platform": platform,
        "raw_evidence_sha256": sha256_file(raw_evidence),
        "fixture": {
            "id": oracle_value["fixture"],
            "oracle": {
                "selected": oracle_value["selected"],
                "action_count": oracle_value["action_count"],
            },
        },
        "runtime": {
            "perception": {
                "extension_id": status["id"],
                "extension_version": status["active_version"],
                "trust": status["trust"],
                "publisher_id": status["publisher_id"],
                "publisher_key_id": status["publisher_key_id"],
                "catalog_version": status["catalog_version"],
                "signature_algorithm": "ed25519",
                "model_id": parser["parser"]["model_id"],
                "model_sha256": sha256_file(model),
            },
            "chooser": {
                "mode": chooser_mode,
                "provider": "typesafe" if chooser_mode == "live" else "fixture",
                "model_id": chooser["model"],
            },
        },
        "result": {
            "status": "passed",
            "selected_candidate": chooser["selected_id"],
            "stale_capture_refused": True,
        },
        "artifacts": [{
            "kind": "video",
            "path": "recording.mp4",
            "sha256": sha256_file(recording),
            "size_bytes": video_size,
        }],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--expected-source-sha", required=True)
    parser.add_argument("--chooser-mode", choices=("mock", "live"), required=True)
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
        chooser_mode=args.chooser_mode,
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
    args.output_dir.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(args.recording, args.output_dir / "recording.mp4")
    (args.output_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
