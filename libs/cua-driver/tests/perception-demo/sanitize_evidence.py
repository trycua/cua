#!/usr/bin/env python3
"""Build review evidence only from measured runtime inputs."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


SHA40 = re.compile(r"^[0-9a-f]{40}$")
SHA64 = re.compile(r"^[0-9a-f]{64}$")
MODEL_ID = re.compile(r"^[A-Za-z0-9._/-]{1,96}$")


def sha256_file(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def require_file(path: Path, label: str, maximum: int) -> int:
    if not path.is_file():
        raise ValueError(f"{label} must be a regular file")
    size = path.stat().st_size
    if not 0 < size <= maximum:
        raise ValueError(f"{label} must be nonempty and at most {maximum} bytes")
    return size


def measure_source_sha(repo_root: Path, expected_source_sha: str) -> str:
    if not SHA40.fullmatch(expected_source_sha):
        raise ValueError("expected source SHA must be 40 lowercase hexadecimal characters")
    result = subprocess.run(
        ["git", "-C", str(repo_root), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    measured = result.stdout.strip()
    if measured != expected_source_sha:
        raise ValueError("checked-out source does not match the approved workflow candidate")
    return measured


def measure_platform(platform_name: str = sys.platform, environment: dict = os.environ) -> str:
    if platform_name == "win32":
        return "windows"
    if platform_name.startswith("linux") and environment.get("DISPLAY"):
        return "linux-x11"
    raise ValueError("evidence sanitizer requires Windows or an active Linux X11 session")


def verify_extension_signature(
    extension: Path,
    signature: Path,
    public_key: Path,
    expected_public_key_sha256: str,
) -> tuple[str, str]:
    require_file(extension, "signed extension", 100 * 1024 * 1024)
    require_file(signature, "extension signature", 64 * 1024)
    require_file(public_key, "trusted public key", 64 * 1024)
    measured_key_sha = sha256_file(public_key)
    if not SHA64.fullmatch(expected_public_key_sha256):
        raise ValueError("trusted public key digest must be 64 lowercase hexadecimal characters")
    if measured_key_sha != expected_public_key_sha256:
        raise ValueError("trusted public key bytes do not match the approved digest")
    measured_extension_sha = sha256_file(extension)
    result = subprocess.run(
        [
            "openssl",
            "dgst",
            "-sha256",
            "-verify",
            str(public_key),
            "-signature",
            str(signature),
            str(extension),
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 or "Verified OK" not in result.stdout:
        raise ValueError("signed extension bytes failed RSA-SHA256 verification")
    if (
        sha256_file(public_key) != measured_key_sha
        or sha256_file(extension) != measured_extension_sha
    ):
        raise ValueError("extension or signing key changed during signature verification")
    return measured_key_sha, measured_extension_sha


def load_oracle(path: Path) -> dict[str, object]:
    require_file(path, "fixture oracle", 64 * 1024)
    value = json.loads(path.read_text(encoding="utf-8"))
    expected = {"fixture", "ready", "selected", "action_count"}
    if not isinstance(value, dict) or set(value) != expected:
        raise ValueError(f"fixture oracle keys must be exactly {sorted(expected)}")
    if value["fixture"] != "visual-only-canvas/v1" or value["ready"] is not True:
        raise ValueError("fixture oracle is not a ready visual-only canvas")
    if value["selected"] not in {"save", "send", "cancel"}:
        raise ValueError("fixture oracle contains an unexpected selection")
    if type(value["action_count"]) is not int or not 1 <= value["action_count"] <= 8:
        raise ValueError("fixture oracle action_count must be an integer from 1 through 8")
    return value


def load_adapter_result(path: Path, source_sha: str) -> dict[str, str]:
    require_file(path, "adapter result", 64 * 1024)
    value = json.loads(path.read_text(encoding="utf-8"))
    expected = {"adapter", "status", "source_sha", "model_id"}
    if not isinstance(value, dict) or set(value) != expected:
        raise ValueError(f"adapter result keys must be exactly {sorted(expected)}")
    if value["adapter"] != "jev-use" or value["status"] != "passed":
        raise ValueError("adapter result is not a passing Jev use result")
    if value["source_sha"] != source_sha:
        raise ValueError("adapter result source does not match the measured checkout")
    if not isinstance(value["model_id"], str) or not MODEL_ID.fullmatch(value["model_id"]):
        raise ValueError("adapter result model_id contains unsafe characters")
    return value


def build_manifest(
    *,
    source_sha: str,
    platform: str,
    extension: Path,
    extension_signature: Path,
    trusted_public_key: Path,
    trusted_public_key_sha256: str,
    model: Path,
    adapter_result: Path,
    oracle: Path,
    recording: Path,
) -> dict:
    if not SHA40.fullmatch(source_sha):
        raise ValueError("source_sha must be 40 lowercase hexadecimal characters")
    if platform not in {"windows", "linux-x11"}:
        raise ValueError("platform must be windows or linux-x11")
    signing_key_sha, signed_extension_sha = verify_extension_signature(
        extension,
        extension_signature,
        trusted_public_key,
        trusted_public_key_sha256,
    )
    require_file(model, "model", 8 * 1024 * 1024 * 1024)
    video_size = require_file(recording, "recording.mp4", 100 * 1024 * 1024)
    adapter_value = load_adapter_result(adapter_result, source_sha)
    oracle_value = load_oracle(oracle)
    return {
        "schema": "cua-visual-perception-demo-evidence/v1",
        "source_sha": source_sha,
        "platform": platform,
        "fixture": {
            "id": oracle_value["fixture"],
            "oracle": {
                "selected": oracle_value["selected"],
                "action_count": oracle_value["action_count"],
            },
        },
        "runtime": {
            "adapter": adapter_value["adapter"],
            "model_id": adapter_value["model_id"],
            "model_sha256": sha256_file(model),
            "signed_extension_sha256": signed_extension_sha,
            "signing_key_sha256": signing_key_sha,
            "signature_algorithm": "rsa-sha256",
        },
        "result": {"status": adapter_value["status"]},
        "artifacts": [
            {
                "kind": "video",
                "path": "recording.mp4",
                "sha256": sha256_file(recording),
                "size_bytes": video_size,
            }
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--expected-source-sha", required=True)
    parser.add_argument("--extension", type=Path, required=True)
    parser.add_argument("--extension-signature", type=Path, required=True)
    parser.add_argument("--trusted-public-key", type=Path, required=True)
    parser.add_argument("--trusted-public-key-sha256", required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--adapter-result", type=Path, required=True)
    parser.add_argument("--oracle", type=Path, required=True)
    parser.add_argument("--recording", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    manifest = build_manifest(
        source_sha=measure_source_sha(args.repo_root, args.expected_source_sha),
        platform=measure_platform(),
        extension=args.extension,
        extension_signature=args.extension_signature,
        trusted_public_key=args.trusted_public_key,
        trusted_public_key_sha256=args.trusted_public_key_sha256,
        model=args.model,
        adapter_result=args.adapter_result,
        oracle=args.oracle,
        recording=args.recording,
    )
    if args.output_dir.exists() and any(args.output_dir.iterdir()):
        raise ValueError("output directory must be empty before evidence publication")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(args.recording, args.output_dir / "recording.mp4")
    (args.output_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
