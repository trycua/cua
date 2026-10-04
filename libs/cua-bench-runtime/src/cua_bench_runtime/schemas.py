"""Validation against the repository's canonical manifest schemas."""

from __future__ import annotations

import json
import re
from functools import lru_cache
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker
from jsonschema.exceptions import ValidationError
from referencing import Registry, Resource

from cua_bench_runtime.canon import digest_file
from cua_bench_runtime.errors import ValidationFailure

REPO_ROOT = Path(__file__).resolve().parents[2]
SCHEMA_ROOT = Path(__file__).resolve().parent / "schemas_data"
SCHEMA_DIR = SCHEMA_ROOT / "v0.1.0"
SCHEMA_VERSION = re.compile(r"^0\.[0-9]+\.0$")


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationFailure(f"cannot read JSON from {path}: {error}") from error


@lru_cache(maxsize=8)
def _validators(version: str) -> dict[str, Draft202012Validator]:
    if not SCHEMA_VERSION.fullmatch(version):
        raise ValidationFailure(f"unsupported schema version: {version}")
    schema_dir = SCHEMA_ROOT / f"v{version}"
    paths = sorted(schema_dir.glob("*.schema.json"))
    if not paths:
        raise ValidationFailure(f"no schemas found for version {version}")
    schemas = {path.name: load_json(path) for path in paths}
    registry = Registry()
    for schema in schemas.values():
        Draft202012Validator.check_schema(schema)
        registry = registry.with_resource(schema["$id"], Resource.from_contents(schema))
    return {
        name: Draft202012Validator(schema, registry=registry, format_checker=FormatChecker())
        for name, schema in schemas.items()
    }


def infer_kind(path: Path) -> str:
    name = path.name
    for kind in (
        "task",
        "dataset",
        "driver",
        "profile",
        "system",
        "execution-policy",
        "trial",
        "release",
    ):
        if name == f"{kind}.cuabench.json" or name.startswith(f"{kind}."):
            return kind
    raise ValidationFailure(f"cannot infer manifest kind from {name}; pass --kind")


def validate_manifest(path: Path, kind: str | None = None) -> dict[str, Any]:
    path = path.resolve()
    document = load_json(path)
    selected = kind or infer_kind(path)
    version = document.get("schema_version")
    if not isinstance(version, str):
        raise ValidationFailure(f"{path}: /schema_version: must be a string")
    validator = _validators(version).get(f"{selected}.schema.json")
    if validator is None:
        raise ValidationFailure(f"unknown manifest kind: {selected}")
    errors = sorted(validator.iter_errors(document), key=lambda item: list(item.path))
    if errors:
        error = errors[0]
        pointer = "/" + "/".join(str(part) for part in error.absolute_path)
        raise ValidationFailure(f"{path}: {pointer}: {error.message}")
    if selected == "task":
        validate_task_artifacts(path, document)
    return document


def _safe_relative(root: Path, value: str) -> Path:
    resolved_root = root.resolve()
    resolved = (resolved_root / value).resolve()
    if not resolved.is_relative_to(resolved_root):
        raise ValidationFailure(f"artifact path escapes task directory: {value}")
    return resolved


def task_artifacts(task: dict[str, Any]) -> list[dict[str, str]]:
    artifacts: list[dict[str, str]] = []
    for variant in task["variants"]:
        artifacts.extend(variant["fixture_artifacts"])
    artifacts.extend(task["evaluator"]["inputs"])
    for failure in task["reset"]["failure_fixtures"]:
        artifacts.append(failure["artifact"])
    return artifacts


def validate_task_artifacts(path: Path, task: dict[str, Any]) -> None:
    evaluator_inputs = {
        artifact["path"]: artifact["sha256"] for artifact in task["evaluator"]["inputs"]
    }
    if task["evaluator"]["entrypoint"] not in evaluator_inputs:
        raise ValidationFailure("evaluator entrypoint is not a pinned evaluator input")
    for artifact in task_artifacts(task):
        artifact_path = _safe_relative(path.parent, artifact["path"])
        if not artifact_path.is_file():
            raise ValidationFailure(f"missing task artifact: {artifact['path']}")
        actual = digest_file(artifact_path).removeprefix("sha256:")
        if actual != artifact["sha256"]:
            raise ValidationFailure(
                f"task artifact digest mismatch for {artifact['path']}: {actual}"
            )


def schema_suite() -> int:
    """Run the repository's existing semantic and negative validation suite."""

    from cua_bench_runtime import schema_suite as module

    try:
        return int(module.main())
    except (AssertionError, KeyError, OSError, ValidationError, ValueError) as error:
        raise ValidationFailure(str(error)) from error
