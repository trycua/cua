from __future__ import annotations

from hashlib import sha256
import importlib.util
import json
from pathlib import Path
import tarfile

import pytest
from jsonschema import Draft202012Validator
import yaml


ROOT = Path(__file__).resolve().parents[3]
CONTROL = ROOT / ".github/releases/cua-perception"
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
        "payload/worker": b"worker-v1\n",
        "payload/libonnxruntime.so": b"runtime-v1\n",
        "payload/NOTICE": b"Apache-2.0 notice\n",
        "payload/model.onnx": b"model\n",
        "payload/ocr-det.onnx": b"ocr-det\n",
        "payload/ocr-rec.onnx": b"ocr-rec\n",
        "payload/ocr-dictionary.txt": b"a\nb\n",
        "payload/source.tar": b"source\n",
        "payload/review.mp4": b"review-recording\n",
    }
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
        ("source", "cua-perception-source.tar", "payload/source.tar"),
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
            "gate": gate,
            "status": "passed",
            "target": target["triple"],
            "protocolVersion": 1,
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
            "kind": "verification-report",
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
                "artifact": "cua-perception-source.tar",
                "repository": "https://github.com/trycua/cua",
                "revision": "a" * 40,
                "license": "Apache-2.0",
                "durableLocation": "candidate archive source/",
                "sourceOfferStatus": "bundled",
            }],
        },
    }
    for name, ledger in ledgers.items():
        (payload / name).write_text(json.dumps(ledger))
    manifest = {
        "$schema": "artifact-manifest.schema.json",
        "schemaVersion": 1,
        "component": "cua-perception",
        "version": (CONTROL / "VERSION").read_text().strip(),
        "sourceSha": "a" * 40,
        "target": target,
        "protocol": {"name": "cua-perception-worker", "version": 1},
        "artifacts": artifacts,
        "modelLedger": "model-ledger.json",
        "sourceLedger": "source-ledger.json",
        "verification": verification,
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


def test_packages_deterministically_with_catalog_sbom_and_redacted_provenance(tmp_path: Path) -> None:
    payload, manifest = fixture(tmp_path)
    first, second = tmp_path / "first", tmp_path / "second"
    archive = release.package_candidate(manifest, payload, first)
    repeated = release.package_candidate(manifest, payload, second)
    assert archive.read_bytes() == repeated.read_bytes()
    assert (first / "checksums.txt").read_text().startswith(release.file_digest(archive))
    catalog = json.loads((first / "catalog-input.json").read_text())
    provenance = json.loads((first / "provenance.redacted.json").read_text())
    assert catalog["signatureRequired"] is True and catalog["candidate"] is True
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
    } <= set(names)
    assert catalog["releaseStatus"] == "staging-candidate"
    runtime_contract = json.loads((first / "runtime-contract.json").read_text())
    assert runtime_contract["rejectMismatch"] is True
    assert len(runtime_contract["models"]) == 3


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
    path = ".github/releases/cua-perception"
    assert config["packages"][path]["skip-github-release"] is True
    assert manifest[path] == (CONTROL / "VERSION").read_text().strip()
    assert registry["components"]["cua-perception"]["candidateOnly"] is True
    assert "libs/cua-driver/rust/crates/cua-perception" in registry["components"]["cua-driver-rs"]["changeDetectionExcludePaths"]


def test_workflows_are_valid_and_candidate_workflow_cannot_publish() -> None:
    candidate_path = ROOT / ".github/workflows/cd-cua-perception-candidate.yml"
    ci_path = ROOT / ".github/workflows/ci-cua-perception-release.yml"
    for path in (candidate_path, ci_path):
        assert isinstance(yaml.safe_load(path.read_text()), dict)
    candidate = candidate_path.read_text()
    assert "actions/upload-artifact" in candidate and "actions/download-artifact" in candidate
    assert "github_release.py" not in candidate and "gh release" not in candidate
    assert "contents: write" not in candidate
    assert "STAGING-cua-perception-candidate" in candidate
    assert "openssl dgst -sha256 -verify" in candidate
    assert "sha256sum --check checksums.txt" in candidate


def test_driver_workflow_never_packages_perception() -> None:
    driver = (ROOT / ".github/workflows/cd-rust-cua-driver.yml").read_text()
    assert driver.count("--exclude-path libs/cua-driver/rust/crates/cua-perception") == 2
    assert driver.count("--exclude-path libs/cua-driver/examples") == 2
    assert all("cua-perception" not in line for line in driver.splitlines() if line.strip().startswith("cp "))
