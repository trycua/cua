#!/usr/bin/env python3
"""Verify a sealed Cua Perception installation and exercise its real backend."""

from __future__ import annotations

import argparse
import base64
import json
import shutil
import struct
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from artifact_tooling import (
    ArtifactError,
    LOCK_PATH,
    host_target,
    read_json,
    read_model_paths,
    request,
    require_binary_target,
    sha256,
    static_verify,
    worker_request,
    write_json,
)


def image_dimensions(path: Path) -> tuple[int, int]:
    data = path.read_bytes()
    if len(data) < 24 or data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise ArtifactError("real-parse fixture must be a non-empty PNG with an IHDR")
    return struct.unpack(">II", data[16:24])


def source_inspection(bundle: Path, manifest: dict[str, Any]) -> list[dict[str, Any]]:
    ledger = read_json(bundle / manifest["sourceLedger"])
    artifacts = {
        item["name"]: item for item in manifest["artifacts"] if item["kind"] == "source"
    }
    return [
        {
            "kind": entry["contentKind"],
            "location": artifacts[entry["artifact"]]["path"],
            "revision": entry["revision"],
            "sha256": entry["artifactSha256"],
            "status": entry["sourceOfferStatus"],
        }
        for entry in ledger["sources"]
    ]


def mismatch_rejection(worker: Path, manifest: Path, runtime: Path, target: str) -> None:
    with tempfile.TemporaryDirectory(prefix="cua-perception-mismatch-") as temporary:
        tampered = Path(temporary) / runtime.name
        shutil.copyfile(runtime, tampered)
        with tampered.open("ab") as stream:
            stream.write(b"tampered")
        process = subprocess.run(
            [str(worker), "--manifest", str(manifest), "--onnx-runtime-library", str(tampered)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=30,
            check=False,
        )
        if process.returncode == 0 or b"sha256 mismatch" not in process.stderr.lower():
            raise ArtifactError("worker did not reject a runtime with the wrong hash")

        wrong = Path(temporary) / "wrong-platform"
        if target == "x86_64-unknown-linux-gnu":
            wrong.write_bytes(b"MZ" + b"\0" * 58 + (64).to_bytes(4, "little") + b"PE\0\0\x64\x86")
        else:
            header = bytearray(64)
            header[:4] = b"\x7fELF"
            header[4] = 2
            header[5] = 1
            header[18:20] = (62).to_bytes(2, "little")
            wrong.write_bytes(header)
        try:
            require_binary_target(wrong, target)
        except ArtifactError as error:
            if "wrong-platform" not in str(error):
                raise
        else:
            raise ArtifactError("platform gate accepted a binary for another target")


def exercise_bundle(
    bundle: Path, target: str, fixture: Path, report_dir: Path | None = None
) -> dict[str, Any]:
    lock = read_json(LOCK_PATH)
    target_lock = lock["onnx_runtime"]["targets"].get(target)
    if not target_lock:
        raise ArtifactError(f"unsupported target: {target}")
    if target != host_target():
        raise ArtifactError(f"wrong-platform bundle: host is {host_target()}, bundle is {target}")
    worker = bundle / target_lock["worker_filename"]
    manifest, runtime, _ = read_model_paths(bundle, target)
    require_binary_target(worker, target)
    require_binary_target(runtime, target)
    worker_hash = sha256(worker)
    runtime_hash = sha256(runtime)

    health = worker_request(worker, manifest, runtime, request("health", {}, "installed-health"))
    result = health.get("result", {})
    if (
        result.get("ready") is not True
        or result.get("runtime") != "onnx_runtime_cpu"
        or result.get("identity", {}).get("backend") != "onnx_runtime_cpu"
    ):
        raise ArtifactError(f"health returned the wrong backend: {health}")

    self_test = worker_request(
        worker, manifest, runtime, request("self_test", {}, "installed-self-test")
    )
    if self_test.get("result", {}).get("passed") is not True:
        raise ArtifactError(f"self_test did not pass: {self_test}")
    mismatch_rejection(worker, manifest, runtime, target)

    width, height = image_dimensions(fixture)
    fixture_bytes = fixture.read_bytes()
    parse = worker_request(
        worker,
        manifest,
        runtime,
        request(
            "parse",
            {
                "capture_id": "installed-real-parse",
                "image": {
                    "media_type": "image/png",
                    "width": width,
                    "height": height,
                    "byte_length": len(fixture_bytes),
                    "data_base64": base64.b64encode(fixture_bytes).decode("ascii"),
                },
            },
            "installed-real-parse",
        ),
    )
    parse_result = parse.get("result", {})
    regions = parse_result.get("regions")
    if parse_result.get("runtime") != "onnx_runtime_cpu" or not isinstance(regions, list) or not regions:
        raise ArtifactError(f"real parse returned no observations: {parse}")

    reports = {
        "health": {
            "schemaVersion": 1,
            "gate": "health",
            "status": "passed",
            "target": target,
            "protocolVersion": 1,
            "workerSha256": worker_hash,
            "runtimeSha256": runtime_hash,
        },
        "self-test": {
            "schemaVersion": 1,
            "gate": "self-test",
            "status": "passed",
            "target": target,
            "protocolVersion": 1,
            "workerSha256": worker_hash,
            "runtimeSha256": runtime_hash,
            "mismatchRejectionPassed": True,
        },
        "real-parse": {
            "schemaVersion": 1,
            "gate": "real-parse",
            "status": "passed",
            "target": target,
            "protocolVersion": 1,
            "workerSha256": worker_hash,
            "runtimeSha256": runtime_hash,
            "fixtureSha256": sha256(fixture),
            "observations": len(regions),
        },
    }
    if report_dir is not None:
        for name, report in reports.items():
            write_json(report_dir / f"{name}.json", report)
    return reports


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument(
        "--real-parse-fixture",
        type=Path,
        help="PNG fixture; defaults to verification/known-answer.png in the bundle",
    )
    parser.add_argument("--static-only", action="store_true")
    args = parser.parse_args()
    try:
        manifest = static_verify(args.bundle, require_host=not args.static_only)
        sources = source_inspection(args.bundle, manifest)
        if args.static_only:
            print(json.dumps({"status": "passed", "target": manifest["target"]["triple"], "sources": sources}, sort_keys=True))
            return 0
        fixture = args.real_parse_fixture or args.bundle / "verification/known-answer.png"
        reports = exercise_bundle(args.bundle, manifest["target"]["triple"], fixture)
        print(json.dumps({"status": "passed", "reports": reports, "sources": sources}, sort_keys=True))
        return 0
    except (ArtifactError, OSError, KeyError, subprocess.SubprocessError) as error:
        parser.exit(1, f"verification failed: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
