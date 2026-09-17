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
import tarfile
import tempfile
from typing import Any, Mapping, Sequence
import zipfile

from jsonschema import Draft202012Validator


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


def canonical_json(path: Path, value: Mapping[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def spdx_id(value: str) -> str:
    return "SPDXRef-" + re.sub(r"[^A-Za-z0-9.-]", "-", value)


def generated_documents(manifest: Mapping[str, Any]) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any]]:
    artifacts = sorted(manifest["artifacts"], key=lambda item: (item["kind"], item["name"]))
    catalog = {
        "$schema": "catalog-input.schema.json",
        "schemaVersion": 1,
        "component": "cua-perception",
        "version": manifest["version"],
        "sourceSha": manifest["sourceSha"],
        "target": manifest["target"]["triple"],
        "protocolVersion": manifest["protocol"]["version"],
        "candidate": True,
        "releaseStatus": "staging-candidate",
        "signatureRequired": True,
        "artifacts": [
            {
                "name": item["name"],
                "kind": item["kind"],
                "sha256": item["sha256"],
                "size": item["size"],
                "license": item["license"]["spdx"],
            }
            for item in artifacts
        ],
    }
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
    return catalog, sbom, provenance


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


def package_candidate(manifest_path: Path, payload_root: Path, output: Path) -> Path:
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

        catalog, sbom, provenance = generated_documents(manifest)
        runtime = runtime_contract(manifest)
        canonical_json(stage / "metadata/artifact-manifest.json", manifest)
        canonical_json(stage / "metadata/catalog-input.json", catalog)
        canonical_json(stage / "metadata/sbom.spdx.json", sbom)
        canonical_json(stage / "metadata/provenance.redacted.json", provenance)
        canonical_json(stage / "metadata/runtime-contract.json", runtime)
        validate_schema(catalog, CONTROL / "catalog-input.schema.json")
        validate_schema(sbom, CONTROL / "sbom.schema.json")
        validate_schema(provenance, CONTROL / "provenance.schema.json")
        validate_schema(runtime, CONTROL / "runtime-contract.schema.json")

        archive = output / (
            f"cua-perception-{manifest['version']}-{manifest['target']['triple']}.tar.gz"
        )
        create_archive(stage, archive)
        canonical_json(output / "catalog-input.json", catalog)
        canonical_json(output / "sbom.spdx.json", sbom)
        canonical_json(output / "provenance.redacted.json", provenance)
        canonical_json(output / "runtime-contract.json", runtime)
        checksum_paths = [
            archive,
            output / "catalog-input.json",
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
    driver = subparsers.add_parser("verify-driver-exclusion")
    driver.add_argument("archives", nargs="+", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "validate":
            load_and_validate_manifest(args.manifest, args.payload_root)
            print("Cua Perception candidate inputs are valid")
        elif args.command == "package":
            archive = package_candidate(args.manifest, args.payload_root, args.output)
            print(archive)
        else:
            verify_driver_exclusion(args.archives)
            print("Driver archives exclude Cua Perception payloads")
    except (CandidateError, OSError, ValueError) as error:
        print(f"Cua Perception release error: {error}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
