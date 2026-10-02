#!/usr/bin/env python3
"""Validate and publish a privacy-bounded reel derived from raw demo evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess


SHA40 = re.compile(r"^[0-9a-f]{40}$")
SHA64 = re.compile(r"^[0-9a-f]{64}$")
SAFE_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$")
WAIT_DISCLOSURE = "wait-only interval removed; no action or result omitted"
MAX_VIDEO_SIZE = 100 * 1024 * 1024


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def load_json(path: Path, label: str, maximum: int = 1024 * 1024) -> dict:
    if not path.is_file() or not 0 < path.stat().st_size <= maximum:
        raise ValueError(f"{label} must be a nonempty regular file no larger than {maximum} bytes")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def resolve_input(root: Path, relative_text: str, label: str) -> Path:
    if not isinstance(relative_text, str):
        raise ValueError(f"{label} path must be a string")
    relative = PurePosixPath(relative_text)
    if relative.is_absolute() or not relative.parts or ".." in relative.parts:
        raise ValueError(f"{label} path must be a safe relative path")
    resolved_root = root.resolve()
    resolved = root.joinpath(*relative.parts).resolve()
    try:
        resolved.relative_to(resolved_root)
    except ValueError as error:
        raise ValueError(f"{label} path escapes the source root") from error
    if not resolved.is_file():
        raise ValueError(f"{label} must be a regular file")
    return resolved


def probe_video(path: Path, *, decode: bool) -> dict:
    if not path.is_file() or not 0 < path.stat().st_size <= MAX_VIDEO_SIZE:
        raise ValueError("video must be a nonempty regular file no larger than 100 MiB")
    if decode:
        subprocess.run(
            ["ffmpeg", "-v", "error", "-xerror", "-i", str(path), "-map", "0:v:0", "-f", "null", "-"],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )
    result = subprocess.run(
        ["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=avg_frame_rate:format=duration", "-of", "json", str(path)],
        check=True,
        capture_output=True,
        text=True,
    )
    value = json.loads(result.stdout)
    streams = value.get("streams")
    if not isinstance(streams, list) or len(streams) != 1:
        raise ValueError("video must contain exactly one video stream")
    try:
        numerator, denominator = map(int, streams[0]["avg_frame_rate"].split("/", 1))
        duration_ms = round(float(value["format"]["duration"]) * 1000)
    except (KeyError, TypeError, ValueError, ZeroDivisionError) as error:
        raise ValueError("video metadata is incomplete") from error
    if min(numerator, denominator, duration_ms) <= 0:
        raise ValueError("video metadata must be positive")
    return {"duration_ms": duration_ms, "frame_duration_ms": 1000 * denominator / numerator}


def validate_source_manifest(manifest: dict, recording: Path) -> tuple[str, str]:
    try:
        artifact = manifest["artifacts"]
        recording_value = manifest["recording"]
        driver = manifest["runtime"]["driver"]
        perception = manifest["runtime"]["perception"]
        chooser = manifest["runtime"]["chooser"]
    except (KeyError, TypeError) as error:
        raise ValueError("source evidence manifest is incomplete") from error
    digest = sha256_file(recording)
    size = recording.stat().st_size
    if (
        manifest.get("schema") != "cua-visual-perception-demo-evidence/v3"
        or manifest.get("platform") not in {"windows", "linux-x11", "macos"}
        or manifest.get("fixture", {}).get("id") != "visual-only-canvas/v1"
        or manifest.get("result", {}).get("status") != "passed"
        or manifest.get("result", {}).get("stale_capture_refused") is not True
        or perception.get("trust") != "review-only-publisher-verified"
        or perception.get("signature_algorithm") != "ed25519"
        or chooser.get("mode") != "live"
        or chooser.get("provider") != "typesafe"
        or not chooser.get("model_id")
    ):
        raise ValueError("source evidence is not a passed, signed, live visual demo")
    if not SHA40.fullmatch(driver.get("source_sha", "")) or not SHA40.fullmatch(chooser.get("source_sha", "")):
        raise ValueError("source evidence contains an invalid source identity")
    expected_artifact = [{"kind": "video", "path": "recording.mp4", "sha256": digest, "size_bytes": size}]
    if artifact != expected_artifact or recording_value.get("final_sha256") != digest or recording_value.get("size_bytes") != size:
        raise ValueError("source recording bytes differ from the source evidence manifest")
    return driver["source_sha"], chooser["source_sha"]


def checked_range(value: dict, label: str, duration_ms: int) -> dict:
    if not isinstance(value, dict) or set(value) != {"start_ms", "end_ms"}:
        raise ValueError(f"{label} must contain exactly start_ms and end_ms")
    start, end = value["start_ms"], value["end_ms"]
    if type(start) is not int or type(end) is not int or not 0 <= start < end <= duration_ms:
        raise ValueError(f"{label} is outside its measured source duration")
    return {"start_ms": start, "end_ms": end}


def build_manifest(*, source_root: Path, edit_plan: Path, reel: Path) -> dict:
    plan = load_json(edit_plan, "edit plan")
    if set(plan) != {"schema", "sources", "shots", "wait_cuts"} or plan["schema"] != "cua-derived-reel-edit-plan/v1":
        raise ValueError("edit plan does not match cua-derived-reel-edit-plan/v1")
    source_values, shot_values, wait_values = plan["sources"], plan["shots"], plan["wait_cuts"]
    if not isinstance(source_values, list) or not 1 <= len(source_values) <= 8:
        raise ValueError("edit plan must contain one through eight sources")
    if not isinstance(shot_values, list) or len(shot_values) < 2:
        raise ValueError("derived reel must contain at least two real trim operations")
    if not isinstance(wait_values, list):
        raise ValueError("wait_cuts must be an array")

    sources: dict[str, dict] = {}
    identities: set[tuple[str, str]] = set()
    for source in source_values:
        if not isinstance(source, dict) or set(source) != {"id", "evidence_manifest", "recording"}:
            raise ValueError("each source must contain exactly id, evidence_manifest, and recording")
        source_id = source.get("id")
        if not isinstance(source_id, str) or not SAFE_ID.fullmatch(source_id) or source_id in sources:
            raise ValueError("source IDs must be unique safe identifiers")
        manifest_path = resolve_input(source_root, source["evidence_manifest"], f"{source_id} manifest")
        recording_path = resolve_input(source_root, source["recording"], f"{source_id} recording")
        manifest = load_json(manifest_path, f"{source_id} evidence manifest")
        identities.add(validate_source_manifest(manifest, recording_path))
        probe = probe_video(recording_path, decode=False)
        sources[source_id] = {
            "id": source_id,
            "platform": manifest["platform"],
            "evidence_manifest_sha256": sha256_file(manifest_path),
            "recording_sha256": sha256_file(recording_path),
            "duration_ms": probe["duration_ms"],
            "used_ranges": [],
            "removed_wait_ranges": [],
        }
    if len(identities) != 1:
        raise ValueError("all source evidence must bind the same Driver and Jev source SHAs")

    operations = []
    reel_cursor = 0
    for index, shot in enumerate(shot_values):
        if not isinstance(shot, dict) or set(shot) != {"source_id", "start_ms", "end_ms"}:
            raise ValueError(f"shot {index} must contain exactly source_id, start_ms, and end_ms")
        source_id = shot.get("source_id")
        if source_id not in sources:
            raise ValueError(f"shot {index} names an unknown source")
        span = checked_range({"start_ms": shot["start_ms"], "end_ms": shot["end_ms"]}, f"shot {index}", sources[source_id]["duration_ms"])
        prior = sources[source_id]["used_ranges"]
        if prior and span["start_ms"] < prior[-1]["end_ms"]:
            raise ValueError("shots from each source must be ordered and non-overlapping")
        prior.append(span)
        length = span["end_ms"] - span["start_ms"]
        operations.append({
            "operation": "trim", "source_id": source_id,
            "source_start_ms": span["start_ms"], "source_end_ms": span["end_ms"],
            "reel_start_ms": reel_cursor, "reel_end_ms": reel_cursor + length, "speed": "1x",
        })
        reel_cursor += length

    declared_waits: dict[tuple[str, int, int], dict] = {}
    for index, wait_cut in enumerate(wait_values):
        if not isinstance(wait_cut, dict) or set(wait_cut) != {"source_id", "start_ms", "end_ms", "disclosure"}:
            raise ValueError(f"wait cut {index} has unexpected fields")
        source_id = wait_cut.get("source_id")
        if source_id not in sources or wait_cut.get("disclosure") != WAIT_DISCLOSURE:
            raise ValueError("every removed wait must use the exact no-action-omitted disclosure")
        span = checked_range({"start_ms": wait_cut["start_ms"], "end_ms": wait_cut["end_ms"]}, f"wait cut {index}", sources[source_id]["duration_ms"])
        key = (source_id, span["start_ms"], span["end_ms"])
        if key in declared_waits:
            raise ValueError("wait cuts must be unique")
        disclosed = {**span, "disclosure": WAIT_DISCLOSURE}
        declared_waits[key] = disclosed
        sources[source_id]["removed_wait_ranges"].append(disclosed)

    required_waits = set()
    for source_id, source in sources.items():
        ranges = source["used_ranges"]
        for before, after in zip(ranges, ranges[1:]):
            if before["end_ms"] < after["start_ms"]:
                required_waits.add((source_id, before["end_ms"], after["start_ms"]))
    if set(declared_waits) != required_waits:
        raise ValueError("wait_cuts must disclose every and only internal interval omitted between used ranges")

    reel_probe = probe_video(reel, decode=True)
    tolerance_ms = max(100, round(reel_probe["frame_duration_ms"] * 2))
    if abs(reel_probe["duration_ms"] - reel_cursor) > tolerance_ms:
        raise ValueError("decoded reel duration does not match the declared 1x edit timeline")
    reel_size = reel.stat().st_size
    reel_sha = sha256_file(reel)
    source_sha, jev_source_sha = identities.pop()
    operations.append({"operation": "concatenate", "shot_count": len(shot_values)})
    return {
        "schema": "cua-visual-perception-derived-reel/v1",
        "identity": {
            "source_sha": source_sha, "jev_source_sha": jev_source_sha,
            "fixture": "visual-only-canvas/v1", "chooser_provider": "typesafe",
        },
        "sources": list(sources.values()),
        "edit_operations": operations,
        "recording": {
            "expected_duration_ms": reel_cursor, "decoded_duration_ms": reel_probe["duration_ms"],
            "fully_decoded": True, "final_sha256": reel_sha, "size_bytes": reel_size,
        },
        "artifacts": [{"kind": "video", "path": "reel.mp4", "sha256": reel_sha, "size_bytes": reel_size}],
    }


def validate_manifest_schema(manifest: dict, schema_path: Path | None = None) -> None:
    """Validate the closed publication shape after semantic/source checks pass."""
    try:
        from jsonschema import Draft202012Validator
    except ImportError as error:
        raise ValueError("jsonschema is required to validate derived reel evidence") from error
    path = schema_path or Path(__file__).with_name("derived-reel-manifest.schema.json")
    schema = load_json(path, "derived reel schema")
    Draft202012Validator.check_schema(schema)
    errors = sorted(Draft202012Validator(schema).iter_errors(manifest), key=lambda item: list(item.path))
    if errors:
        raise ValueError(f"derived reel manifest violates its schema: {errors[0].message}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--edit-plan", type=Path, required=True)
    parser.add_argument("--reel", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    manifest = build_manifest(source_root=args.source_root, edit_plan=args.edit_plan, reel=args.reel)
    validate_manifest_schema(manifest)
    if args.output_dir.exists() and any(args.output_dir.iterdir()):
        raise ValueError("output directory must be empty before reel publication")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(args.reel, args.output_dir / "reel.mp4")
    (args.output_dir / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
