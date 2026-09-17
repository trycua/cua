#!/usr/bin/env python3
"""Validate and package non-publishing Cua Perception release candidates."""

from __future__ import annotations

import argparse
import gzip
from hashlib import sha256
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import time
from typing import Any, Mapping, Sequence
import zipfile

try:
    from jsonschema import Draft202012Validator
except ImportError:  # Release packaging intentionally has no network-installed dependencies.
    Draft202012Validator = None  # type: ignore[assignment]


ROOT = Path(__file__).resolve().parents[2]
CONTROL = ROOT / ".github/releases/cua-perception"
SEMVER_RE = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
TARGET_RE = re.compile(r"^[a-z0-9_+.]+(?:-[a-z0-9_+.]+)+$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
KIND_DIRECTORIES = {
    "worker": "bin",
    "runtime": "runtime",
    "model": "models",
    "dictionary": "models",
    "notice": "notices",
    "source": "source",
    "verification-report": "verification",
    "review-recording": "review-recordings",
}


class CandidateError(RuntimeError):
    """A candidate release invariant failed."""


def read_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise CandidateError(f"cannot read JSON from {path}: {error}") from error


def validate_schema(document: Any, schema_path: Path) -> None:
    if Draft202012Validator is None:
        return
    schema = read_json(schema_path)
    errors = sorted(Draft202012Validator(schema).iter_errors(document), key=lambda item: list(item.path))
    if errors:
        details = "; ".join(
            f"{'/'.join(str(part) for part in error.path) or '<root>'}: {error.message}"
            for error in errors
        )
        raise CandidateError(f"{schema_path.name} validation failed: {details}")


def confined_file(root: Path, relative: str) -> Path:
    pure = PurePosixPath(relative)
    if pure.is_absolute() or ".." in pure.parts:
        raise CandidateError(f"input path must be relative and confined: {relative!r}")
    resolved = (root / pure).resolve()
    try:
        resolved.relative_to(root.resolve())
    except ValueError as error:
        raise CandidateError(f"input path escapes payload root: {relative!r}") from error
    if not resolved.is_file() or resolved.is_symlink():
        raise CandidateError(f"input must be a regular non-symlink file: {relative!r}")
    return resolved


def file_digest(path: Path) -> str:
    digest = sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def load_and_validate_manifest(manifest_path: Path, payload_root: Path) -> dict[str, Any]:
    manifest = read_json(manifest_path)
    validate_schema(manifest, CONTROL / "artifact-manifest.schema.json")
    required = {
        "schemaVersion", "component", "version", "driverVersion", "sourceSha",
        "target", "protocol", "artifacts", "modelLedger", "sourceLedger", "verification",
    }
    missing = required - set(manifest) if isinstance(manifest, dict) else required
    if missing:
        raise CandidateError(f"candidate manifest is missing: {', '.join(sorted(missing))}")
    if manifest["schemaVersion"] != 1 or manifest["component"] != "cua-perception":
        raise CandidateError("candidate manifest identity or schema version is invalid")
    if not re.fullmatch(r"[0-9a-f]{40}", str(manifest["sourceSha"])):
        raise CandidateError("candidate sourceSha must be an exact lowercase commit SHA")
    authority = (CONTROL / "VERSION").read_text(encoding="utf-8").strip()
    if manifest["version"] != authority:
        raise CandidateError(
            f"candidate version {manifest['version']} differs from release authority {authority}"
        )
    if not SEMVER_RE.fullmatch(authority):
        raise CandidateError(f"release authority is not stable SemVer: {authority!r}")
    target = manifest["target"]
    if not TARGET_RE.fullmatch(target["triple"]):
        raise CandidateError(f"invalid target triple: {target['triple']!r}")

    names: set[str] = set()
    paths: set[str] = set()
    kinds: dict[str, list[Mapping[str, Any]]] = {}
    for artifact in manifest["artifacts"]:
        name = artifact["name"]
        relative = artifact["path"]
        if name in names or relative in paths:
            raise CandidateError(f"duplicate artifact name or path: {name!r}, {relative!r}")
        names.add(name)
        paths.add(relative)
        source = confined_file(payload_root, relative)
        actual_size = source.stat().st_size
        actual_digest = file_digest(source)
        if actual_size != artifact["size"]:
            raise CandidateError(
                f"size mismatch for {name}: expected {artifact['size']}, got {actual_size}"
            )
        if actual_digest != artifact["sha256"]:
            raise CandidateError(
                f"SHA-256 mismatch for {name}: expected {artifact['sha256']}, got {actual_digest}"
            )
        if not artifact.get("license", {}).get("spdx") or not artifact["license"].get("source"):
            raise CandidateError(f"artifact {name} has incomplete license provenance")
        if artifact["kind"] in {"worker", "runtime"}:
            if artifact.get("target") != target:
                raise CandidateError(f"{artifact['kind']} {name} does not match candidate target")
            if artifact.get("protocolVersion") != manifest["protocol"]["version"]:
                raise CandidateError(f"{artifact['kind']} {name} has a different protocol version")
        kinds.setdefault(artifact["kind"], []).append(artifact)
    if len(kinds.get("worker", [])) != 1:
        raise CandidateError("candidate must contain exactly one worker artifact")
    if len(kinds.get("runtime", [])) != 1:
        raise CandidateError("candidate must contain exactly one target-specific runtime")
    model_roles = {item.get("role") for item in kinds.get("model", [])}
    if model_roles != {"icon-detect", "ocr-detect", "ocr-recognize"}:
        raise CandidateError("candidate must contain exactly the three required model roles")
    dictionaries = kinds.get("dictionary", [])
    if len(dictionaries) != 1 or dictionaries[0].get("role") != "ocr-dictionary":
        raise CandidateError("candidate must contain exactly one OCR dictionary")
    runtime = kinds["runtime"][0]
    expected_runtime_suffix = {"linux": ".so", "macos": ".dylib", "windows": ".dll"}[
        target["os"]
    ]
    if not runtime["name"].lower().endswith(expected_runtime_suffix):
        raise CandidateError(
            f"runtime {runtime['name']} does not match target OS {target['os']}"
        )
    notice_paths = {item["path"] for item in kinds.get("notice", [])}
    for artifact in manifest["artifacts"]:
        if artifact["kind"] == "notice":
            continue
        notice = artifact["license"].get("notice")
        if not notice or notice not in notice_paths:
            raise CandidateError(
                f"{artifact['kind']} {artifact['name']} must reference a declared notice artifact"
            )

    _validate_ledger(manifest, payload_root, "modelLedger", "model", "model-ledger.schema.json")
    _validate_ledger(manifest, payload_root, "sourceLedger", "source", "source-ledger.schema.json")
    _validate_verification_reports(manifest, payload_root, kinds)
    return manifest


def _validate_verification_reports(
    manifest: Mapping[str, Any], payload_root: Path, kinds: Mapping[str, list[Mapping[str, Any]]]
) -> None:
    declared = {item["path"]: item for item in kinds.get("verification-report", [])}
    worker = kinds["worker"][0]
    runtime = kinds["runtime"][0]
    for field, gate in (("health", "health"), ("selfTest", "self-test"), ("realParse", "real-parse")):
        relative = manifest["verification"][field]
        artifact = declared.get(relative)
        if not artifact or artifact.get("role") != gate:
            raise CandidateError(f"verification gate {gate} is not a declared report artifact")
        report = read_json(confined_file(payload_root, relative))
        validate_schema(report, CONTROL / "verification-report.schema.json")
        if report["gate"] != gate or report["target"] != manifest["target"]["triple"]:
            raise CandidateError(f"verification gate {gate} targets a different platform")
        if report["protocolVersion"] != manifest["protocol"]["version"]:
            raise CandidateError(f"verification gate {gate} uses a different protocol")
        if report["workerSha256"] != worker["sha256"] or report["runtimeSha256"] != runtime["sha256"]:
            raise CandidateError(f"verification gate {gate} hashes a different worker or runtime")
        if gate == "real-parse" and (not report.get("fixtureSha256") or not report.get("observations")):
            raise CandidateError("real-parse verification must record a fixture and observations")
        if gate == "self-test" and report.get("mismatchRejectionPassed") is not True:
            raise CandidateError("self-test must prove cross-platform runtime/hash mismatch rejection")


def _validate_ledger(
    manifest: Mapping[str, Any],
    payload_root: Path,
    field: str,
    kind: str,
    schema_name: str,
) -> None:
    artifacts = {item["name"]: item for item in manifest["artifacts"] if item["kind"] == kind}
    artifact_names = set(artifacts)
    ledger_path = manifest.get(field)
    if artifact_names and not ledger_path:
        raise CandidateError(f"{field} is required when {kind} artifacts are supplied")
    if not ledger_path:
        return
    ledger = read_json(confined_file(payload_root, str(ledger_path)))
    validate_schema(ledger, CONTROL / schema_name)
    key = "models" if kind == "model" else "sources"
    if kind == "source" and not ledger[key]:
        raise CandidateError("sourceLedger must contain bundled corresponding source")
    ledger_names = {item["artifact"] for item in ledger[key]}
    if artifact_names != ledger_names:
        raise CandidateError(
            f"{field} entries differ from {kind} artifacts: "
            f"artifacts={sorted(artifact_names)}, ledger={sorted(ledger_names)}"
        )
    entries = {item["artifact"]: item for item in ledger[key]}
    for name, artifact in artifacts.items():
        entry = entries[name]
        if entry["license"] != artifact["license"]["spdx"]:
            raise CandidateError(f"{field} license differs from manifest for {name}")
        if kind == "model" and entry["artifactSha256"] != artifact["sha256"]:
            raise CandidateError(f"{field} SHA-256 differs from manifest for {name}")
        if kind == "model" and entry.get("artifactSize", artifact["size"]) != artifact["size"]:
            raise CandidateError(f"{field} size differs from manifest for {name}")
        if kind == "model" and (
            entry.get("redistributionAllowed") is not True or not entry.get("exportSource")
        ):
            raise CandidateError(f"{field} lacks redistribution or exporter-source proof for {name}")
        if kind == "source":
            if entry.get("artifactSha256") != artifact["sha256"]:
                raise CandidateError(f"{field} SHA-256 differs from manifest for {name}")
            if entry.get("artifactSize") != artifact["size"]:
                raise CandidateError(f"{field} size differs from manifest for {name}")
            if entry.get("revision") != manifest["sourceSha"]:
                raise CandidateError(f"{field} revision differs from manifest sourceSha")
            if entry.get("sourceOfferStatus") != "bundled":
                raise CandidateError("candidate catalog requires bundled corresponding source")


def canonical_json(path: Path, value: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def spdx_id(value: str) -> str:
    return "SPDXRef-" + re.sub(r"[^A-Za-z0-9.-]", "-", value)


def generated_documents(manifest: Mapping[str, Any]) -> tuple[dict[str, Any], dict[str, Any]]:
    artifacts = sorted(manifest["artifacts"], key=lambda item: (item["kind"], item["name"]))
    namespace_seed = sha256(
        f"{manifest['sourceSha']}:{manifest['target']['triple']}:{manifest['version']}".encode()
    ).hexdigest()
    sbom = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"cua-perception-{manifest['version']}-{manifest['target']['triple']}",
        "documentNamespace": f"https://cua.ai/spdx/cua-perception/{namespace_seed}",
        "creationInfo": {"creators": ["Organization: Cua"], "created": "1970-01-01T00:00:00Z"},
        "packages": [
            {
                "name": "cua-perception",
                "SPDXID": "SPDXRef-Package-cua-perception",
                "versionInfo": manifest["version"],
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": True,
                "licenseConcluded": "NOASSERTION",
                "licenseDeclared": "NOASSERTION",
                "copyrightText": "NOASSERTION",
            }
        ],
        "files": [
            {
                "fileName": f"./{KIND_DIRECTORIES[item['kind']]}/{item['name']}",
                "SPDXID": spdx_id(f"File-{item['kind']}-{item['name']}"),
                "checksums": [{"algorithm": "SHA256", "checksumValue": item["sha256"]}],
                "licenseConcluded": item["license"]["spdx"],
                "licenseInfoInFiles": [item["license"]["spdx"]],
                "copyrightText": "NOASSERTION",
            }
            for item in artifacts
        ],
        "relationships": [
            {
                "spdxElementId": "SPDXRef-Package-cua-perception",
                "relationshipType": "CONTAINS",
                "relatedSpdxElement": spdx_id(f"File-{item['kind']}-{item['name']}"),
            }
            for item in artifacts
        ],
    }
    provenance = {
        "$schema": "provenance.schema.json",
        "schemaVersion": 1,
        "component": "cua-perception",
        "candidate": True,
        "releaseStatus": "staging-candidate",
        "source": {
            "repository": os.environ.get("GITHUB_REPOSITORY", "trycua/cua"),
            "commit": manifest["sourceSha"],
        },
        "build": {
            "workflow": os.environ.get("GITHUB_WORKFLOW_REF", "local-candidate"),
            "runId": os.environ.get("GITHUB_RUN_ID", "local"),
            "target": manifest["target"]["triple"],
        },
        "subjects": [
            {"name": item["name"], "digest": {"sha256": item["sha256"]}}
            for item in artifacts
        ],
        "redactions": ["absolutePaths", "environment", "credentials", "runnerIdentity"],
    }
    return sbom, provenance


def extension_manifest(stage: Path, manifest: Mapping[str, Any], payload_root: Path) -> dict[str, Any]:
    model_ledger = read_json(confined_file(payload_root, manifest["modelLedger"]))
    model_entries = {item["artifact"]: item for item in model_ledger["models"]}
    artifacts = manifest["artifacts"]
    files = []
    for path in sorted(stage.rglob("*"), key=lambda item: item.as_posix()):
        if path.is_file():
            relative = path.relative_to(stage).as_posix()
            files.append({
                "path": relative,
                "sha256": file_digest(path),
                "executable": relative.startswith("bin/"),
            })
    models = []
    for item in artifacts:
        if item["kind"] != "model":
            continue
        ledger = model_entries[item["name"]]
        original = ledger.get("sourceArtifact", {}).get("sha256", item["sha256"])
        models.append({
            "path": f"models/{item['name']}",
            "revision": ledger["revision"],
            "original_sha256": original,
            "conversion_sha256": item["sha256"],
        })
    components = [
        {
            "name": item["name"],
            "version": manifest["version"],
            "license": item["license"]["spdx"],
            "notice": f"notices/{Path(item['license']['notice']).name}",
            "source_uri": item["license"]["source"],
            "source_revision": manifest["sourceSha"],
        }
        for item in artifacts
        if item["kind"] != "notice"
    ]
    source = next(item for item in artifacts if item["kind"] == "source")
    worker = next(item for item in artifacts if item["kind"] == "worker")
    return {
        "schema_version": 1,
        "id": "cua-perception",
        "version": manifest["version"],
        "driver_version": manifest["driverVersion"],
        "protocol_version": manifest["protocol"]["version"],
        "target": manifest["target"]["triple"],
        "entrypoint": f"bin/{worker['name']}",
        "files": files,
        "models": models,
        "components": components,
        "license": "LicenseRef-Mixed",
        "source": "https://github.com/trycua/cua",
        "corresponding_source_uri": f"source/{source['name']}",
        "corresponding_source_revision": manifest["sourceSha"],
        "provenance": "metadata/provenance.redacted.json",
        "health_args": ["--health"],
        "self_test_args": ["--self-test"],
    }


def catalog_payload(
    manifest: Mapping[str, Any], archive: Path, manifest_file: Path, key_id: str,
    catalog_version: int, expires_unix: int, next_key: Any,
) -> dict[str, Any]:
    source = next(item for item in manifest["artifacts"] if item["kind"] == "source")
    return {
        "schema_version": 1,
        "catalog_version": catalog_version,
        "expires_unix": expires_unix,
        "publisher_id": "cua",
        "publisher_name": "Cua",
        "key_id": key_id,
        "extension_id": "cua-perception",
        "version": manifest["version"],
        "target": manifest["target"]["triple"],
        "archive": archive.name,
        "archive_size": archive.stat().st_size,
        "archive_sha256": file_digest(archive),
        "manifest_sha256": file_digest(manifest_file),
        "license": "LicenseRef-Mixed",
        "source": "https://github.com/trycua/cua",
        "corresponding_source_uri": f"source/{source['name']}",
        "corresponding_source_revision": manifest["sourceSha"],
        "provenance": "metadata/provenance.redacted.json",
        "next_key": next_key,
    }


def runtime_contract(manifest: Mapping[str, Any]) -> dict[str, Any]:
    artifacts = manifest["artifacts"]
    binding = lambda item: {
        "name": item["name"],
        **({"role": item["role"]} if item.get("role") else {}),
        "sha256": item["sha256"],
    }
    return {
        "$schema": "runtime-contract.schema.json",
        "schemaVersion": 1,
        "target": manifest["target"]["triple"],
        "protocolVersion": manifest["protocol"]["version"],
        "worker": binding(next(item for item in artifacts if item["kind"] == "worker")),
        "runtime": binding(next(item for item in artifacts if item["kind"] == "runtime")),
        "models": [binding(item) for item in artifacts if item["kind"] == "model"],
        "dictionary": binding(next(item for item in artifacts if item["kind"] == "dictionary")),
        "rejectMismatch": True,
    }


def create_archive(stage: Path, destination: Path) -> None:
    with destination.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive:
                for path in sorted(stage.rglob("*"), key=lambda item: item.as_posix()):
                    relative = path.relative_to(stage).as_posix()
                    info = archive.gettarinfo(str(path), arcname=relative)
                    info.uid = info.gid = 0
                    info.uname = info.gname = "root"
                    info.mtime = 0
                    if info.isfile():
                        with path.open("rb") as source:
                            archive.addfile(info, source)
                    else:
                        archive.addfile(info)


def package_candidate(
    manifest_path: Path, payload_root: Path, output: Path, *,
    key_id: str = "cua-extension-ed25519-2026-01",
    catalog_version: int = 1,
    expires_unix: int = 2000000000,
) -> Path:
    trust = read_json(CONTROL / "trust-root.json")
    if key_id != trust.get("activeKeyId"):
        raise CandidateError(f"catalog key id is not the active trust root: {key_id}")
    active_keys = [
        key for key in trust.get("keys", [])
        if key.get("keyId") == key_id and key.get("algorithm") == "Ed25519" and key.get("status") == "active"
    ]
    if len(active_keys) != 1:
        raise CandidateError("active Ed25519 trust root is missing or ambiguous")
    active_key = active_keys[0]
    now = int(time.time())
    if not active_key["validFromUnix"] <= now < active_key["validUntilUnix"]:
        raise CandidateError("active Ed25519 trust root is outside its validity window")
    if catalog_version < 1:
        raise CandidateError("catalog version must be greater than zero")
    if expires_unix <= now:
        raise CandidateError("catalog expiration must be in the future")
    if expires_unix > active_key["validUntilUnix"]:
        raise CandidateError("catalog expiration exceeds the signing key validity window")
    next_key = None
    next_key_id = trust.get("rotation", {}).get("nextKeyId")
    if next_key_id is not None:
        pending = [key for key in trust["keys"] if key.get("keyId") == next_key_id]
        if len(pending) != 1 or pending[0].get("status") != "pending":
            raise CandidateError("pending Ed25519 trust root is missing or ambiguous")
        pending_key = pending[0]
        overlap_seconds = active_key["validUntilUnix"] - pending_key["validFromUnix"]
        minimum_overlap = trust["rotation"]["minimumOverlapDays"] * 86_400
        if overlap_seconds < minimum_overlap:
            raise CandidateError("pending Ed25519 trust root has insufficient overlap")
        next_key = {
            "key_id": pending_key["keyId"],
            "public_key_base64": pending_key["publicKeyBase64"],
            "valid_from_unix": pending_key["validFromUnix"],
            "valid_until_unix": pending_key["validUntilUnix"],
        }
    manifest = load_and_validate_manifest(manifest_path, payload_root)
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="cua-perception-candidate-") as temporary:
        stage = Path(temporary) / "candidate"
        stage.mkdir()
        for item in manifest["artifacts"]:
            destination = stage / KIND_DIRECTORIES[item["kind"]] / item["name"]
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(confined_file(payload_root, item["path"]), destination)
            destination.chmod(0o755 if item["kind"] in {"worker", "runtime"} else 0o644)
        for field in ("modelLedger", "sourceLedger"):
            if field in manifest:
                shutil.copyfile(confined_file(payload_root, manifest[field]), stage / f"{field}.json")

        sbom, provenance = generated_documents(manifest)
        runtime = runtime_contract(manifest)
        canonical_json(stage / "metadata/artifact-manifest.json", manifest)
        canonical_json(stage / "metadata/sbom.spdx.json", sbom)
        canonical_json(stage / "metadata/provenance.redacted.json", provenance)
        canonical_json(stage / "metadata/runtime-contract.json", runtime)
        validate_schema(sbom, CONTROL / "sbom.schema.json")
        validate_schema(provenance, CONTROL / "provenance.schema.json")
        validate_schema(runtime, CONTROL / "runtime-contract.schema.json")
        extension = extension_manifest(stage, manifest, payload_root)
        canonical_json(stage / "extension.json", extension)

        archive = output / (
            f"cua-perception-{manifest['version']}-{manifest['target']['triple']}.tar.gz"
        )
        create_archive(stage, archive)
        payload = catalog_payload(
            manifest, archive, stage / "extension.json", key_id, catalog_version, expires_unix,
            next_key,
        )
        validate_schema(payload, CONTROL / "catalog-input.schema.json")
        (output / "catalog-payload.json").write_text(
            json.dumps(payload, separators=(",", ":"), ensure_ascii=False), encoding="utf-8"
        )
        canonical_json(output / "sbom.spdx.json", sbom)
        canonical_json(output / "provenance.redacted.json", provenance)
        canonical_json(output / "runtime-contract.json", runtime)
        checksum_paths = [
            archive,
            output / "catalog-payload.json",
            output / "sbom.spdx.json",
            output / "provenance.redacted.json",
            output / "runtime-contract.json",
        ]
        (output / "checksums.txt").write_text(
            "".join(f"{file_digest(path)}  {path.name}\n" for path in checksum_paths),
            encoding="utf-8",
        )
    return archive


def archive_names(path: Path) -> list[str]:
    if tarfile.is_tarfile(path):
        with tarfile.open(path, "r:*") as archive:
            return archive.getnames()
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            return archive.namelist()
    raise CandidateError(f"unsupported Driver archive: {path}")


def verify_driver_exclusion(paths: Sequence[Path]) -> None:
    forbidden_suffixes = (".onnx", ".ort", ".gguf", ".safetensors")
    for path in paths:
        for name in archive_names(path):
            lowered = name.lower()
            if (
                "cua-perception" in lowered
                or "/models/" in f"/{lowered.strip('/')}"
                or lowered.endswith(forbidden_suffixes)
            ):
                raise CandidateError(f"Driver archive {path.name} contains Perception payload: {name}")


def run_candidate_gates(manifest_path: Path, payload_root: Path) -> None:
    manifest = load_and_validate_manifest(manifest_path, payload_root)
    worker_item = next(item for item in manifest["artifacts"] if item["kind"] == "worker")
    runtime_item = next(item for item in manifest["artifacts"] if item["kind"] == "runtime")
    worker = confined_file(payload_root, worker_item["path"])
    runtime = confined_file(payload_root, runtime_item["path"])
    worker.chmod(worker.stat().st_mode | stat.S_IXUSR)
    environment = {
        "PATH": os.environ.get("PATH", ""),
        "CUA_PERCEPTION_RUNTIME": str(runtime),
        "CUA_PERCEPTION_RUNTIME_SHA256": runtime_item["sha256"],
        "CUA_PERCEPTION_TARGET": manifest["target"]["triple"],
    }
    for gate, arguments in (
        ("health", ["--health"]),
        ("self-test", ["--self-test"]),
        ("real-parse", ["--real-parse-self-test"]),
        ("mismatch-rejection", ["--mismatch-rejection-self-test"]),
    ):
        result = subprocess.run(
            [str(worker), *arguments],
            cwd=payload_root,
            env=environment,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=120,
        )
        if result.returncode != 0:
            raise CandidateError(
                f"executed {gate} gate failed with exit {result.returncode}: "
                f"{result.stderr.decode(errors='replace')[:500]}"
            )


def verify_checksums(checksum_path: Path) -> None:
    seen: set[str] = set()
    for line in checksum_path.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9._-]+)", line)
        if not match:
            raise CandidateError("checksum manifest has an invalid entry")
        expected, name = match.groups()
        if name in seen:
            raise CandidateError(f"checksum manifest repeats {name}")
        seen.add(name)
        path = checksum_path.parent / name
        if not path.is_file() or file_digest(path) != expected:
            raise CandidateError(f"checksum verification failed for {name}")
    if not seen:
        raise CandidateError("checksum manifest is empty")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate = subparsers.add_parser("validate")
    validate.add_argument("--manifest", type=Path, required=True)
    validate.add_argument("--payload-root", type=Path, required=True)
    package = subparsers.add_parser("package")
    package.add_argument("--manifest", type=Path, required=True)
    package.add_argument("--payload-root", type=Path, required=True)
    package.add_argument("--output", type=Path, required=True)
    package.add_argument("--key-id", required=True)
    package.add_argument("--catalog-version", type=int, required=True)
    package.add_argument("--expires-unix", type=int, required=True)
    driver = subparsers.add_parser("verify-driver-exclusion")
    driver.add_argument("archives", nargs="+", type=Path)
    gates = subparsers.add_parser("run-gates")
    gates.add_argument("--manifest", type=Path, required=True)
    gates.add_argument("--payload-root", type=Path, required=True)
    checksums = subparsers.add_parser("verify-checksums")
    checksums.add_argument("checksum_path", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "validate":
            load_and_validate_manifest(args.manifest, args.payload_root)
            print("Cua Perception candidate inputs are valid")
        elif args.command == "package":
            archive = package_candidate(
                args.manifest,
                args.payload_root,
                args.output,
                key_id=args.key_id,
                catalog_version=args.catalog_version,
                expires_unix=args.expires_unix,
            )
            print(archive)
        elif args.command == "verify-driver-exclusion":
            verify_driver_exclusion(args.archives)
            print("Driver archives exclude Cua Perception payloads")
        elif args.command == "run-gates":
            run_candidate_gates(args.manifest, args.payload_root)
            print("Executed health, self-test, real-parse, and mismatch-rejection gates")
        else:
            verify_checksums(args.checksum_path)
            print("Candidate checksums are valid")
    except (CandidateError, OSError, ValueError) as error:
        print(f"Cua Perception release error: {error}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
