from __future__ import annotations

from hashlib import sha256
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tarfile

import pytest
from jsonschema import Draft202012Validator, ValidationError
import yaml


ROOT = Path(__file__).resolve().parents[3]
CONTROL = ROOT / ".github/releases/cua-perception"
VERSION_AUTHORITY = ROOT / "libs/cua-driver/rust/crates/cua-perception/VERSION"
SPEC = importlib.util.spec_from_file_location(
    "perception_release", ROOT / ".github/scripts/perception_release.py"
)
assert SPEC and SPEC.loader
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


def digest(data: bytes) -> str:
    return sha256(data).hexdigest()


def fixture(tmp_path: Path) -> tuple[Path, Path]:
    payload = tmp_path / "payload-root"
    values = {
        "payload/worker": b'''#!/bin/sh
[ -f fail-gates ] && exit 7
version="$5"
[ -f wrong-identity ] && version="9.9.9"
case "$1" in
  --health) printf '{"protocol":"cua-perception/1","status":"ok","result":{"ready":true,"identity":{"extension":{"id":"cua-perception","version":"%s"}}}}\n' "$version";;
  --self-test) printf '{"protocol":"cua-perception/1","status":"ok","result":{"passed":true,"identity":{"extension":{"id":"cua-perception","version":"%s"}}}}\n' "$version";;
  --real-parse-self-test) printf '{"protocol":"cua-perception/1","status":"ok","result":{"regions":[{"id":"fixture-region"}],"identity":{"extension":{"id":"cua-perception","version":"%s"}}}}\n' "$version";;
  --mismatch-rejection-self-test) printf '{"mismatch_rejection":true}\n';;
  *) exit 2;;
esac
''',
        "payload/libonnxruntime.so": b"runtime-v1\n",
        "payload/NOTICE": b"Apache-2.0 notice\n",
        "payload/model.onnx": b"model\n",
        "payload/ocr-det.onnx": b"ocr-det\n",
        "payload/ocr-rec.onnx": b"ocr-rec\n",
        "payload/ocr-dictionary.txt": b"a\nb\n",
        "payload/review.mp4": b"review-recording\n",
    }
    source_buffer = io.BytesIO()
    with tarfile.open(fileobj=source_buffer, mode="w:gz") as source_archive:
        source_bytes = b"fixture source\n"
        source_info = tarfile.TarInfo("fixture/AGENTS.md")
        source_info.size = len(source_bytes)
        source_archive.addfile(source_info, io.BytesIO(source_bytes))
    values["payload/source.tar.gz"] = source_buffer.getvalue()
    model_manifest = {
        "schema_version": 1,
        "identity": {"name": "fixture", "version": "1", "source_url": "fixture", "source_revision": "fixture", "license": "Apache-2.0"},
        "onnx_runtime": {"version": "1", "library_sha256": digest(values["payload/libonnxruntime.so"]), "intra_threads": 1},
        "detector": {"model": {"path": "models/fixture-model.onnx", "sha256": digest(values["payload/model.onnx"])}, "input_name": "images", "output_name": "output0", "input_width": 32, "input_height": 32, "confidence_threshold": 0.3, "iou_threshold": 0.1, "output_layout": "yolo_v8_cxcywh_class_scores"},
        "ocr": {
            "detector": {"model": {"path": "models/ocr-det.onnx", "sha256": digest(values["payload/ocr-det.onnx"])}, "input_name": "x", "output_name": "out", "input_width": 32, "input_height": 32, "pixel_threshold": 0.3, "box_threshold": 0.6, "unclip_ratio": 1.5, "minimum_area": 1, "max_candidates": 10},
            "recognizer": {"model": {"path": "models/ocr-rec.onnx", "sha256": digest(values["payload/ocr-rec.onnx"])}, "input_name": "x", "output_name": "out", "input_width": 32, "input_height": 16, "blank_index": 0},
            "dictionary": {"path": "models/ocr-dictionary.txt", "sha256": digest(values["payload/ocr-dictionary.txt"])},
            "dictionary_format": "plain_lines",
        },
    }
    values["payload/model-manifest.json"] = (json.dumps(model_manifest) + "\n").encode()
    for relative, content in values.items():
        path = payload / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    target = {"triple": "x86_64-unknown-linux-gnu", "os": "linux", "arch": "x86_64"}
    artifacts = []
    for kind, name, relative in (
        ("worker", "cua-perception-worker", "payload/worker"),
        ("runtime", "libonnxruntime.so", "payload/libonnxruntime.so"),
        ("notice", "NOTICE", "payload/NOTICE"),
        ("model", "fixture-model.onnx", "payload/model.onnx"),
        ("model", "ocr-det.onnx", "payload/ocr-det.onnx"),
        ("model", "ocr-rec.onnx", "payload/ocr-rec.onnx"),
        ("dictionary", "ocr-dictionary.txt", "payload/ocr-dictionary.txt"),
        ("model-manifest", "model-manifest.json", "payload/model-manifest.json"),
        ("source", "cua-perception-source.tar.gz", "payload/source.tar.gz"),
        ("review-recording", "linux-review.mp4", "payload/review.mp4"),
    ):
        content = values[relative]
        item = {
            "kind": kind,
            "name": name,
            "path": relative,
            "sha256": digest(content),
            "size": len(content),
            "license": {
                "spdx": "Apache-2.0",
                "source": "https://github.com/trycua/cua",
                "notice": "payload/NOTICE",
            },
        }
        if kind in {"worker", "runtime"}:
            item.update({"target": target, "protocolVersion": 1})
        roles = {
            "fixture-model.onnx": "icon-detect",
            "ocr-det.onnx": "ocr-detect",
            "ocr-rec.onnx": "ocr-recognize",
            "ocr-dictionary.txt": "ocr-dictionary",
        }
        if name in roles:
            item["role"] = roles[name]
        artifacts.append(item)
    worker_hash = digest(values["payload/worker"])
    runtime_hash = digest(values["payload/libonnxruntime.so"])
    verification = {}
    for gate, field in (("health", "health"), ("self-test", "selfTest"), ("real-parse", "realParse")):
        relative = f"verification/{gate}.json"
        report = {
            "$schema": "verification-report.schema.json",
            "schemaVersion": 1,
            "evidenceKind": "supplied",
            "gate": gate,
            "status": "passed",
            "target": target["triple"],
            "protocolVersion": 1,
            "extensionId": "cua-perception",
            "extensionVersion": VERSION_AUTHORITY.read_text().strip(),
            "workerSha256": worker_hash,
            "runtimeSha256": runtime_hash,
        }
        if gate == "real-parse":
            report.update({"fixtureSha256": "f" * 64, "observations": 1})
        if gate == "self-test":
            report["mismatchRejectionPassed"] = True
        report_bytes = (json.dumps(report) + "\n").encode()
        report_path = payload / relative
        report_path.parent.mkdir(parents=True, exist_ok=True)
        report_path.write_bytes(report_bytes)
        artifacts.append({
            "kind": "supplied-verification-report",
            "role": gate,
            "name": f"{gate}.json",
            "path": relative,
            "sha256": digest(report_bytes),
            "size": len(report_bytes),
            "license": {"spdx": "CC0-1.0", "source": "generated test evidence", "notice": "payload/NOTICE"},
        })
        verification[field] = relative
    ledgers = {
        "model-ledger.json": {
            "$schema": "model-ledger.schema.json",
            "schemaVersion": 1,
            "models": [
                {
                    "artifact": name,
                    "artifactSha256": digest(values[relative]),
                    "artifactSize": len(values[relative]),
                    "origin": "fixture",
                    "revision": "model-v1",
                    "license": "Apache-2.0",
                    "redistributionAllowed": True,
                    "verificationStatus": "release-verified",
                    "exportSource": "fixture exporter",
                }
                for name, relative in (
                    ("fixture-model.onnx", "payload/model.onnx"),
                    ("ocr-det.onnx", "payload/ocr-det.onnx"),
                    ("ocr-rec.onnx", "payload/ocr-rec.onnx"),
                )
            ],
        },
        "source-ledger.json": {
            "$schema": "source-ledger.schema.json",
            "schemaVersion": 1,
            "sources": [{
                "artifact": "cua-perception-source.tar.gz",
                "artifactSha256": digest(values["payload/source.tar.gz"]),
                "artifactSize": len(values["payload/source.tar.gz"]),
                "repository": "https://github.com/trycua/cua",
                "revision": "a" * 40,
                "license": "Apache-2.0",
                "durableLocation": "candidate archive source/",
                "sourceOfferStatus": "bundled",
                "contentKind": "cua-source",
                "format": "tar.gz",
                "requiredPaths": ["AGENTS.md"],
            }],
        },
    }
    for name, ledger in ledgers.items():
        (payload / name).write_text(json.dumps(ledger))
    manifest = {
        "$schema": "artifact-manifest.schema.json",
        "schemaVersion": 1,
        "component": "cua-perception",
        "version": VERSION_AUTHORITY.read_text().strip(),
        "driverVersion": ">=0.28.2",
        "sourceSha": "a" * 40,
        "target": target,
        "protocol": {"name": "cua-perception-worker", "version": 1},
        "artifacts": artifacts,
        "modelLedger": "model-ledger.json",
        "sourceLedger": "source-ledger.json",
        "suppliedVerification": verification,
    }
    manifest_path = payload / "release-input.json"
    manifest_path.write_text(json.dumps(manifest))
    return payload, manifest_path


def test_templates_and_schemas_are_valid_and_pin_known_models() -> None:
    for document_name, schema_name in (
        ("release-input.template.json", "artifact-manifest.schema.json"),
        ("model-ledger.template.json", "model-ledger.schema.json"),
        ("source-ledger.template.json", "source-ledger.schema.json"),
    ):
        document = json.loads((CONTROL / document_name).read_text())
        schema = json.loads((CONTROL / schema_name).read_text())
        Draft202012Validator(schema).validate(document)
    models = json.loads((CONTROL / "model-ledger.template.json").read_text())["models"]
    serialized = json.dumps(models)
    for expected in (
        "6600256cb0f1b07651e3bc86166196307bad7e2d",
        "dab3d4351ad00b035db829909a4db98354d5a90f6990e4ac00222a9a95d4bf57",
        "d8a876bf7f9fb73d7da9432904ade7fa78e092e9a91674e5a2806b45562a9ab2",
        "e6f4fa85f00e168c862bc462aebca69eef9b3d3d",
        "a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d",
        "3fafbc3b5dcf93dd72add9f48368be8a3a2cd33b",
        "b5f833dfc5d0eb71da397b4efa06ebeee9b431b690a47d6af40d77d8eabc557f",
    ):
        assert expected in serialized
    assert models[0]["license"] == "AGPL-3.0-only"
    assert models[0]["verificationStatus"] == "license-review-required"
    platforms = json.loads((CONTROL / "platform-inputs.json").read_text())
    platform_validator = Draft202012Validator(json.loads(
        (CONTROL / "platform-inputs.schema.json").read_text()
    ))
    platform_validator.validate(platforms)
    assert [item["triple"] for item in platforms["platforms"]] == [
        "x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"
    ]
    assert all(item["modelManifest"] == "model-manifest.json" for item in platforms["platforms"])
    artifact_lock = json.loads((
        ROOT / "libs/cua-driver/rust/crates/cua-perception/scripts/artifacts.lock.json"
    ).read_text())
    locked_targets = artifact_lock["onnx_runtime"]["targets"]
    assert all(item["triple"] in locked_targets for item in platforms["platforms"])
    macos = next(item for item in platforms["platforms"] if item["os"] == "macos")
    assert (macos["arch"], macos["runner"], macos["bundleArtifact"]) == (
        "aarch64", "macos-15", "cua-perception-input-macos-aarch64"
    )
    stale_intel_platforms = json.loads(json.dumps(platforms))
    stale_intel_macos = next(
        item for item in stale_intel_platforms["platforms"] if item["os"] == "macos"
    )
    stale_intel_macos.update({
        "arch": "x86_64",
        "triple": "x86_64-apple-darwin",
        "runner": "macos-15-intel",
    })
    with pytest.raises(ValidationError):
        platform_validator.validate(stale_intel_platforms)


def test_packages_deterministically_with_catalog_sbom_and_redacted_provenance(tmp_path: Path) -> None:
    payload, manifest = fixture(tmp_path)
    first, second = tmp_path / "first", tmp_path / "second"
    evidence = tmp_path / "executed.json"
    release.run_candidate_gates(manifest, payload, evidence)
    archive = release.package_candidate(manifest, payload, first, executed_evidence_path=evidence)
    repeated = release.package_candidate(manifest, payload, second, executed_evidence_path=evidence)
    assert archive.read_bytes() == repeated.read_bytes()
    assert (first / "checksums.txt").read_text().startswith(release.file_digest(archive))
    catalog = json.loads((first / "catalog-payload.json").read_text())
    assert (first / "catalog-payload.json").read_bytes() == json.dumps(
        catalog, separators=(",", ":"), ensure_ascii=False
    ).encode()
    provenance = json.loads((first / "provenance.redacted.json").read_text())
    assert catalog["publisher_id"] == "cua"
    assert catalog["key_id"] == "cua-extension-ed25519-2026-01"
    assert catalog["archive_sha256"] == release.file_digest(archive)
    assert catalog["next_key"] is None
    assert list(catalog) == [
        "schema_version", "catalog_version", "expires_unix", "publisher_id",
        "publisher_name", "key_id", "extension_id", "version", "target",
        "archive", "archive_size", "archive_sha256", "manifest_sha256",
        "license", "source", "corresponding_source_uri",
        "corresponding_source_revision", "provenance", "next_key",
    ]
    assert "absolutePaths" in provenance["redactions"]
    assert str(tmp_path) not in json.dumps(provenance)
    with tarfile.open(archive) as candidate:
        names = candidate.getnames()
    assert {
        "bin/cua-perception-worker",
        "models/fixture-model.onnx",
        "review-recordings/linux-review.mp4",
        "metadata/sbom.spdx.json",
        "metadata/runtime-contract.json",
        "metadata/executed-verification.json",
        "model-manifest.json",
        "source/cua-perception-source.tar.gz",
        "extension.json",
    } <= set(names)
    with tarfile.open(archive) as candidate:
        extension_bytes = candidate.extractfile("extension.json").read()
        extension = json.loads(extension_bytes)
        archived_files = {
            member.name for member in candidate.getmembers()
            if member.isfile() and member.name != "extension.json"
        }
    assert {item["path"] for item in extension["files"]} == archived_files
    assert catalog["manifest_sha256"] == digest(extension_bytes)
    assert catalog["extension_id"] == "cua-perception"
    runtime_contract = json.loads((first / "runtime-contract.json").read_text())
    assert runtime_contract["rejectMismatch"] is True
    assert len(runtime_contract["models"]) == 3
    assert runtime_contract["modelManifest"]["name"] == "model-manifest.json"
    assert provenance["evidence"]["supplied"] == [
        "verification/health.json", "verification/self-test.json", "verification/real-parse.json"
    ]
    assert provenance["evidence"]["executed"] == "metadata/executed-verification.json"
    release.verify_checksums(first / "checksums.txt")
    (first / "runtime-contract.json").write_text("tampered\n")
    with pytest.raises(release.CandidateError, match="checksum verification failed"):
        release.verify_checksums(first / "checksums.txt")


@pytest.mark.parametrize("field", ["sha256", "size", "protocolVersion", "target"])
def test_rejects_mismatched_payload_contract(tmp_path: Path, field: str) -> None:
    payload, manifest_path = fixture(tmp_path)
    manifest = json.loads(manifest_path.read_text())
    worker = manifest["artifacts"][0]
    if field == "sha256":
        worker[field] = "0" * 64
    elif field == "size":
        worker[field] += 1
    elif field == "protocolVersion":
        worker[field] += 1
    else:
        worker[field] = {"triple": "aarch64-unknown-linux-gnu", "os": "linux", "arch": "aarch64"}
    manifest_path.write_text(json.dumps(manifest))
    with pytest.raises(release.CandidateError):
        release.load_and_validate_manifest(manifest_path, payload)


def test_model_requires_redistributable_ledger(tmp_path: Path) -> None:
    payload, manifest_path = fixture(tmp_path)
    ledger = json.loads((payload / "model-ledger.json").read_text())
    ledger["models"][0]["redistributionAllowed"] = False
    (payload / "model-ledger.json").write_text(json.dumps(ledger))
    with pytest.raises(release.CandidateError, match="model-ledger.schema.json"):
        release.load_and_validate_manifest(manifest_path, payload)


def test_license_review_model_requires_exact_bundled_source_input(tmp_path: Path) -> None:
    payload, manifest_path = fixture(tmp_path)
    source_path = payload / "payload/source/upstream/omniparser-icon-detect-model.pt"
    source_path.parent.mkdir(parents=True, exist_ok=True)
    source_path.write_bytes(b"exact detector source input\n")
    manifest = json.loads(manifest_path.read_text())
    manifest["artifacts"].append({
        "kind": "source",
        "name": "omniparser-icon-detect-model.pt",
        "path": "payload/source/upstream/omniparser-icon-detect-model.pt",
        "sha256": release.file_digest(source_path),
        "size": source_path.stat().st_size,
        "license": {
            "spdx": "AGPL-3.0-only",
            "source": "fixture source",
            "notice": "payload/NOTICE",
        },
    })
    manifest_path.write_text(json.dumps(manifest))
    source_ledger_path = payload / "source-ledger.json"
    source_ledger = json.loads(source_ledger_path.read_text())
    source_ledger["sources"].append({
        "artifact": "omniparser-icon-detect-model.pt",
        "artifactSha256": release.file_digest(source_path),
        "artifactSize": source_path.stat().st_size,
        "repository": "fixture source",
        "revision": "b" * 40,
        "license": "AGPL-3.0-only",
        "durableLocation": "candidate archive source/upstream/omniparser-icon-detect-model.pt",
        "sourceOfferStatus": "bundled-review-only",
        "contentKind": "model-source-input",
        "format": "file",
    })
    source_ledger_path.write_text(json.dumps(source_ledger))
    model_ledger_path = payload / "model-ledger.json"
    model_ledger = json.loads(model_ledger_path.read_text())
    model_ledger["models"][0]["verificationStatus"] = "license-review-required"
    model_ledger["models"][0]["sourceArtifact"] = {
        "artifact": "omniparser-icon-detect-model.pt",
        "path": "payload/source/upstream/omniparser-icon-detect-model.pt",
        "sha256": release.file_digest(source_path),
        "size": source_path.stat().st_size,
    }
    model_ledger_path.write_text(json.dumps(model_ledger))
    release.load_and_validate_manifest(manifest_path, payload)
    model_ledger["models"][0]["sourceArtifact"]["sha256"] = "0" * 64
    model_ledger_path.write_text(json.dumps(model_ledger))
    with pytest.raises(release.CandidateError, match="source input binding differs"):
        release.load_and_validate_manifest(manifest_path, payload)


def test_source_ledger_must_bind_hash_size_revision_and_bundled_offer(tmp_path: Path) -> None:
    payload, manifest_path = fixture(tmp_path)
    ledger_path = payload / "source-ledger.json"
    original = json.loads(ledger_path.read_text())
    for field, value in (
        ("artifactSha256", "0" * 64),
        ("artifactSize", 999),
        ("revision", "b" * 40),
        ("sourceOfferStatus", "external-review-required"),
    ):
        ledger = json.loads(json.dumps(original))
        ledger["sources"][0][field] = value
        ledger_path.write_text(json.dumps(ledger))
        with pytest.raises(release.CandidateError):
            release.load_and_validate_manifest(manifest_path, payload)
        ledger_path.write_text(json.dumps(original))


def test_bind_source_builds_nonempty_archive_from_exact_checked_out_sha(tmp_path: Path) -> None:
    payload, manifest_path = fixture(tmp_path)
    source_sha = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, check=True, text=True, stdout=subprocess.PIPE
    ).stdout.strip()
    manifest = json.loads(manifest_path.read_text())
    manifest["sourceSha"] = source_sha
    manifest_path.write_text(json.dumps(manifest))
    ledger = json.loads((payload / "source-ledger.json").read_text())
    ledger["sources"][0]["revision"] = source_sha
    (payload / "source-ledger.json").write_text(json.dumps(ledger))
    archive = release.bind_corresponding_source(manifest_path, payload, ROOT)
    assert archive.stat().st_size > 0
    first_bytes = archive.read_bytes()
    assert release.bind_corresponding_source(manifest_path, payload, ROOT).read_bytes() == first_bytes
    with tarfile.open(archive) as source:
        assert source.getnames()[0].rstrip("/") == f"cua-source-{source_sha}"
    rebound = json.loads(manifest_path.read_text())
    source_artifact = next(item for item in rebound["artifacts"] if item["kind"] == "source")
    assert source_artifact["sha256"] == release.file_digest(archive)
    assert source_artifact["size"] == archive.stat().st_size


def test_release_gates_execute_worker_instead_of_trusting_reports(tmp_path: Path) -> None:
    payload, manifest_path = fixture(tmp_path)
    evidence_path = tmp_path / "executed.json"
    evidence = release.run_candidate_gates(manifest_path, payload, evidence_path)
    assert evidence["evidenceKind"] == "executed"
    assert [gate["gate"] for gate in evidence["gates"]] == [
        "health", "self-test", "real-parse", "mismatch-rejection"
    ]
    assert json.loads(evidence_path.read_text()) == evidence
    (payload / "fail-gates").write_text("fail\n")
    with pytest.raises(release.CandidateError, match="executed health gate failed"):
        release.run_candidate_gates(manifest_path, payload)


def test_release_gates_reject_worker_identity_outside_signed_manifest(tmp_path: Path) -> None:
    payload, manifest_path = fixture(tmp_path)
    (payload / "wrong-identity").write_text("wrong\n")
    with pytest.raises(release.CandidateError, match="reported extension identity"):
        release.run_candidate_gates(manifest_path, payload)


@pytest.mark.parametrize(
    ("field", "value"),
    [("extensionId", "wrong"), ("extensionVersion", "9.9.9")],
)
def test_supplied_verification_requires_signed_manifest_identity(
    tmp_path: Path, field: str, value: str
) -> None:
    payload, manifest_path = fixture(tmp_path)
    manifest = json.loads(manifest_path.read_text())
    report_path = payload / manifest["suppliedVerification"]["health"]
    report = json.loads(report_path.read_text())
    report[field] = value
    report_bytes = (json.dumps(report) + "\n").encode()
    report_path.write_bytes(report_bytes)
    report_artifact = next(
        item for item in manifest["artifacts"] if item.get("role") == "health"
    )
    report_artifact["sha256"] = digest(report_bytes)
    report_artifact["size"] = len(report_bytes)
    manifest_path.write_text(json.dumps(manifest))
    with pytest.raises((release.CandidateError, ValidationError)):
        release.load_and_validate_manifest(manifest_path, payload)


def test_packaging_rejects_executed_evidence_for_another_candidate(tmp_path: Path) -> None:
    payload, manifest_path = fixture(tmp_path)
    evidence_path = tmp_path / "executed.json"
    release.run_candidate_gates(manifest_path, payload, evidence_path)
    evidence = json.loads(evidence_path.read_text())
    evidence["sourceSha"] = "b" * 40
    evidence_path.write_text(json.dumps(evidence))
    with pytest.raises(release.CandidateError, match="sourceSha differs"):
        release.package_candidate(
            manifest_path, payload, tmp_path / "candidate", executed_evidence_path=evidence_path
        )


def test_requires_one_exact_model_manifest_bound_to_runtime_and_models(tmp_path: Path) -> None:
    payload, manifest_path = fixture(tmp_path)
    manifest = json.loads(manifest_path.read_text())
    model_manifest = next(item for item in manifest["artifacts"] if item["kind"] == "model-manifest")
    manifest["artifacts"].remove(model_manifest)
    manifest_path.write_text(json.dumps(manifest))
    with pytest.raises(release.CandidateError, match="exactly one model-manifest.json"):
        release.load_and_validate_manifest(manifest_path, payload)

    manifest["artifacts"].extend([model_manifest, {**model_manifest, "name": "other.json", "path": "payload/model-manifest-copy.json"}])
    (payload / "payload/model-manifest-copy.json").write_bytes((payload / model_manifest["path"]).read_bytes())
    manifest_path.write_text(json.dumps(manifest))
    with pytest.raises(release.CandidateError, match="exactly one model-manifest.json"):
        release.load_and_validate_manifest(manifest_path, payload)

    manifest["artifacts"] = [item for item in manifest["artifacts"] if item["path"] != "payload/model-manifest-copy.json"]
    document = json.loads((payload / model_manifest["path"]).read_text())
    document["onnx_runtime"]["library_sha256"] = "0" * 64
    changed = (json.dumps(document) + "\n").encode()
    (payload / model_manifest["path"]).write_bytes(changed)
    model_manifest.update({"sha256": digest(changed), "size": len(changed)})
    manifest_path.write_text(json.dumps(manifest))
    with pytest.raises(release.CandidateError, match="different ONNX Runtime"):
        release.load_and_validate_manifest(manifest_path, payload)


def test_driver_archive_exclusion_is_enforced(tmp_path: Path) -> None:
    clean, dirty = tmp_path / "driver.tar.gz", tmp_path / "dirty.tar.gz"
    driver, model = tmp_path / "cua-driver", tmp_path / "model.onnx"
    driver.write_bytes(b"driver")
    model.write_bytes(b"model")
    with tarfile.open(clean, "w:gz") as archive:
        archive.add(driver, arcname="cua-driver")
    with tarfile.open(dirty, "w:gz") as archive:
        archive.add(model, arcname="models/model.onnx")
    release.verify_driver_exclusion([clean])
    with pytest.raises(release.CandidateError, match="Perception payload"):
        release.verify_driver_exclusion([dirty])


def test_release_stream_is_candidate_only_and_driver_remains_excluded() -> None:
    config = json.loads((ROOT / "release-please-config.json").read_text())
    manifest = json.loads((ROOT / ".release-please-manifest.json").read_text())
    registry = json.loads((ROOT / ".github/releases/components.json").read_text())
    path = "libs/cua-driver/rust/crates/cua-perception"
    assert config["packages"][path]["skip-github-release"] is True
    assert config["packages"][path]["component"] == "cua-perception"
    assert config["packages"][path]["version-file"] == "VERSION"
    assert config["packages"][path]["changelog-path"] == "CHANGELOG.md"
    assert config["packages"][path]["extra-files"] == [{
        "type": "toml",
        "path": "Cargo.toml",
        "jsonpath": "$.package.version",
    }]
    assert "include-paths" not in config["packages"][path]
    assert manifest[path] == (ROOT / path / "VERSION").read_text().strip()
    assert registry["components"]["cua-perception"]["releasePleasePath"] == path
    assert registry["components"]["cua-perception"]["candidateOnly"] is True
    assert "libs/cua-driver/rust/crates/cua-perception" in registry["components"]["cua-driver-rs"]["changeDetectionExcludePaths"]
    assert registry["components"]["cua-perception"]["versionAuthorityFile"] == (
        "libs/cua-driver/rust/crates/cua-perception/VERSION"
    )
    assert registry["components"]["cua-perception"]["changelog"] == (
        "libs/cua-driver/rust/crates/cua-perception/CHANGELOG.md"
    )
    assert not (CONTROL / "VERSION").exists()
    assert not (CONTROL / "CHANGELOG.md").exists()


def test_workflows_are_valid_and_candidate_workflow_cannot_publish() -> None:
    candidate_path = ROOT / ".github/workflows/cd-cua-perception-candidate.yml"
    manual_path = ROOT / ".github/workflows/cd-cua-perception-candidate-manual.yml"
    ci_path = ROOT / ".github/workflows/ci-cua-perception-release.yml"
    for path in (candidate_path, manual_path, ci_path):
        assert isinstance(yaml.safe_load(path.read_text()), dict)
    candidate = candidate_path.read_text()
    assert "actions/upload-artifact" in candidate and "actions/download-artifact" in candidate
    assert "github_release.py" not in candidate and "gh release" not in candidate
    assert "contents: write" not in candidate
    assert "STAGING-cua-perception-candidate" in candidate
    assert "crypto.verify(null, payloadBytes, publicKey, signature)" in candidate
    assert "PERCEPTION_ED25519_PRIVATE_KEY_BASE64" in candidate
    assert "openssl genpkey" not in candidate
    assert "verify-checksums" in candidate
    assert "bind-source" in candidate and "--executed-evidence" in candidate
    assert "refs/cua-reviewed/source" in candidate
    assert "macos:aarch64-apple-darwin" in candidate
    assert "macos-15-intel" not in candidate
    assert "refs/pull/3943/head" in manual_path.read_text()
    parsed = yaml.safe_load(candidate)
    assert "PRIVATE_KEY_BASE64" not in json.dumps(parsed["jobs"]["package"])
    assert "PRIVATE_KEY_BASE64" in json.dumps(parsed["jobs"]["sign"])
    assert parsed["jobs"]["sign"]["environment"] == "cua-perception-candidate-signing"
    manual = manual_path.read_text()
    for value in ("x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"):
        assert value in manual
    assert "cua-perception-input-macos-aarch64" in manual
    assert "x86_64-apple-darwin" not in manual


def test_trust_root_and_rfc8032_vector_match_extension_manager_contract() -> None:
    trust = json.loads((CONTROL / "trust-root.json").read_text())
    vector = json.loads((CONTROL / "ed25519-test-vector.json").read_text())
    Draft202012Validator(json.loads((CONTROL / "trust-root.schema.json").read_text())).validate(trust)
    assert trust["activeKeyId"] == "cua-extension-ed25519-2026-01"
    assert trust["keys"][0]["publicKeyBase64"] == "dB7E/36fTXLiHPfr8ya4i4TFbssU/jpO9zrS8gl4bsQ="
    assert trust["rotation"]["privateKeysStoredInRepository"] is False
    assert trust["keys"][0]["validFromUnix"] == 1735689600
    assert trust["keys"][0]["validUntilUnix"] == 2082758400
    assert vector["publicKeyHex"] == "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
    assert len(vector["signatureBase64"]) == 88
    subprocess.run(
        [
            "node", "-e",
            "const c=require('node:crypto');"
            "const key=Buffer.from('302a300506032b6570032100'+process.argv[1],'hex');"
            "if(!c.verify(null,Buffer.from(process.argv[2],'hex'),"
            "{key,format:'der',type:'spki'},Buffer.from(process.argv[3],'base64')))process.exit(1)",
            vector["publicKeyHex"], vector["messageHex"], vector["signatureBase64"],
        ],
        check=True,
    )


def test_driver_workflow_never_packages_perception() -> None:
    driver = (ROOT / ".github/workflows/cd-rust-cua-driver.yml").read_text()
    assert driver.count("--exclude-path libs/cua-driver/rust/crates/cua-perception") == 2
    assert driver.count("--exclude-path libs/cua-driver/examples") == 2
    assert all("cua-perception" not in line for line in driver.splitlines() if line.strip().startswith("cp "))
