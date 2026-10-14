#!/usr/bin/env python3
"""Measure the synthetic Cua Perception corpus from normalized engine results.

The input contract is intentionally model-neutral. Each Rust or Python result is
a closed JSON object with this shape (all hashes are lowercase SHA-256):

    {
      "schema_version": 1,
      "engine": "rust",
      "identity": {
        "model": {"id": "...", "version": "...", "revision": "..."},
        "runtime": {"id": "...", "version": "..."},
        "adapter": {"id": "...", "version": "..."},
        "reference_machine": {
          "id": "...", "os": "...", "arch": "...", "cpu": "...",
          "memory_bytes": 123
        }
      },
      "bindings": {
        "artifacts": [{"role": "source_model", "id": "omniparser_source",
                       "path": "files/model.pt", "sha256": "..."}],
        "package_artifact_id": "package",
        "installed_artifact_ids": ["worker", "runtime"]
      },
      "images": [{
        "file": "images/example.png", "image_sha256": "...",
        "coordinate_space": {"space": "source_pixels", "origin": "top_left",
                             "bbox_format": "x_y_width_height", "width": 100, "height": 80},
        "regions": [{
          "id": "optional", "kind": "text|control|icon",
          "bounds": {"x": 0, "y": 0, "width": 10, "height": 10},
          "text": "optional", "label": "optional", "confidence": 0.9
        }],
        "samples": [{"phase": "cold|warm", "latency_ms": 1.2,
                     "sampled_process_peak_rss_bytes": 123}]
      }]
    }

All binding paths are confined relative to the result JSON and are omitted from
the report; hashes and sizes are recomputed from those regular files.
Each image must contain exactly one cold sample followed by exactly one warm
sample. RSS is sampled every 5 ms for the single worker process; it does not
include descendants and may miss peaks between samples.
Confidence and duplicate detections do not affect matching. Expected and
predicted regions are compatible when their intersection over union is at least
0.5. OCR additionally requires the manifest's case-sensitive,
outer-whitespace-trimmed target string. Result kinds describe detection
families: both ``control`` and ``icon`` are control detections. Neither family
implies a manifest control subtype; only an explicit ``control_kind`` is scored,
and a missing subtype is reported as unknown through metric coverage. No
release quality threshold is implied.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import struct
import sys
from pathlib import Path, PurePosixPath
from statistics import mean, median
from typing import Any, Callable, Iterable


MAX_JSON_BYTES = 32 * 1024 * 1024
MAX_IMAGES = 10_000
MAX_REGIONS_PER_IMAGE = 100_000
MAX_SAMPLES_PER_IMAGE = 10_000
SHA256_RE = re.compile(r"[0-9a-f]{64}")
MIN_MATCH_IOU = 0.5


class MeasurementError(RuntimeError):
    """An input is unsafe, malformed, inconsistent, or incomplete."""


def _reject_duplicate_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise MeasurementError(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def _reject_constant(value: str) -> None:
    raise MeasurementError(f"non-finite JSON number: {value}")


def read_json(path: Path) -> tuple[dict[str, Any], str]:
    if path.is_symlink() or not path.is_file():
        raise MeasurementError(f"JSON input is not a regular file: {path}")
    data = path.read_bytes()
    if len(data) > MAX_JSON_BYTES:
        raise MeasurementError(f"JSON input exceeds {MAX_JSON_BYTES} bytes: {path}")
    try:
        value = json.loads(
            data,
            object_pairs_hook=_reject_duplicate_keys,
            parse_constant=_reject_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError, OSError) as error:
        raise MeasurementError(f"cannot read JSON {path}: {error}") from error
    if not isinstance(value, dict):
        raise MeasurementError(f"expected a JSON object in {path}")
    return value, hashlib.sha256(data).hexdigest()


def _closed(value: Any, required: Iterable[str], optional: Iterable[str], where: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise MeasurementError(f"{where} must be an object")
    required_set = set(required)
    allowed = required_set | set(optional)
    missing = sorted(required_set - value.keys())
    unknown = sorted(value.keys() - allowed)
    if missing:
        raise MeasurementError(f"{where} is missing fields: {', '.join(missing)}")
    if unknown:
        raise MeasurementError(f"{where} has unknown fields: {', '.join(unknown)}")
    return value


def _list(value: Any, where: str, maximum: int) -> list[Any]:
    if not isinstance(value, list):
        raise MeasurementError(f"{where} must be an array")
    if len(value) > maximum:
        raise MeasurementError(f"{where} exceeds {maximum} entries")
    return value


def _string(value: Any, where: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or (not allow_empty and not value.strip()):
        raise MeasurementError(f"{where} must be a non-empty string")
    if "\x00" in value:
        raise MeasurementError(f"{where} contains a NUL byte")
    return value


def _integer(value: Any, where: str, *, minimum: int = 0) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise MeasurementError(f"{where} must be an integer >= {minimum}")
    return value


def _number(value: Any, where: str, *, minimum: float = 0.0, maximum: float | None = None) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise MeasurementError(f"{where} must be a number")
    result = float(value)
    if not math.isfinite(result) or result < minimum or (maximum is not None and result > maximum):
        limit = f" and <= {maximum}" if maximum is not None else ""
        raise MeasurementError(f"{where} must be finite and >= {minimum}{limit}")
    return result


def _sha256(value: Any, where: str) -> str:
    if not isinstance(value, str) or SHA256_RE.fullmatch(value) is None:
        raise MeasurementError(f"{where} must be a lowercase SHA-256 digest")
    return value


def _safe_relative_file(value: Any, where: str) -> str:
    name = _string(value, where)
    path = PurePosixPath(name)
    if path.is_absolute() or not path.parts or any(part in {"", ".", ".."} for part in path.parts):
        raise MeasurementError(f"{where} is not a safe relative path")
    if "\\" in name:
        raise MeasurementError(f"{where} must use POSIX separators")
    return name


def _bounds(value: Any, where: str, width: int, height: int) -> tuple[float, float, float, float]:
    item = _closed(value, ("x", "y", "width", "height"), (), where)
    x = _number(item["x"], f"{where}.x")
    y = _number(item["y"], f"{where}.y")
    box_width = _number(item["width"], f"{where}.width", minimum=0.0)
    box_height = _number(item["height"], f"{where}.height", minimum=0.0)
    if box_width <= 0 or box_height <= 0:
        raise MeasurementError(f"{where} width and height must be positive")
    if x + box_width > width or y + box_height > height:
        raise MeasurementError(f"{where} is outside its image")
    return x, y, box_width, box_height


def _manifest_bounds(value: Any, where: str, width: int, height: int) -> tuple[float, float, float, float]:
    items = _list(value, where, 4)
    if len(items) != 4:
        raise MeasurementError(f"{where} must contain four integers")
    for index, item in enumerate(items):
        _integer(item, f"{where}[{index}]")
    return _bounds(dict(zip(("x", "y", "width", "height"), items)), where, width, height)


def _png_dimensions(path: Path) -> tuple[int, int]:
    if path.is_symlink() or not path.is_file():
        raise MeasurementError(f"corpus image is not a regular file: {path}")
    with path.open("rb") as stream:
        header = stream.read(24)
    if len(header) != 24 or header[:8] != b"\x89PNG\r\n\x1a\n" or header[12:16] != b"IHDR":
        raise MeasurementError(f"corpus image is not a PNG with an IHDR header: {path}")
    return struct.unpack(">II", header[16:24])


def _file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def validate_manifest(manifest: dict[str, Any], manifest_path: Path) -> list[dict[str, Any]]:
    top = _closed(
        manifest,
        ("schema_version", "corpus_version", "license", "coordinate_system", "metric_definitions", "images"),
        (),
        "manifest",
    )
    if top["schema_version"] != 1:
        raise MeasurementError("manifest.schema_version must be 1")
    _string(top["corpus_version"], "manifest.corpus_version")
    _string(top["license"], "manifest.license")
    coordinates = _closed(top["coordinate_system"], ("space", "origin", "bbox_format"), (), "manifest.coordinate_system")
    if coordinates != {"space": "source_pixels", "origin": "top_left", "bbox_format": "x_y_width_height"}:
        raise MeasurementError("manifest coordinate system is unsupported")
    definitions = top["metric_definitions"]
    if not isinstance(definitions, dict) or not definitions:
        raise MeasurementError("manifest.metric_definitions must be a non-empty object")
    for name, definition in definitions.items():
        _string(name, "manifest metric name")
        item = _closed(definition, ("unit", "description"), (), f"manifest.metric_definitions.{name}")
        _string(item["unit"], f"manifest.metric_definitions.{name}.unit")
        _string(item["description"], f"manifest.metric_definitions.{name}.description")

    root = manifest_path.parent.resolve()
    images: list[dict[str, Any]] = []
    seen_files: set[str] = set()
    seen_hashes: set[str] = set()
    for index, raw in enumerate(_list(top["images"], "manifest.images", MAX_IMAGES)):
        where = f"manifest.images[{index}]"
        item = _closed(raw, ("file", "sha256", "width", "height", "scenario_tags", "expected"), (), where)
        file_name = _safe_relative_file(item["file"], f"{where}.file")
        digest = _sha256(item["sha256"], f"{where}.sha256")
        width = _integer(item["width"], f"{where}.width", minimum=1)
        height = _integer(item["height"], f"{where}.height", minimum=1)
        if file_name in seen_files or digest in seen_hashes:
            raise MeasurementError(f"{where} duplicates a corpus file or hash")
        seen_files.add(file_name)
        seen_hashes.add(digest)
        tags = _list(item["scenario_tags"], f"{where}.scenario_tags", 1_000)
        if len(tags) != len({_string(tag, f"{where}.scenario_tags") for tag in tags}):
            raise MeasurementError(f"{where}.scenario_tags contains duplicates")
        expected = _closed(item["expected"], ("text", "controls"), (), f"{where}.expected")
        text_items: list[dict[str, Any]] = []
        control_items: list[dict[str, Any]] = []
        ids: set[str] = set()
        for text_index, raw_text in enumerate(_list(expected["text"], f"{where}.expected.text", MAX_REGIONS_PER_IMAGE)):
            text_where = f"{where}.expected.text[{text_index}]"
            text = _closed(raw_text, ("id", "text", "bbox", "role"), (), text_where)
            annotation_id = _string(text["id"], f"{text_where}.id")
            if annotation_id in ids:
                raise MeasurementError(f"{text_where}.id is duplicated")
            ids.add(annotation_id)
            target = _string(text["text"], f"{text_where}.text")
            text_items.append({"id": annotation_id, "text": target.strip(), "bounds": _manifest_bounds(text["bbox"], f"{text_where}.bbox", width, height)})
            _string(text["role"], f"{text_where}.role")
        for control_index, raw_control in enumerate(_list(expected["controls"], f"{where}.expected.controls", MAX_REGIONS_PER_IMAGE)):
            control_where = f"{where}.expected.controls[{control_index}]"
            control = _closed(raw_control, ("id", "kind", "label", "bbox", "state"), (), control_where)
            annotation_id = _string(control["id"], f"{control_where}.id")
            if annotation_id in ids:
                raise MeasurementError(f"{control_where}.id is duplicated")
            ids.add(annotation_id)
            _string(control["kind"], f"{control_where}.kind")
            if control["label"] is not None:
                _string(control["label"], f"{control_where}.label")
            _string(control["state"], f"{control_where}.state")
            control_items.append({
                "id": annotation_id,
                "kind": control["kind"],
                "label": control["label"].strip() if control["label"] is not None else None,
                "state": control["state"],
                "bounds": _manifest_bounds(control["bbox"], f"{control_where}.bbox", width, height),
            })
        image_candidate = manifest_path.parent / PurePosixPath(file_name)
        if image_candidate.is_symlink():
            raise MeasurementError(f"{where}.file must not be a symlink")
        image_path = image_candidate.resolve()
        if image_path.parent != root and root not in image_path.parents:
            raise MeasurementError(f"{where}.file escapes the corpus root")
        actual_dimensions = _png_dimensions(image_path)
        if actual_dimensions != (width, height):
            raise MeasurementError(f"{where} dimensions do not match the PNG IHDR")
        actual_hash = _file_sha256(image_path)
        if actual_hash != digest:
            raise MeasurementError(f"{where} SHA-256 does not match the corpus image")
        images.append({
            "file": file_name,
            "sha256": digest,
            "width": width,
            "height": height,
            "scenario_tags": sorted(tags),
            "text": text_items,
            "controls": control_items,
        })
    if not images:
        raise MeasurementError("manifest.images must not be empty")
    return images


def _identity_string(value: Any, where: str) -> str:
    result = _string(value, where)
    if (
        "/" in result
        or "\\" in result
        or "://" in result
        or result.startswith((".", "~"))
        or re.match(r"^[A-Za-z]:", result)
        or any(ord(character) < 32 for character in result)
    ):
        raise MeasurementError(f"{where} must be a path-free identity string")
    return result


def _identity(value: Any, where: str) -> dict[str, Any]:
    identity = _closed(value, ("model", "runtime", "adapter", "reference_machine"), (), where)
    model = _closed(identity["model"], ("id", "version", "revision"), (), f"{where}.model")
    runtime = _closed(identity["runtime"], ("id", "version"), (), f"{where}.runtime")
    adapter = _closed(identity["adapter"], ("id", "version"), (), f"{where}.adapter")
    machine = _closed(identity["reference_machine"], ("id", "os", "arch", "cpu", "memory_bytes"), (), f"{where}.reference_machine")
    return {
        "model": {
            "id": _identity_string(model["id"], f"{where}.model.id"),
            "version": _identity_string(model["version"], f"{where}.model.version"),
            "revision": _identity_string(model["revision"], f"{where}.model.revision"),
        },
        "runtime": {
            "id": _identity_string(runtime["id"], f"{where}.runtime.id"),
            "version": _identity_string(runtime["version"], f"{where}.runtime.version"),
        },
        "adapter": {
            "id": _identity_string(adapter["id"], f"{where}.adapter.id"),
            "version": _identity_string(adapter["version"], f"{where}.adapter.version"),
        },
        "reference_machine": {
            "id": _identity_string(machine["id"], f"{where}.reference_machine.id"),
            "os": _identity_string(machine["os"], f"{where}.reference_machine.os"),
            "arch": _identity_string(machine["arch"], f"{where}.reference_machine.arch"),
            "cpu": _identity_string(machine["cpu"], f"{where}.reference_machine.cpu"),
            "memory_bytes": _integer(machine["memory_bytes"], f"{where}.reference_machine.memory_bytes", minimum=1),
        },
    }


ARTIFACT_ROLES = {
    "source_model",
    "converted_model",
    "ocr_model",
    "ocr_dictionary",
    "model_manifest",
    "conversion_metadata",
    "runtime",
    "worker",
    "package",
    "dependency_lock",
}


def _bound_file(root: Path, value: Any, where: str) -> dict[str, Any]:
    binding = _closed(value, ("role", "id", "path", "sha256"), (), where)
    role = _identity_string(binding["role"], f"{where}.role")
    if role not in ARTIFACT_ROLES:
        raise MeasurementError(f"{where}.role is unsupported")
    artifact_id = _identity_string(binding["id"], f"{where}.id")
    relative = _safe_relative_file(binding["path"], f"{where}.path")
    expected_hash = _sha256(binding["sha256"], f"{where}.sha256")
    candidate = root / PurePosixPath(relative)
    current = root
    for part in PurePosixPath(relative).parts:
        current = current / part
        if current.is_symlink():
            raise MeasurementError(f"{where}.path must not traverse a symlink")
    resolved = candidate.resolve()
    if (resolved.parent != root and root not in resolved.parents) or not resolved.is_file():
        raise MeasurementError(f"{where}.path is not a confined regular file")
    actual_hash = _file_sha256(resolved)
    if actual_hash != expected_hash:
        raise MeasurementError(f"{where}.sha256 does not match the bound file")
    return {
        "role": role,
        "id": artifact_id,
        "sha256": actual_hash,
        "size_bytes": resolved.stat().st_size,
        "resolved": resolved,
    }


def _binding_list(root: Path, value: Any, where: str) -> list[dict[str, Any]]:
    items = [_bound_file(root, raw, f"{where}[{index}]") for index, raw in enumerate(_list(value, where, 10_000))]
    ids = [item["id"] for item in items]
    if len(ids) != len(set(ids)):
        raise MeasurementError(f"{where} contains duplicate artifact ids")
    return sorted(items, key=lambda item: item["id"])


def _binding_identity(items: list[dict[str, Any]]) -> tuple[list[dict[str, Any]], str]:
    public = [
        {"role": item["role"], "id": item["id"], "sha256": item["sha256"], "size_bytes": item["size_bytes"]}
        for item in items
    ]
    encoded = json.dumps(public, sort_keys=True, separators=(",", ":")).encode()
    return public, hashlib.sha256(encoded).hexdigest()


def _coordinate_space(value: Any, where: str, source_width: int, source_height: int) -> tuple[float, float]:
    coordinate = _closed(value, ("space", "origin", "bbox_format", "width", "height"), (), where)
    if coordinate["space"] not in {"source_pixels", "adapter_pixels", "normalized"}:
        raise MeasurementError(f"{where}.space is unsupported")
    if coordinate["origin"] != "top_left" or coordinate["bbox_format"] != "x_y_width_height":
        raise MeasurementError(f"{where} origin or bbox format is unsupported")
    width = _number(coordinate["width"], f"{where}.width")
    height = _number(coordinate["height"], f"{where}.height")
    if width <= 0 or height <= 0:
        raise MeasurementError(f"{where} dimensions must be positive")
    if coordinate["space"] == "source_pixels" and (width != source_width or height != source_height):
        raise MeasurementError(f"{where} source-pixel dimensions must match the corpus image")
    if coordinate["space"] == "normalized" and (width != 1.0 or height != 1.0):
        raise MeasurementError(f"{where} normalized dimensions must be 1 by 1")
    return width, height


def validate_results(
    document: dict[str, Any],
    expected_engine: str,
    corpus: list[dict[str, Any]],
    results_path: Path,
) -> dict[str, Any]:
    top = _closed(document, ("schema_version", "engine", "identity", "bindings", "images"), (), f"{expected_engine} results")
    if top["schema_version"] != 1:
        raise MeasurementError(f"{expected_engine} results schema_version must be 1")
    if top["engine"] != expected_engine:
        raise MeasurementError(f"expected {expected_engine} results, got {top['engine']!r}")
    identity = _identity(top["identity"], f"{expected_engine} results.identity")
    binding_root = results_path.parent.resolve()
    bindings = _closed(
        top["bindings"],
        ("artifacts", "package_artifact_id", "installed_artifact_ids"),
        (),
        f"{expected_engine} results.bindings",
    )
    artifacts = _binding_list(binding_root, bindings["artifacts"], f"{expected_engine} results.bindings.artifacts")
    artifacts_by_id = {item["id"]: item for item in artifacts}
    required_roles = {"source_model", "runtime", "worker", "package", "dependency_lock"}
    missing_roles = sorted(required_roles - {item["role"] for item in artifacts})
    if missing_roles:
        raise MeasurementError(f"{expected_engine} results.bindings.artifacts is missing roles: {', '.join(missing_roles)}")
    role_counts = {role: sum(item["role"] == role for item in artifacts) for role in ARTIFACT_ROLES}
    if role_counts["ocr_model"] == 0:
        raise MeasurementError(f"{expected_engine} results must bind at least one OCR model artifact")
    if expected_engine == "rust" and (
        role_counts["converted_model"] == 0
        or role_counts["ocr_model"] < 2
        or role_counts["ocr_dictionary"] == 0
    ):
        raise MeasurementError(
            "rust results must bind a converted detector, OCR detector and recognizer models, and an OCR dictionary"
        )
    sources = [item for item in artifacts if item["role"] == "source_model"]
    if len(sources) != 1 or sources[0]["id"] != "omniparser_source":
        raise MeasurementError(f"{expected_engine} results must bind exactly one source_model named omniparser_source")
    package_id = _identity_string(bindings["package_artifact_id"], f"{expected_engine} results.bindings.package_artifact_id")
    package = artifacts_by_id.get(package_id)
    if package is None or package["role"] != "package":
        raise MeasurementError(f"{expected_engine} package_artifact_id must identify a package artifact")
    installed_ids = [
        _identity_string(item, f"{expected_engine} results.bindings.installed_artifact_ids")
        for item in _list(bindings["installed_artifact_ids"], f"{expected_engine} results.bindings.installed_artifact_ids", 10_000)
    ]
    if not installed_ids or len(installed_ids) != len(set(installed_ids)):
        raise MeasurementError(f"{expected_engine} installed_artifact_ids must be non-empty and unique")
    try:
        installed = [artifacts_by_id[item] for item in installed_ids]
    except KeyError as error:
        raise MeasurementError(f"{expected_engine} installed_artifact_ids contains an unknown id: {error.args[0]}") from error
    public_artifacts, artifacts_sha256 = _binding_identity(artifacts)
    model_artifacts = [item for item in artifacts if item["role"] in {"source_model", "converted_model", "ocr_model", "ocr_dictionary"}]
    model_public, model_artifacts_sha256 = _binding_identity(model_artifacts)
    runtime_artifacts = [item for item in artifacts if item["role"] in {"runtime", "worker", "dependency_lock"}]
    runtime_public, runtime_artifacts_sha256 = _binding_identity(runtime_artifacts)
    expected_by_file = {image["file"]: image for image in corpus}
    parsed_images: dict[str, dict[str, Any]] = {}
    for index, raw in enumerate(_list(top["images"], f"{expected_engine} results.images", MAX_IMAGES)):
        where = f"{expected_engine} results.images[{index}]"
        item = _closed(raw, ("file", "image_sha256", "coordinate_space", "regions", "samples"), (), where)
        file_name = _safe_relative_file(item["file"], f"{where}.file")
        if file_name in parsed_images:
            raise MeasurementError(f"{where}.file is duplicated")
        corpus_image = expected_by_file.get(file_name)
        if corpus_image is None:
            raise MeasurementError(f"{where}.file is not in the corpus")
        if _sha256(item["image_sha256"], f"{where}.image_sha256") != corpus_image["sha256"]:
            raise MeasurementError(f"{where}.image_sha256 does not match the corpus")
        coordinate_width, coordinate_height = _coordinate_space(
            item["coordinate_space"], f"{where}.coordinate_space", corpus_image["width"], corpus_image["height"]
        )
        regions: list[dict[str, Any]] = []
        for region_index, raw_region in enumerate(_list(item["regions"], f"{where}.regions", MAX_REGIONS_PER_IMAGE)):
            region_where = f"{where}.regions[{region_index}]"
            region = _closed(
                raw_region,
                ("kind", "bounds"),
                ("id", "text", "label", "state", "control_kind", "confidence"),
                region_where,
            )
            kind = region["kind"]
            if kind not in {"text", "control", "icon"}:
                raise MeasurementError(f"{region_where}.kind must be text, control, or icon")
            text = region.get("text")
            label = region.get("label")
            state = region.get("state")
            control_kind = region.get("control_kind")
            if text is not None:
                text = _string(text, f"{region_where}.text").strip()
            if label is not None:
                label = _string(label, f"{region_where}.label").strip()
            if state is not None:
                state = _identity_string(state, f"{region_where}.state")
            if control_kind is not None:
                control_kind = _identity_string(control_kind, f"{region_where}.control_kind")
            if kind == "text" and text is None:
                raise MeasurementError(f"{region_where}.text is required for text regions")
            if "id" in region:
                _string(region["id"], f"{region_where}.id")
            if "confidence" in region:
                _number(region["confidence"], f"{region_where}.confidence", maximum=1.0)
            raw_bounds = _bounds(region["bounds"], f"{region_where}.bounds", coordinate_width, coordinate_height)
            scale_x = corpus_image["width"] / coordinate_width
            scale_y = corpus_image["height"] / coordinate_height
            regions.append({
                "kind": "control" if kind == "icon" else kind,
                "bounds": (raw_bounds[0] * scale_x, raw_bounds[1] * scale_y, raw_bounds[2] * scale_x, raw_bounds[3] * scale_y),
                "text": text,
                "label": label,
                "state": state,
                "control_kind": control_kind,
            })
        samples: list[dict[str, Any]] = []
        raw_samples = _list(item["samples"], f"{where}.samples", MAX_SAMPLES_PER_IMAGE)
        if len(raw_samples) != 2:
            raise MeasurementError(f"{where}.samples must contain exactly one cold and one warm measurement")
        phases: list[str] = []
        for sample_index, raw_sample in enumerate(raw_samples):
            sample_where = f"{where}.samples[{sample_index}]"
            sample = _closed(raw_sample, ("phase", "latency_ms", "sampled_process_peak_rss_bytes"), (), sample_where)
            phase = sample["phase"]
            if phase not in {"cold", "warm"}:
                raise MeasurementError(f"{sample_where}.phase must be cold or warm")
            phases.append(phase)
            samples.append({
                "phase": phase,
                "latency_ms": _number(sample["latency_ms"], f"{sample_where}.latency_ms"),
                "sampled_process_peak_rss_bytes": _integer(
                    sample["sampled_process_peak_rss_bytes"],
                    f"{sample_where}.sampled_process_peak_rss_bytes",
                    minimum=1,
                ),
            })
        if phases != ["cold", "warm"]:
            raise MeasurementError(f"{where}.samples must contain exactly one cold followed by one warm measurement")
        parsed_images[file_name] = {"regions": regions, "samples": samples}
    missing = sorted(expected_by_file.keys() - parsed_images.keys())
    if missing or len(parsed_images) != len(expected_by_file):
        raise MeasurementError(f"{expected_engine} results do not cover the exact corpus image set; missing: {missing}")
    installed_unique = {item["resolved"]: item for item in installed}
    return {
        "identity": {
            **identity,
            "model": {
                **identity["model"],
                "artifacts_sha256": model_artifacts_sha256,
                "artifacts": model_public,
            },
            "runtime": {
                **identity["runtime"],
                "artifacts_sha256": runtime_artifacts_sha256,
                "artifacts": runtime_public,
            },
            "verified_artifacts_sha256": artifacts_sha256,
            "verified_artifacts": public_artifacts,
        },
        "model_comparison": {
            "identity": identity["model"],
            "omniparser_source": {
                "id": sources[0]["id"],
                "sha256": sources[0]["sha256"],
                "size_bytes": sources[0]["size_bytes"],
            },
        },
        "artifact": {
            "sha256": package["sha256"],
            "size_bytes": package["size_bytes"],
            "bound_installed_artifact_bytes": sum(item["size_bytes"] for item in installed_unique.values()),
        },
        "images": parsed_images,
    }


def _center(bounds: tuple[float, float, float, float]) -> tuple[float, float]:
    x, y, width, height = bounds
    return x + width / 2.0, y + height / 2.0


def _center_error(left: tuple[float, float, float, float], right: tuple[float, float, float, float]) -> float:
    left_center = _center(left)
    right_center = _center(right)
    return math.hypot(left_center[0] - right_center[0], left_center[1] - right_center[1])


def _iou(left: tuple[float, float, float, float], right: tuple[float, float, float, float]) -> float:
    left_x, left_y, left_width, left_height = left
    right_x, right_y, right_width, right_height = right
    intersection_width = max(0.0, min(left_x + left_width, right_x + right_width) - max(left_x, right_x))
    intersection_height = max(0.0, min(left_y + left_height, right_y + right_height) - max(left_y, right_y))
    intersection = intersection_width * intersection_height
    union = left_width * left_height + right_width * right_height - intersection
    return intersection / union if union else 0.0


def _geometrically_compatible(left: tuple[float, float, float, float], right: tuple[float, float, float, float]) -> bool:
    return _iou(left, right) >= MIN_MATCH_IOU


def _maximum_matching(
    expected: list[dict[str, Any]],
    predicted: list[dict[str, Any]],
    compatible: Callable[[dict[str, Any], dict[str, Any]], bool],
) -> list[tuple[int, int]]:
    adjacency = [
        sorted(
            (index for index, prediction in enumerate(predicted) if compatible(item, prediction)),
            key=lambda index: (-_iou(item["bounds"], predicted[index]["bounds"]), _center_error(item["bounds"], predicted[index]["bounds"]), index),
        )
        for item in expected
    ]
    prediction_to_expected: dict[int, int] = {}

    def augment(expected_index: int, visited: set[int]) -> bool:
        for prediction_index in adjacency[expected_index]:
            if prediction_index in visited:
                continue
            visited.add(prediction_index)
            previous = prediction_to_expected.get(prediction_index)
            if previous is None or augment(previous, visited):
                prediction_to_expected[prediction_index] = expected_index
                return True
        return False

    for expected_index in range(len(expected)):
        augment(expected_index, set())
    return sorted((expected_index, prediction_index) for prediction_index, expected_index in prediction_to_expected.items())


def _ratio(matched: int, expected: int) -> float | None:
    return matched / expected if expected else None


def _attribute_accuracy(correct: int, evaluated: int, matched: int, eligible: int) -> dict[str, Any]:
    return {
        "status": "available" if evaluated else "unavailable",
        "correct": correct,
        "matched": matched,
        "evaluated": evaluated,
        "coverage": {
            "available": evaluated,
            "eligible": eligible,
            "ratio": _ratio(evaluated, eligible),
        },
        "ratio": _ratio(correct, evaluated),
    }


def _percentile(values: list[float], percentile: float) -> float:
    ordered = sorted(values)
    position = max(0, math.ceil(percentile * len(ordered)) - 1)
    return ordered[position]


def _summary(values: list[float]) -> dict[str, Any]:
    if not values:
        raise MeasurementError("cannot summarize an empty measurement set")
    return {
        "count": len(values),
        "min": min(values),
        "median": median(values),
        "p95": _percentile(values, 0.95),
        "max": max(values),
        "mean": mean(values),
    }


def _optional_summary(values: list[float]) -> dict[str, Any]:
    return _summary(values) if values else {
        "count": 0,
        "min": None,
        "median": None,
        "p95": None,
        "max": None,
        "mean": None,
    }


def _duplicate_neutral_false_positives(
    expected: list[dict[str, Any]],
    predicted: list[dict[str, Any]],
    pairs: list[tuple[int, int]],
    compatible: Callable[[dict[str, Any], dict[str, Any]], bool],
    signature: Callable[[dict[str, Any]], tuple[Any, ...]],
) -> int:
    matched_predictions = {prediction_index for _, prediction_index in pairs}
    false_positives = 0
    for prediction_index, prediction in enumerate(predicted):
        if prediction_index in matched_predictions:
            continue
        duplicate = any(
            compatible(expected[expected_index], prediction)
            and signature(prediction) == signature(predicted[matched_prediction_index])
            for expected_index, matched_prediction_index in pairs
        )
        if not duplicate:
            false_positives += 1
    return false_positives


def measure_engine(corpus: list[dict[str, Any]], results: dict[str, Any], result_sha256: str) -> dict[str, Any]:
    control_expected = 0
    control_matched = 0
    text_expected = 0
    text_matched = 0
    text_recognition_correct = 0
    false_text = 0
    false_controls = 0
    center_errors: list[float] = []
    text_ious: list[float] = []
    control_ious: list[float] = []
    control_kind_correct = 0
    control_kind_evaluated = 0
    control_label_eligible = 0
    control_label_correct = 0
    control_label_evaluated = 0
    control_state_correct = 0
    control_state_evaluated = 0
    latency: dict[str, list[float]] = {"cold": [], "warm": []}
    memory: dict[str, list[float]] = {"cold": [], "warm": []}

    for image in corpus:
        measured = results["images"][image["file"]]
        predicted_text = [region for region in measured["regions"] if region["kind"] == "text"]
        predicted_controls = [region for region in measured["regions"] if region["kind"] == "control"]
        text_detection_pairs = _maximum_matching(
            image["text"],
            predicted_text,
            lambda expected, predicted: _geometrically_compatible(expected["bounds"], predicted["bounds"]),
        )
        ocr_pairs = _maximum_matching(
            image["text"],
            predicted_text,
            lambda expected, predicted: expected["text"] == predicted["text"]
            and _geometrically_compatible(expected["bounds"], predicted["bounds"]),
        )
        control_pairs = _maximum_matching(
            image["controls"],
            predicted_controls,
            lambda expected, predicted: _geometrically_compatible(expected["bounds"], predicted["bounds"]),
        )
        text_expected += len(image["text"])
        text_matched += len(ocr_pairs)
        control_expected += len(image["controls"])
        control_matched += len(control_pairs)
        for expected_index, prediction_index in text_detection_pairs:
            expected = image["text"][expected_index]
            prediction = predicted_text[prediction_index]
            center_errors.append(_center_error(expected["bounds"], prediction["bounds"]))
            text_ious.append(_iou(expected["bounds"], prediction["bounds"]))
            text_recognition_correct += prediction["text"] == expected["text"]
        for expected_index, prediction_index in control_pairs:
            expected = image["controls"][expected_index]
            prediction = predicted_controls[prediction_index]
            center_errors.append(_center_error(expected["bounds"], prediction["bounds"]))
            control_ious.append(_iou(expected["bounds"], prediction["bounds"]))
            if prediction["control_kind"] is not None:
                control_kind_evaluated += 1
                control_kind_correct += prediction["control_kind"] == expected["kind"]
            if prediction["state"] is not None:
                control_state_evaluated += 1
                control_state_correct += prediction["state"] == expected["state"]
            if expected["label"] is not None:
                control_label_eligible += 1
                if prediction["label"] is not None:
                    control_label_evaluated += 1
                    control_label_correct += prediction["label"] == expected["label"]

        false_text += _duplicate_neutral_false_positives(
            image["text"],
            predicted_text,
            text_detection_pairs,
            lambda expected, predicted: _geometrically_compatible(expected["bounds"], predicted["bounds"]),
            lambda prediction: (prediction["text"],),
        )
        false_controls += _duplicate_neutral_false_positives(
            image["controls"],
            predicted_controls,
            control_pairs,
            lambda expected, predicted: _geometrically_compatible(expected["bounds"], predicted["bounds"]),
            lambda prediction: (prediction["control_kind"], prediction["label"], prediction["state"]),
        )
        for sample in measured["samples"]:
            latency[sample["phase"]].append(sample["latency_ms"])
            memory[sample["phase"]].append(float(sample["sampled_process_peak_rss_bytes"]))

    artifact = results["artifact"]
    return {
        "identity": results["identity"],
        "result_sha256": result_sha256,
        "quality": {
            "expected_control_recall": {"matched": control_matched, "expected": control_expected, "ratio": _ratio(control_matched, control_expected)},
            "false_positives": {"count": false_text + false_controls, "text": false_text, "control": false_controls},
            "false_positive_count": {"count": false_text + false_controls, "text": false_text, "control": false_controls},
            "ocr_target_string_recall": {"matched": text_matched, "expected": text_expected, "ratio": _ratio(text_matched, text_expected)},
            "text_detection_iou": _optional_summary(text_ious),
            "text_recognition_exact": {
                "correct": text_recognition_correct,
                "matched": len(text_ious),
                "ratio": _ratio(text_recognition_correct, len(text_ious)),
            },
            "control_detection_iou": _optional_summary(control_ious),
            "control_kind_accuracy": _attribute_accuracy(
                control_kind_correct, control_kind_evaluated, control_matched, control_matched
            ),
            "control_label_accuracy": {
                **_attribute_accuracy(
                    control_label_correct, control_label_evaluated, control_matched, control_label_eligible
                ),
                "expected_labeled": control_label_eligible,
            },
            "control_state_accuracy": _attribute_accuracy(
                control_state_correct, control_state_evaluated, control_matched, control_matched
            ),
            "bounding_box_center_error_pixels": _optional_summary(center_errors),
        },
        "performance": {
            "latency_ms": {"cold": _summary(latency["cold"]), "warm": _summary(latency["warm"])},
            "sampled_process_peak_rss_bytes": {
                "cold": {**_summary(memory["cold"]), "max": int(max(memory["cold"]))},
                "warm": {**_summary(memory["warm"]), "max": int(max(memory["warm"]))},
                "overall_max": int(max(memory["cold"] + memory["warm"])),
            },
        },
        "footprint": {
            "artifact_sha256": artifact["sha256"],
            "artifact_size_bytes": artifact["size_bytes"],
            "bound_installed_artifact_bytes": artifact["bound_installed_artifact_bytes"],
        },
    }


def build_report(manifest_path: Path, rust_results_path: Path, python_results_path: Path) -> dict[str, Any]:
    manifest, manifest_sha256 = read_json(manifest_path)
    corpus = validate_manifest(manifest, manifest_path)
    rust_document, rust_sha256 = read_json(rust_results_path)
    python_document, python_sha256 = read_json(python_results_path)
    rust = validate_results(rust_document, "rust", corpus, rust_results_path)
    python = validate_results(python_document, "python", corpus, python_results_path)
    if rust["identity"]["reference_machine"] != python["identity"]["reference_machine"]:
        raise MeasurementError("Rust and Python results must use the same reference_machine identity")
    if rust["model_comparison"] != python["model_comparison"]:
        raise MeasurementError(
            "Rust and Python results must bind the same model identity and underlying OmniParser/OCR source artifacts"
        )
    image_set_payload = json.dumps(
        [{"sha256": image["sha256"], "width": image["width"], "height": image["height"]} for image in corpus],
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return {
        "schema_version": 1,
        "corpus": {
            "schema_version": manifest["schema_version"],
            "version": manifest["corpus_version"],
            "manifest_sha256": manifest_sha256,
            "image_set_sha256": hashlib.sha256(image_set_payload).hexdigest(),
            "image_count": len(corpus),
        },
        "reference_machine": rust["identity"]["reference_machine"],
        "engines": {
            "python": measure_engine(corpus, python, python_sha256),
            "rust": measure_engine(corpus, rust, rust_sha256),
        },
        "thresholds": {"status": "unset", "values": {}},
    }


def canonical_json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False) + "\n"


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--rust-results", required=True, type=Path)
    parser.add_argument("--python-results", required=True, type=Path)
    parser.add_argument("--output", type=Path, help="write canonical JSON here; stdout when omitted")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        report = build_report(args.manifest, args.rust_results, args.python_results)
        encoded = canonical_json(report)
        if args.output is None:
            sys.stdout.write(encoded)
        else:
            if args.output.is_symlink() or (args.output.exists() and not args.output.is_file()):
                raise MeasurementError(f"output is not a regular file: {args.output}")
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(encoded, encoding="utf-8")
    except (MeasurementError, OSError) as error:
        print(f"measure_quality: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
