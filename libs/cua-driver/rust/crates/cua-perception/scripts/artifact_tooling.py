#!/usr/bin/env python3
"""Shared, dependency-free helpers for sealed Cua Perception bundles."""

from __future__ import annotations

import hashlib
import json
import platform
import struct
import subprocess
import tarfile
from pathlib import Path
from pathlib import PurePosixPath
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
CRATE_DIR = SCRIPT_DIR.parent
LOCK_PATH = SCRIPT_DIR / "artifacts.lock.json"
PROTOCOL = "cua-perception/1"


class ArtifactError(RuntimeError):
    pass


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ArtifactError(f"cannot read JSON {path}: {error}") from error
    if not isinstance(value, dict):
        raise ArtifactError(f"expected a JSON object in {path}")
    return value


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_file(path: Path, expected_hash: str, expected_size: int | None = None) -> None:
    if path.is_symlink() or not path.is_file():
        raise ArtifactError(f"missing regular artifact: {path}")
    if expected_size is not None and path.stat().st_size != expected_size:
        raise ArtifactError(
            f"size mismatch for {path}: expected {expected_size}, got {path.stat().st_size}"
        )
    actual = sha256(path)
    if actual != expected_hash:
        raise ArtifactError(f"SHA-256 mismatch for {path}: expected {expected_hash}, got {actual}")


def host_target() -> str:
    os_name = platform.system().lower()
    machine = platform.machine().lower()
    if os_name == "darwin" and machine in {"arm64", "aarch64"}:
        return "aarch64-apple-darwin"
    if os_name == "windows" and machine in {"amd64", "x86_64"}:
        return "x86_64-pc-windows-msvc"
    if os_name == "linux" and machine in {"amd64", "x86_64"}:
        return "x86_64-unknown-linux-gnu"
    raise ArtifactError(f"unsupported verification host: {platform.system()} {platform.machine()}")


def binary_target(path: Path) -> str:
    with path.open("rb") as stream:
        data = stream.read(4096)
    if len(data) >= 20 and data[:4] == b"\x7fELF":
        byte_order = "little" if data[5] == 1 else "big"
        machine = int.from_bytes(data[18:20], byte_order)
        if data[4] == 2 and machine == 62:
            return "x86_64-unknown-linux-gnu"
    if len(data) >= 64 and data[:2] == b"MZ":
        offset = int.from_bytes(data[0x3C:0x40], "little")
        if offset + 6 <= len(data) and data[offset : offset + 4] == b"PE\0\0":
            if int.from_bytes(data[offset + 4 : offset + 6], "little") == 0x8664:
                return "x86_64-pc-windows-msvc"
    if len(data) >= 8 and data[:4] in {b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf"}:
        byte_order = "little" if data[:4] == b"\xcf\xfa\xed\xfe" else "big"
        if int.from_bytes(data[4:8], byte_order) == 0x0100000C:
            return "aarch64-apple-darwin"
    raise ArtifactError(f"unsupported or unrecognized executable format: {path}")


def require_binary_target(path: Path, expected: str) -> None:
    actual = binary_target(path)
    if actual != expected:
        raise ArtifactError(f"wrong-platform binary {path}: expected {expected}, got {actual}")


def confined_file(root: Path, relative: str) -> Path:
    candidate = Path(relative)
    if candidate.is_absolute() or not candidate.parts or ".." in candidate.parts:
        raise ArtifactError(f"unsafe bundle path: {relative}")
    resolved_root = root.resolve()
    resolved = (root / candidate).resolve()
    if resolved.parent != resolved_root and resolved_root not in resolved.parents:
        raise ArtifactError(f"bundle path escapes root: {relative}")
    if (root / candidate).is_symlink() or not resolved.is_file():
        raise ArtifactError(f"bundle path is not a regular file: {relative}")
    return resolved


def read_model_paths(bundle: Path, target: str) -> tuple[Path, Path, list[Path]]:
    model_manifest = read_json(bundle / "model-manifest.json")
    target_lock = read_json(LOCK_PATH)["onnx_runtime"]["targets"].get(target)
    if not target_lock:
        raise ArtifactError(f"unsupported bundle target: {target}")
    runtime_name = target_lock["filename"]
    runtime = confined_file(bundle, runtime_name)
    artifacts = [
        model_manifest["detector"]["model"],
        model_manifest["ocr"]["detector"]["model"],
        model_manifest["ocr"]["recognizer"]["model"],
        model_manifest["ocr"]["dictionary"],
    ]
    paths: list[Path] = []
    for artifact in artifacts:
        path = confined_file(bundle, artifact["path"])
        verify_file(path, artifact["sha256"])
        paths.append(path)
    verify_file(runtime, model_manifest["onnx_runtime"]["library_sha256"])
    return bundle / "model-manifest.json", runtime, paths


def worker_request(worker: Path, manifest: Path, runtime: Path, request: dict[str, Any]) -> dict[str, Any]:
    payload = json.dumps(request, separators=(",", ":")).encode()
    framed = struct.pack(">I", len(payload)) + payload
    process = subprocess.run(
        [str(worker), "--manifest", str(manifest), "--onnx-runtime-library", str(runtime)],
        input=framed,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=180,
        check=False,
    )
    if process.returncode != 0:
        raise ArtifactError(
            f"worker exited {process.returncode}: {process.stderr.decode(errors='replace').strip()}"
        )
    if len(process.stdout) < 4:
        raise ArtifactError("worker returned a truncated response frame")
    size = struct.unpack(">I", process.stdout[:4])[0]
    if len(process.stdout) != size + 4:
        raise ArtifactError("worker response frame has the wrong length")
    try:
        response = json.loads(process.stdout[4:])
    except json.JSONDecodeError as error:
        raise ArtifactError(f"worker returned invalid JSON: {error}") from error
    if response.get("status") != "ok":
        raise ArtifactError(f"worker request failed: {response}")
    return response


def request(method: str, params: dict[str, Any], request_id: str) -> dict[str, Any]:
    return {"protocol": PROTOCOL, "request_id": request_id, "method": method, "params": params}


def _verify_source_archive(path: Path, required_paths: list[str]) -> None:
    try:
        with tarfile.open(path, "r:gz") as archive:
            members = archive.getmembers()
    except tarfile.TarError as error:
        raise ArtifactError(f"invalid corresponding-source archive {path}: {error}") from error
    if not members:
        raise ArtifactError(f"empty corresponding-source archive: {path}")
    safe_names = []
    for member in members:
        pure = PurePosixPath(member.name)
        if pure.is_absolute() or ".." in pure.parts:
            raise ArtifactError(f"unsafe corresponding-source member in {path}: {member.name}")
        if member.issym() or member.islnk():
            target = PurePosixPath(member.linkname)
            parts = list(pure.parent.parts)
            if target.is_absolute():
                raise ArtifactError(f"unsafe corresponding-source link in {path}: {member.name}")
            for part in target.parts:
                if part in {"", "."}:
                    continue
                if part == "..":
                    if len(parts) <= 1:
                        raise ArtifactError(f"corresponding-source link escapes archive root: {member.name}")
                    parts.pop()
                else:
                    parts.append(part)
        safe_names.append(pure.as_posix().rstrip("/"))
    for required in required_paths:
        wanted = required.rstrip("/")
        if not any(name == wanted or name.endswith("/" + wanted) or f"/{wanted}/" in f"/{name}/" for name in safe_names):
            raise ArtifactError(f"corresponding-source archive {path} lacks required path: {required}")


def verify_corresponding_sources(bundle: Path, manifest: dict[str, Any], lock: dict[str, Any]) -> list[dict[str, Any]]:
    source_artifacts = {
        item["name"]: item for item in manifest.get("artifacts", []) if item.get("kind") == "source"
    }
    if not source_artifacts:
        raise ArtifactError("bundle does not declare corresponding-source artifacts")
    ledger = read_json(confined_file(bundle, str(manifest.get("sourceLedger", ""))))
    entries = ledger.get("sources")
    if not isinstance(entries, list) or not entries:
        raise ArtifactError("source ledger contains no bundled source entries")
    ledger_entries = {entry.get("artifact"): entry for entry in entries if isinstance(entry, dict)}
    if len(ledger_entries) != len(entries) or set(ledger_entries) != set(source_artifacts):
        raise ArtifactError("source ledger entries differ from declared source artifacts")
    for name, artifact in source_artifacts.items():
        entry = ledger_entries[name]
        path = confined_file(bundle, artifact["path"])
        verify_file(path, artifact["sha256"], artifact["size"])
        if entry.get("artifactSha256") != artifact["sha256"] or entry.get("artifactSize") != artifact["size"]:
            raise ArtifactError(f"source ledger hash or size differs for {name}")
        if entry.get("license") != artifact.get("license", {}).get("spdx"):
            raise ArtifactError(f"source ledger license differs for {name}")
        if entry.get("sourceOfferStatus") not in {"bundled", "bundled-review-only"}:
            raise ArtifactError(f"source ledger does not identify a bundled payload for {name}")
        if entry.get("format") == "tar.gz":
            _verify_source_archive(path, entry.get("requiredPaths", []))
        elif entry.get("format") != "file":
            raise ArtifactError(f"source ledger has an unsupported format for {name}")
    expected = {entry["filename"]: entry for entry in lock.get("corresponding_source", [])}
    for name, pinned in expected.items():
        artifact = source_artifacts.get(name)
        entry = ledger_entries.get(name)
        if not artifact or not entry:
            raise ArtifactError(f"bundle is missing pinned source material: {name}")
        if artifact["path"] != pinned["bundle_path"]:
            raise ArtifactError(f"pinned source material has the wrong path: {name}")
        if artifact["sha256"] != pinned["sha256"] or artifact["size"] != pinned["size"]:
            raise ArtifactError(f"pinned source material differs from the artifact lock: {name}")
        if entry.get("revision") != pinned["revision"] or entry.get("contentKind") != pinned["content_kind"]:
            raise ArtifactError(f"source ledger revision or kind differs from the artifact lock: {name}")
    model_source = ledger_entries.get("omniparser-icon-detect-model.pt")
    if not model_source or model_source.get("contentKind") != "model-source-input":
        raise ArtifactError("exact OmniParser source model is not bundled")
    return entries


def static_verify(bundle: Path, require_host: bool = True) -> dict[str, Any]:
    manifest = read_json(bundle / "artifact-manifest.json")
    target = manifest.get("target", {}).get("triple")
    lock = read_json(LOCK_PATH)
    if target not in lock["onnx_runtime"]["targets"]:
        raise ArtifactError(f"unsupported bundle target: {target}")
    if require_host and target != host_target():
        raise ArtifactError(f"wrong-platform bundle: host is {host_target()}, bundle is {target}")
    seen: set[str] = set()
    for artifact in manifest.get("artifacts", []):
        relative = artifact.get("path")
        if not isinstance(relative, str) or relative in seen:
            raise ArtifactError("artifact manifest contains a missing or duplicate path")
        seen.add(relative)
        verify_file(confined_file(bundle, relative), artifact["sha256"], artifact["size"])
    worker_entries = [item for item in manifest["artifacts"] if item["kind"] == "worker"]
    runtime_entries = [item for item in manifest["artifacts"] if item["kind"] == "runtime"]
    if len(worker_entries) != 1 or len(runtime_entries) != 1:
        raise ArtifactError("bundle must contain exactly one declared worker and runtime")
    if len(list(bundle.rglob("model-manifest.json"))) != 1:
        raise ArtifactError("bundle must contain one unique model-manifest.json at its root")
    require_binary_target(confined_file(bundle, worker_entries[0]["path"]), target)
    require_binary_target(confined_file(bundle, runtime_entries[0]["path"]), target)
    target_lock = lock["onnx_runtime"]["targets"][target]
    runtime_path = confined_file(bundle, runtime_entries[0]["path"])
    if runtime_path.name != target_lock["filename"]:
        raise ArtifactError("runtime filename does not match the pinned target runtime")
    verify_file(runtime_path, target_lock["sha256"], target_lock["size"])
    _, runtime, _ = read_model_paths(bundle, target)
    if sha256(runtime) != runtime_entries[0]["sha256"]:
        raise ArtifactError("runtime hash differs between model and artifact manifests")
    expected_roles = {entry["role"]: entry for entry in lock["artifacts"]}
    declared_roles = {
        item.get("role"): item
        for item in manifest["artifacts"]
        if item.get("kind") in {"model", "dictionary"}
    }
    if set(declared_roles) != set(expected_roles):
        raise ArtifactError("bundle does not declare exactly the pinned model and dictionary roles")
    for role, expected in expected_roles.items():
        declared = declared_roles[role]
        if declared["name"] != expected["filename"] or declared["sha256"] != expected["sha256"]:
            raise ArtifactError(f"{role} differs from the pinned artifact lock")
    if manifest.get("sourceLedger"):
        verify_corresponding_sources(bundle, manifest, lock)
    sums_path = bundle / "SHA256SUMS"
    if sums_path.exists():
        for line in sums_path.read_text(encoding="utf-8").splitlines():
            digest, marker, relative = line.partition("  ")
            if marker != "  " or len(digest) != 64:
                raise ArtifactError("invalid SHA256SUMS line")
            verify_file(confined_file(bundle, relative), digest)
    return manifest
