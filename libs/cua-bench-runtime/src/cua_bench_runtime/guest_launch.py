"""Frozen, harness-neutral launch contracts for disposable guests."""

from __future__ import annotations

import json
import re
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from cua_bench_runtime.canon import digest_file
from cua_bench_runtime.errors import ValidationFailure

PLACEHOLDER = re.compile(r"\$\{([a-z_]+)\}")
ALLOWED_PLACEHOLDERS = frozenset(
    {"agent", "workspace", "artifacts", "home", "brief", "driver_socket"}
)
ENVIRONMENT_KEY = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
SHA256 = re.compile(r"^[a-f0-9]{64}$")


@dataclass(frozen=True)
class GuestArtifact:
    path: str
    sha256: str


@dataclass(frozen=True)
class TelemetrySource:
    path: str
    trust: str


@dataclass(frozen=True)
class CollectionBounds:
    max_files: int = 10_000
    max_total_bytes: int = 512 * 1024 * 1024
    max_file_bytes: int = 64 * 1024 * 1024
    max_depth: int = 32


@dataclass(frozen=True)
class RenderedGuestLaunch:
    argv: tuple[str, ...]
    cwd: str
    environment: Mapping[str, str]
    stdin_path: str | None = None
    harness_kind: str | None = None
    executable_sha256: str | None = None
    credential_names: tuple[str, ...] = ()
    support_executables: tuple[tuple[str, str], ...] = ()

    def document(self) -> dict[str, Any]:
        document: dict[str, Any] = {
            "argv": list(self.argv),
            "cwd": self.cwd,
            "environment": dict(self.environment),
        }
        if self.stdin_path is not None:
            document["stdin_path"] = self.stdin_path
        production = (
            self.harness_kind,
            self.executable_sha256,
            self.credential_names or None,
        )
        if any(value is not None for value in production):
            if (
                self.stdin_path is None
                or self.harness_kind is None
                or self.executable_sha256 is None
                or (not self.credential_names and self.harness_kind != "opencode")
            ):
                raise ValidationFailure("production guest launch is incomplete")
            document.update(
                {
                    "harness_kind": self.harness_kind,
                    "executable_sha256": self.executable_sha256,
                    "credential_names": list(self.credential_names),
                }
            )
            if self.support_executables:
                document["support_executables"] = [
                    {"path": path, "sha256": digest} for path, digest in self.support_executables
                ]
        return document


@dataclass(frozen=True)
class GuestLaunchSpec:
    schema_version: int
    id: str
    argv: tuple[str, ...]
    cwd: str
    environment: Mapping[str, str]
    build_artifacts: tuple[GuestArtifact, ...]
    telemetry: tuple[TelemetrySource, ...]
    collection: CollectionBounds
    source: Path
    digest: str

    def render(self, values: Mapping[str, str]) -> RenderedGuestLaunch:
        missing = ALLOWED_PLACEHOLDERS.difference(values)
        if missing:
            raise ValidationFailure(
                f"guest launch placeholder value is missing: {sorted(missing)[0]}"
            )
        rendered_argv = tuple(_render(token, values) for token in self.argv)
        rendered_cwd = _render(self.cwd, values)
        rendered_environment = {
            key: _render(value, values) for key, value in sorted(self.environment.items())
        }
        attempt_root = PurePosixPath(values["home"]).parent
        cwd = PurePosixPath(rendered_cwd)
        if not cwd.is_absolute() or not cwd.is_relative_to(attempt_root):
            raise ValidationFailure("rendered guest working directory escapes attempt root")
        if any("\x00" in token for token in (*rendered_argv, *rendered_environment.values())):
            raise ValidationFailure("rendered guest launch contains a NUL byte")
        return RenderedGuestLaunch(
            argv=rendered_argv,
            cwd=rendered_cwd,
            environment=rendered_environment,
        )


def _render(value: str, values: Mapping[str, str]) -> str:
    return PLACEHOLDER.sub(lambda match: values[match.group(1)], value)


def _closed_placeholders(value: str, label: str) -> None:
    unknown = sorted(set(PLACEHOLDER.findall(value)).difference(ALLOWED_PLACEHOLDERS))
    if unknown:
        raise ValidationFailure(f"unknown guest launch placeholder in {label}: {unknown[0]}")
    residue = re.search(r"\$\{[^}]*\}", value)
    if residue and not PLACEHOLDER.fullmatch(residue.group(0)):
        raise ValidationFailure(f"malformed guest launch placeholder in {label}")


def _relative_path(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValidationFailure(f"{label} must be a non-empty string")
    path = PurePosixPath(value)
    if path.is_absolute() or ".." in path.parts or "\\" in value:
        raise ValidationFailure(f"{label} must be a safe relative POSIX path")
    return value


def _positive(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 1:
        raise ValidationFailure(f"{label} must be a positive integer")
    return value


def load_guest_launch(path: Path) -> GuestLaunchSpec:
    path = path.resolve()
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValidationFailure("guest launch contract is not valid UTF-8 JSON") from error
    if not isinstance(document, dict):
        raise ValidationFailure("guest launch contract must be an object")
    allowed = {
        "schema_version",
        "id",
        "argv",
        "cwd",
        "env",
        "build_artifacts",
        "telemetry",
        "collection",
    }
    unknown = sorted(set(document).difference(allowed))
    if unknown:
        raise ValidationFailure(f"unknown guest launch field: {unknown[0]}")
    if document.get("schema_version") != 1:
        raise ValidationFailure("guest launch schema_version must be 1")
    identifier = document.get("id")
    if not isinstance(identifier, str) or not re.fullmatch(
        r"[a-z0-9]+(?:[._-][a-z0-9]+)+", identifier
    ):
        raise ValidationFailure("guest launch id is invalid")
    raw_argv = document.get("argv")
    if (
        not isinstance(raw_argv, list)
        or not raw_argv
        or not all(isinstance(token, str) and token for token in raw_argv)
    ):
        raise ValidationFailure("guest launch argv must contain non-empty strings")
    cwd = document.get("cwd")
    if not isinstance(cwd, str) or not cwd:
        raise ValidationFailure("guest launch cwd must be a non-empty string")
    for index, token in enumerate(raw_argv):
        _closed_placeholders(token, f"argv[{index}]")
    _closed_placeholders(cwd, "cwd")

    raw_environment = document.get("env", {})
    if not isinstance(raw_environment, dict):
        raise ValidationFailure("guest launch env must be an object")
    environment: dict[str, str] = {}
    for key, value in raw_environment.items():
        if not isinstance(key, str) or not ENVIRONMENT_KEY.fullmatch(key):
            raise ValidationFailure(f"guest launch environment key is invalid: {key}")
        if key.startswith(("DYLD_", "LD_")):
            raise ValidationFailure(f"guest launch environment key is forbidden: {key}")
        if not isinstance(value, str):
            raise ValidationFailure(f"guest launch environment value must be a string: {key}")
        _closed_placeholders(value, f"env.{key}")
        environment[key] = value

    raw_artifacts = document.get("build_artifacts", [])
    if not isinstance(raw_artifacts, list):
        raise ValidationFailure("guest launch build_artifacts must be an array")
    artifacts: list[GuestArtifact] = []
    for item in raw_artifacts:
        if not isinstance(item, dict) or set(item) != {"path", "sha256"}:
            raise ValidationFailure("guest launch build artifact shape is invalid")
        relative = _relative_path(item["path"], "build artifact path")
        sha256 = item["sha256"]
        if not isinstance(sha256, str) or not SHA256.fullmatch(sha256):
            raise ValidationFailure("guest launch build artifact digest is invalid")
        source = (path.parent / relative).resolve()
        if not source.is_relative_to(path.parent) or not source.is_file():
            raise ValidationFailure(f"guest launch build artifact is missing: {relative}")
        if digest_file(source).removeprefix("sha256:") != sha256:
            raise ValidationFailure(f"guest launch build artifact digest mismatch: {relative}")
        artifacts.append(GuestArtifact(relative, sha256))

    raw_telemetry = document.get("telemetry", [])
    if not isinstance(raw_telemetry, list):
        raise ValidationFailure("guest launch telemetry must be an array")
    telemetry: list[TelemetrySource] = []
    for item in raw_telemetry:
        if not isinstance(item, dict) or set(item) != {"path", "trust"}:
            raise ValidationFailure("guest launch telemetry source shape is invalid")
        if item["trust"] != "harness_reported":
            raise ValidationFailure("PR 35 telemetry must be harness_reported")
        telemetry.append(
            TelemetrySource(_relative_path(item["path"], "telemetry path"), item["trust"])
        )

    raw_bounds = document.get("collection", {})
    if not isinstance(raw_bounds, dict):
        raise ValidationFailure("guest launch collection must be an object")
    bounds_allowed = {"max_files", "max_total_bytes", "max_file_bytes", "max_depth"}
    unknown_bounds = sorted(set(raw_bounds).difference(bounds_allowed))
    if unknown_bounds:
        raise ValidationFailure(f"unknown collection bound: {unknown_bounds[0]}")
    defaults = CollectionBounds()
    bounds = CollectionBounds(
        max_files=_positive(raw_bounds.get("max_files", defaults.max_files), "max_files"),
        max_total_bytes=_positive(
            raw_bounds.get("max_total_bytes", defaults.max_total_bytes),
            "max_total_bytes",
        ),
        max_file_bytes=_positive(
            raw_bounds.get("max_file_bytes", defaults.max_file_bytes),
            "max_file_bytes",
        ),
        max_depth=_positive(raw_bounds.get("max_depth", defaults.max_depth), "max_depth"),
    )
    if bounds.max_file_bytes > bounds.max_total_bytes:
        raise ValidationFailure("max_file_bytes cannot exceed max_total_bytes")
    return GuestLaunchSpec(
        schema_version=1,
        id=identifier,
        argv=tuple(raw_argv),
        cwd=cwd,
        environment=environment,
        build_artifacts=tuple(artifacts),
        telemetry=tuple(telemetry),
        collection=bounds,
        source=path,
        digest=digest_file(path),
    )
