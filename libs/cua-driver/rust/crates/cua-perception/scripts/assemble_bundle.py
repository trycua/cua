#!/usr/bin/env python3
"""Assemble and seal a target-native Cua Perception worker bundle."""

from __future__ import annotations

import argparse
import gzip
import json
import shutil
import subprocess
import tarfile
import tempfile
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path
from typing import Any

from artifact_tooling import (
    ArtifactError,
    CRATE_DIR,
    LOCK_PATH,
    extension_identity,
    host_target,
    read_json,
    require_binary_target,
    sha256,
    static_verify,
    verify_file,
    write_json,
)
from verify_bundle import exercise_bundle

SOURCE_PATHS = (
    "LICENSE.md",
    "libs/cua-driver/rust/Cargo.lock",
    "libs/cua-driver/rust/Cargo.toml",
    "libs/cua-driver/rust/crates/cua-perception",
)


def immutable_url(url: str) -> None:
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "https" or parsed.query or parsed.fragment:
        raise ArtifactError(f"artifact URL is not an immutable HTTPS coordinate: {url}")
    if "github.com/microsoft/onnxruntime/releases/download/v1.26.0/" in url:
        return
    marker = "/resolve/"
    if "huggingface.co/" in url and marker in url:
        revision = url.split(marker, 1)[1].split("/", 1)[0]
        if len(revision) == 40 and all(character in "0123456789abcdef" for character in revision):
            return
    if "codeload.github.com/" in url and "/tar.gz/" in url:
        revision = url.rsplit("/tar.gz/", 1)[1]
        if len(revision) == 40 and all(character in "0123456789abcdef" for character in revision):
            return
    if parsed.netloc == "files.pythonhosted.org" and parsed.path.startswith("/packages/"):
        return
    raise ArtifactError(f"artifact URL does not pin an approved tag or commit: {url}")


def download(url: str, destination: Path, expected_hash: str, expected_size: int | None) -> Path:
    immutable_url(url)
    if destination.exists():
        verify_file(destination, expected_hash, expected_size)
        return destination
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(prefix=destination.name + ".", dir=destination.parent, delete=False) as stream:
        temporary = Path(stream.name)
        request = urllib.request.Request(url, headers={"User-Agent": "cua-perception-assembler/1"})
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                shutil.copyfileobj(response, stream, length=1024 * 1024)
        except Exception:
            temporary.unlink(missing_ok=True)
            raise
    try:
        verify_file(temporary, expected_hash, expected_size)
        temporary.replace(destination)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    return destination


def obtain_artifact(entry: dict[str, Any], inputs: Path, cache: Path) -> Path:
    supplied = inputs / entry["filename"]
    if supplied.is_file():
        verify_file(supplied, entry["sha256"], entry["size"])
        return supplied
    if not entry.get("url"):
        raise ArtifactError(
            f"required reviewed conversion is absent: place {entry['filename']} in {inputs}"
        )
    return download(entry["url"], cache / entry["filename"], entry["sha256"], entry["size"])


RUNTIME_NOTICE_FILES = {
    "LICENSE": "onnxruntime-LICENSE.txt",
    "ThirdPartyNotices.txt": "onnxruntime-ThirdPartyNotices.txt",
}
LICENSE_TEXTS = ("AGPL-3.0-only.txt", "Apache-2.0.txt")


def runtime_notice_members(target_lock: dict[str, Any]) -> dict[str, str]:
    member = target_lock["archive_member"]
    prefix = member.split("/lib/", 1)[0]
    return {f"{prefix}/{name}": output for name, output in RUNTIME_NOTICE_FILES.items()}


def extract_runtime(
    lock: dict[str, Any], target: str, cache: Path, destination: Path, licenses: Path
) -> None:
    target_lock = lock["onnx_runtime"]["targets"][target]
    archive_name = target_lock["archive_url"].rsplit("/", 1)[1]
    archive = download(
        target_lock["archive_url"],
        cache / archive_name,
        target_lock["archive_sha256"],
        None,
    )
    if target_lock["archive_format"] == "zip":
        with zipfile.ZipFile(archive) as package:
            members = [name for name in package.namelist() if name == target_lock["archive_member"]]
            if members != [target_lock["archive_member"]]:
                raise ArtifactError(f"runtime archive does not contain one exact target member: {members}")
            with package.open(members[0]) as source, destination.open("wb") as output:
                shutil.copyfileobj(source, output, length=1024 * 1024)
            for name, output_name in runtime_notice_members(target_lock).items():
                if name not in package.namelist():
                    raise ArtifactError(f"runtime archive lacks license file: {name}")
                (licenses / output_name).write_bytes(package.read(name))
    elif target_lock["archive_format"] == "tar.gz":
        with tarfile.open(archive, mode="r:gz") as package:
            members = [member for member in package.getmembers() if member.name == target_lock["archive_member"]]
            if len(members) != 1 or not members[0].isfile():
                raise ArtifactError("runtime archive does not contain one exact regular target member")
            source = package.extractfile(members[0])
            if source is None:
                raise ArtifactError("runtime archive target member cannot be read")
            with source, destination.open("wb") as output:
                shutil.copyfileobj(source, output, length=1024 * 1024)
            for name, output_name in runtime_notice_members(target_lock).items():
                notice_members = [item for item in package.getmembers() if item.name == name]
                if len(notice_members) != 1 or not notice_members[0].isfile():
                    raise ArtifactError(f"runtime archive lacks license file: {name}")
                notice_source = package.extractfile(notice_members[0])
                if notice_source is None:
                    raise ArtifactError(f"runtime archive license file cannot be read: {name}")
                with notice_source:
                    (licenses / output_name).write_bytes(notice_source.read())
    else:
        raise ArtifactError(f"unsupported runtime archive format: {target_lock['archive_format']}")
    verify_file(destination, target_lock["sha256"], target_lock["size"])
    require_binary_target(destination, target)


def repository_root() -> Path:
    result = subprocess.run(
        ["git", "-C", str(CRATE_DIR), "rev-parse", "--show-toplevel"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise ArtifactError(result.stderr.strip())
    return Path(result.stdout.strip())


def corresponding_source(repo: Path, revision: str, destination: Path) -> None:
    if len(revision) != 40 or any(character not in "0123456789abcdef" for character in revision):
        raise ArtifactError("source revision must be an exact lowercase 40-character commit")
    check = subprocess.run(
        ["git", "-C", str(repo), "cat-file", "-e", f"{revision}^{{commit}}"], check=False
    )
    if check.returncode != 0:
        raise ArtifactError(f"source commit is unavailable locally: {revision}")
    archive = subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "archive",
            "--format=tar",
            "--prefix=cua-perception-source/",
            revision,
            "--",
            *SOURCE_PATHS,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if archive.returncode != 0:
        raise ArtifactError(archive.stderr.decode(errors="replace").strip())
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(gzip.compress(archive.stdout, compresslevel=9, mtime=0))


def model_manifest(lock: dict[str, Any], target: str) -> dict[str, Any]:
    artifacts = {entry["role"]: entry for entry in lock["artifacts"]}
    return {
        "schema_version": 1,
        "identity": {
            "name": "omniparser-v2-ppocrv5-en",
            "version": "2026-09-17",
            "source_url": "https://huggingface.co/microsoft/OmniParser-v2.0",
            "source_revision": artifacts["icon-detect"]["revision"],
            "license": "AGPL-3.0-only (detector); Apache-2.0 (OCR)",
        },
        "onnx_runtime": {
            "version": lock["onnx_runtime"]["version"],
            "target": target,
            "library_sha256": lock["onnx_runtime"]["targets"][target]["sha256"],
            "intra_threads": 2,
        },
        "detector": {
            "model": {"path": f"models/{artifacts['icon-detect']['filename']}", "sha256": artifacts["icon-detect"]["sha256"]},
            "input_name": "images", "output_name": "output0", "input_width": 1280,
            "input_height": 1280, "confidence_threshold": 0.3, "iou_threshold": 0.1,
            "output_layout": "yolo_v8_cxcywh_class_scores",
            "max_candidates": 10000, "max_detections": 1000,
        },
        "ocr": {
            "detector": {
                "model": {"path": f"models/{artifacts['ocr-detect']['filename']}", "sha256": artifacts["ocr-detect"]["sha256"]},
                "input_name": "x", "output_name": "fetch_name_0", "input_width": 960,
                "input_height": 960, "pixel_threshold": 0.3, "box_threshold": 0.6,
                "unclip_ratio": 1.5, "minimum_area": 3, "minimum_side": 3,
                "max_candidates": 1000,
            },
            "recognizer": {
                "model": {"path": f"models/{artifacts['ocr-recognize']['filename']}", "sha256": artifacts["ocr-recognize"]["sha256"]},
                "input_name": "x", "output_name": "fetch_name_0", "input_width": 320,
                "input_height": 48, "blank_index": 0,
            },
            "dictionary": {"path": f"models/{artifacts['ocr-dictionary']['filename']}", "sha256": artifacts["ocr-dictionary"]["sha256"]},
            "dictionary_format": "paddle_inference_yaml",
        },
    }


def model_ledger(lock: dict[str, Any]) -> dict[str, Any]:
    models = []
    for entry in lock["artifacts"]:
        if entry["role"] == "ocr-dictionary":
            continue
        model = {
            "artifact": entry["filename"], "artifactSha256": entry["sha256"],
            "artifactSize": entry["size"], "origin": entry["origin"],
            "revision": entry["revision"], "license": entry["license"],
            "redistributionAllowed": True, "verificationStatus": entry["verification_status"],
            "exportSource": "source/cua-perception-source.tar.gz contains models/conversion-recipe.json and models/export_omniparser_detector.py; principal exporter sources are bundled under source/exporter/",
        }
        if entry["role"] == "icon-detect":
            model["sourceArtifact"] = {
                "artifact": "omniparser-icon-detect-model.pt",
                "path": "source/upstream/omniparser-icon-detect-model.pt",
                "sha256": entry["source_sha256"], "size": 40623819
            }
            model["usageRestrictions"] = [
                "AGPL-3.0-only: redistribution, and network use of this converted model, require the corresponding source bundled under source/; see THIRD_PARTY_NOTICES.md and SOURCE_OFFER.md"
            ]
        models.append(model)
    return {"schemaVersion": 1, "models": models}


def artifact(
    path: Path,
    root: Path,
    kind: str,
    license_id: str,
    notice: bool = True,
    source_url: str = "https://github.com/trycua/cua",
    **extra: Any,
) -> dict[str, Any]:
    relative = path.relative_to(root).as_posix()
    license_value: dict[str, str] = {"spdx": license_id, "source": source_url}
    if notice:
        license_value["notice"] = "THIRD_PARTY_NOTICES.md"
    return {
        "kind": kind, "name": path.name, "path": relative, "sha256": sha256(path),
        "size": path.stat().st_size, "license": license_value, **extra,
    }


def release_manifest(bundle: Path, lock: dict[str, Any], target: str, version: str, revision: str) -> dict[str, Any]:
    target_lock = lock["onnx_runtime"]["targets"][target]
    target_value = {"triple": target, "os": target_lock["os"], "arch": target_lock["arch"]}
    entries = [
        artifact(bundle / target_lock["worker_filename"], bundle, "worker", "MIT", target=target_value, protocolVersion=1),
        artifact(
            bundle / target_lock["filename"], bundle, "runtime", "MIT",
            source_url="https://github.com/microsoft/onnxruntime",
            target=target_value, protocolVersion=1,
        ),
    ]
    for item in lock["artifacts"]:
        kind = "dictionary" if item["role"] == "ocr-dictionary" else "model"
        entries.append(
            artifact(
                bundle / "models" / item["filename"], bundle, kind, item["license"],
                source_url=item["origin"], role=item["role"],
            )
        )
    entries.extend([
        artifact(bundle / "model-manifest.json", bundle, "model-manifest", "MIT"),
        artifact(bundle / "THIRD_PARTY_NOTICES.md", bundle, "notice", "MIT", notice=False),
        artifact(bundle / "SOURCE_OFFER.md", bundle, "notice", "AGPL-3.0-only", notice=False),
        artifact(bundle / "licenses/AGPL-3.0-only.txt", bundle, "notice", "AGPL-3.0-only", notice=False),
        artifact(bundle / "licenses/Apache-2.0.txt", bundle, "notice", "Apache-2.0", notice=False),
        artifact(
            bundle / "licenses/onnxruntime-LICENSE.txt", bundle, "notice", "MIT", notice=False,
            source_url="https://github.com/microsoft/onnxruntime",
        ),
        artifact(
            bundle / "licenses/onnxruntime-ThirdPartyNotices.txt", bundle, "notice", "MIT", notice=False,
            source_url="https://github.com/microsoft/onnxruntime",
        ),
        artifact(bundle / "source/cua-perception-source.tar.gz", bundle, "source", "MIT"),
        artifact(bundle / "verification/health.json", bundle, "supplied-verification-report", "MIT", role="health"),
        artifact(bundle / "verification/self-test.json", bundle, "supplied-verification-report", "MIT", role="self-test"),
        artifact(bundle / "verification/real-parse.json", bundle, "supplied-verification-report", "MIT", role="real-parse"),
    ])
    for source in lock["corresponding_source"]:
        entries.append(artifact(
            bundle / source["bundle_path"], bundle, "source", source["license"],
            source_url=source["repository"],
        ))
    return {
        "schemaVersion": 1, "component": "cua-perception", "version": version,
        "driverVersion": ">=0.28.2", "sourceSha": revision, "target": target_value,
        "protocol": {"name": "cua-perception-worker", "version": 1}, "artifacts": entries,
        "modelLedger": "model-ledger.json", "sourceLedger": "source-ledger.json",
        "suppliedVerification": {"health": "verification/health.json", "selfTest": "verification/self-test.json", "realParse": "verification/real-parse.json"},
    }


def write_sums(bundle: Path) -> None:
    lines = []
    for path in sorted(item for item in bundle.rglob("*") if item.is_file() and item.name != "SHA256SUMS"):
        if path.is_symlink():
            raise ArtifactError(f"bundle cannot contain symlinks: {path}")
        lines.append(f"{sha256(path)}  {path.relative_to(bundle).as_posix()}")
    (bundle / "SHA256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")


def deterministic_bundle_archive(bundle: Path, destination: Path) -> None:
    with destination.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, compresslevel=9, mtime=0) as zipped:
            with tarfile.open(fileobj=zipped, mode="w") as archive:
                for path in sorted([bundle, *bundle.rglob("*")]):
                    if path.is_symlink():
                        raise ArtifactError(f"bundle cannot contain symlinks: {path}")
                    info = archive.gettarinfo(str(path), arcname=path.relative_to(bundle.parent).as_posix())
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    info.mtime = 0
                    if path.is_file():
                        with path.open("rb") as stream:
                            archive.addfile(info, stream)
                    else:
                        archive.addfile(info)


def assemble(args: argparse.Namespace) -> tuple[Path, Path]:
    lock = read_json(LOCK_PATH)
    if args.target not in lock["onnx_runtime"]["targets"]:
        raise ArtifactError(f"unsupported target: {args.target}")
    if args.target != host_target():
        raise ArtifactError(f"assembly must run on target host {args.target}; current host is {host_target()}")
    identity = extension_identity({"component": "cua-perception", "version": args.version})
    if args.output.exists():
        raise ArtifactError(f"refusing to overwrite existing output: {args.output}")
    target_lock = lock["onnx_runtime"]["targets"][args.target]
    require_binary_target(args.worker, args.target)
    repo = repository_root()
    bundle = args.output
    bundle.mkdir(parents=True)
    try:
        worker = bundle / target_lock["worker_filename"]
        shutil.copyfile(args.worker, worker)
        worker.chmod(0o755)
        runtime = bundle / target_lock["filename"]
        licenses = bundle / "licenses"
        licenses.mkdir()
        extract_runtime(lock, args.target, args.cache, runtime, licenses)
        for name in LICENSE_TEXTS:
            shutil.copyfile(CRATE_DIR / "models/licenses" / name, licenses / name)
        models_dir = bundle / "models"
        models_dir.mkdir()
        for entry in lock["artifacts"]:
            shutil.copyfile(obtain_artifact(entry, args.inputs, args.cache), models_dir / entry["filename"])
        for entry in lock["corresponding_source"]:
            source_material = obtain_artifact(entry, args.inputs, args.cache)
            destination = bundle / entry["bundle_path"]
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source_material, destination)
        write_json(bundle / "model-manifest.json", model_manifest(lock, args.target))
        shutil.copyfile(CRATE_DIR / "models/THIRD_PARTY_NOTICES.md", bundle / "THIRD_PARTY_NOTICES.md")
        shutil.copyfile(CRATE_DIR / "models/SOURCE_OFFER.md", bundle / "SOURCE_OFFER.md")
        write_json(bundle / "model-ledger.json", model_ledger(lock))
        source = bundle / "source/cua-perception-source.tar.gz"
        corresponding_source(repo, args.source_sha, source)
        source_entries = [{
                "artifact": source.name, "artifactSha256": sha256(source), "artifactSize": source.stat().st_size,
                "repository": "https://github.com/trycua/cua", "revision": args.source_sha, "license": "MIT",
                "durableLocation": f"Candidate archive {source.relative_to(bundle).as_posix()}",
                "sourceOfferStatus": "bundled", "contentKind": "cua-source",
                "format": "tar.gz", "requiredPaths": [
                    *SOURCE_PATHS,
                    "libs/cua-driver/rust/crates/cua-perception/models/conversion-recipe.json",
                    "libs/cua-driver/rust/crates/cua-perception/models/export_omniparser_detector.py",
                    "libs/cua-driver/rust/crates/cua-perception/scripts/artifacts.lock.json",
                ], "patches": [],
            }]
        for entry in lock["corresponding_source"]:
            material = bundle / entry["bundle_path"]
            source_entries.append({
                "artifact": entry["filename"], "artifactSha256": sha256(material),
                "artifactSize": material.stat().st_size, "repository": entry["repository"],
                "revision": entry["revision"], "license": entry["license"],
                "durableLocation": f"Candidate archive {entry['bundle_path']}",
                "sourceOfferStatus": entry["source_offer_status"],
                "contentKind": entry["content_kind"], "format": entry["format"],
                **({"requiredPaths": entry["required_paths"]} if entry.get("required_paths") else {}),
                "patches": [],
            })
        source_ledger = {"schemaVersion": 1, "sources": source_entries}
        write_json(bundle / "source-ledger.json", source_ledger)
        write_json(bundle / "source-inventory.json", {
            "schema_version": 1, "component": "cua-perception", "repository": "https://github.com/trycua/cua",
            "revision": args.source_sha,
            "archive": {"path": source.relative_to(bundle).as_posix(), "sha256": sha256(source), "size": source.stat().st_size},
            "included_paths": list(SOURCE_PATHS), "source_offer": "SOURCE_OFFER.md",
            "bundled_upstream_sources": [
                {"component": entry["content_kind"], "path": entry["bundle_path"],
                 "url": entry["url"], "revision": entry["revision"],
                 "sha256": entry["sha256"], "size": entry["size"]}
                for entry in lock["corresponding_source"]
            ],
            "immutable_external_coordinates": [
                {"component": entry["role"], "url": entry["url"],
                 "revision": entry["revision"], "sha256": entry["sha256"],
                 "size": entry["size"]}
                for entry in lock["artifacts"] if entry.get("url")
            ] + [{
                "component": "onnx-runtime-cpu-archive", "url": target_lock["archive_url"],
                "sha256": target_lock["archive_sha256"],
            }],
            "review_status": "maintainer-approved-agpl-distribution",
        })
        fixture = bundle / "verification/known-answer.png"
        fixture.parent.mkdir()
        shutil.copyfile(args.real_parse_fixture, fixture)
        exercise_bundle(bundle, args.target, fixture, identity, fixture.parent)
        write_json(bundle / "artifact-manifest.json", release_manifest(bundle, lock, args.target, args.version, args.source_sha))
        write_sums(bundle)
        static_verify(bundle)
        archive = bundle.parent / f"cua-perception-{args.version}-{args.target}.tar.gz"
        if archive.exists():
            raise ArtifactError(f"refusing to overwrite existing archive: {archive}")
        deterministic_bundle_archive(bundle, archive)
        return bundle, archive
    except Exception:
        shutil.rmtree(bundle)
        raise


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=sorted(read_json(LOCK_PATH)["onnx_runtime"]["targets"]))
    parser.add_argument("--worker", required=True, type=Path)
    parser.add_argument("--inputs", required=True, type=Path, help="directory containing reviewed local artifacts")
    parser.add_argument("--cache", required=True, type=Path, help="content-verified immutable download cache")
    parser.add_argument("--real-parse-fixture", required=True, type=Path)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        bundle, archive = assemble(args)
        print(json.dumps({"bundle": str(bundle), "archive": str(archive), "sha256": sha256(archive)}, sort_keys=True))
        return 0
    except (ArtifactError, OSError, KeyError, ValueError, zipfile.BadZipFile) as error:
        parser.exit(1, f"assembly failed: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
