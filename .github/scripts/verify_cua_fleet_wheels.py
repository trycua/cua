#!/usr/bin/env python3
"""Verify canonical cua-fleet wheels without changing their bytes."""

from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import json
import re
import sys
import zipfile
from email.parser import BytesParser
from pathlib import Path


WHEEL_PATTERN = re.compile(
    r"^cua_fleet-(?P<version>[0-9]+\.[0-9]+\.[0-9]+)-py3-none-(?P<platform>.+)\.whl$"
)
PLATFORM_NATIVE_NAMES = {
    "manylinux_2_34_x86_64": "libcyclops_sdk.so",
    "manylinux_2_34_aarch64": "libcyclops_sdk.so",
    "macosx_10_12_x86_64": "libcyclops_sdk.dylib",
    "macosx_11_0_arm64": "libcyclops_sdk.dylib",
    "win_amd64": "cyclops_sdk.dll",
}


class WheelError(RuntimeError):
    pass


def sha256_record(data: bytes) -> tuple[str, str]:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return f"sha256={digest}", str(len(data))


def load_manifest(path: Path) -> tuple[str, dict[str, str]]:
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise WheelError(f"could not read canonical wheel manifest: {error}") from error
    version = data.get("version")
    wheels = data.get("wheels")
    if not isinstance(version, str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
        raise WheelError("manifest version must be X.Y.Z")
    if not isinstance(wheels, dict) or not wheels:
        raise WheelError("manifest wheels must be a non-empty filename-to-SHA-256 mapping")
    if any(not isinstance(name, str) or not re.fullmatch(r"[0-9a-f]{64}", digest or "") for name, digest in wheels.items()):
        raise WheelError("manifest wheel hashes must be lowercase SHA-256 values")
    return version, wheels


def verify_record(entries: dict[str, bytes], record_name: str) -> None:
    rows = list(csv.reader(entries[record_name].decode().splitlines()))
    recorded = {row[0]: row[1:] for row in rows}
    if len(recorded) != len(rows) or set(recorded) != set(entries):
        raise WheelError("RECORD entries do not match wheel members")
    for name, data in entries.items():
        digest, size = recorded[name]
        if name == record_name:
            if digest or size:
                raise WheelError("RECORD must have an empty self-hash")
        elif [digest, size] != list(sha256_record(data)):
            raise WheelError(f"RECORD does not match {name}")


def verify_wheel(path: Path, version: str, expected_sha256: str) -> str:
    match = WHEEL_PATTERN.fullmatch(path.name)
    if not match or match.group("version") != version:
        raise WheelError(f"wheel filename does not identify cua-fleet=={version}: {path.name}")
    platform = match.group("platform")
    native_name = PLATFORM_NATIVE_NAMES.get(platform)
    if native_name is None:
        raise WheelError(f"unsupported canonical wheel platform: {platform}")

    actual_sha256 = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual_sha256 != expected_sha256:
        raise WheelError(f"SHA-256 mismatch for {path.name}: {actual_sha256}")

    try:
        with zipfile.ZipFile(path) as archive:
            entries = {
                info.filename: archive.read(info.filename)
                for info in archive.infolist()
                if not info.is_dir()
            }
    except (OSError, zipfile.BadZipFile) as error:
        raise WheelError(f"could not read {path.name}: {error}") from error

    dist_info = f"cua_fleet-{version}.dist-info"
    expected_roots = {"fleet_sdk", dist_info}
    roots = {name.split("/", 1)[0] for name in entries}
    if roots != expected_roots:
        raise WheelError(f"unexpected payload roots in {path.name}: {sorted(roots)}")

    metadata_name = f"{dist_info}/METADATA"
    wheel_name = f"{dist_info}/WHEEL"
    record_name = f"{dist_info}/RECORD"
    required = {metadata_name, wheel_name, record_name, f"fleet_sdk/{native_name}"}
    if not required.issubset(entries):
        raise WheelError(f"wheel is missing required files: {sorted(required - set(entries))}")

    metadata = BytesParser().parsebytes(entries[metadata_name])
    if metadata.get("Name") != "cua-fleet" or metadata.get("Version") != version:
        raise WheelError("wheel METADATA does not match its filename")
    requires = metadata.get_all("Requires-Dist", [])
    if any("cua-train" in requirement.lower() for requirement in requires):
        raise WheelError("wheel METADATA depends on cua-train")

    wheel_metadata = entries[wheel_name].decode()
    if "Root-Is-Purelib: false" not in wheel_metadata or f"Tag: py3-none-{platform}" not in wheel_metadata:
        raise WheelError("wheel platform metadata does not match its filename")
    verify_record(entries, record_name)
    return platform


def verify_release(manifest_path: Path, dist_directory: Path, require_all_platforms: bool = True) -> None:
    version, wheels = load_manifest(manifest_path)
    actual_files = {path.name: path for path in dist_directory.glob("*.whl")}
    actual_names = set(actual_files)
    expected_names = set(wheels)
    valid_set = actual_names == expected_names if require_all_platforms else bool(actual_names) and actual_names <= expected_names
    if not valid_set:
        raise WheelError(
            f"wheel set does not match manifest: actual={sorted(actual_files)}, expected={sorted(wheels)}"
        )

    platforms = {
        verify_wheel(path, version, wheels[name])
        for name, path in actual_files.items()
    }
    expected_platforms = set(PLATFORM_NATIVE_NAMES)
    if require_all_platforms and platforms != expected_platforms:
        raise WheelError(
            f"canonical release platforms do not match: actual={sorted(platforms)}, expected={sorted(expected_platforms)}"
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--allow-partial", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        verify_release(args.manifest, args.dist, require_all_platforms=not args.allow_partial)
    except WheelError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"Verified canonical cua-fleet wheels in {args.dist}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
