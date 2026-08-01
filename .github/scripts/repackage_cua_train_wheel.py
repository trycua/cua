#!/usr/bin/env python3
"""Repackage a verified native cua-train wheel as a cua-fleet wheel."""

from __future__ import annotations

import argparse
import base64
import csv
import hashlib
from io import StringIO
from pathlib import Path
import re
import sys
import zipfile

SOURCE_NAME = "cua-train"
SOURCE_VERSION = "0.1.2"
DESTINATION_NAME = "cua-fleet"
WHEEL_FILENAME = re.compile(
    r"^(?P<distribution>[A-Za-z0-9_]+)-(?P<version>[^-]+)-py3-none-(?P<platform>[^-]+)\.whl$"
)


class WheelError(ValueError):
    """Raised when a wheel does not satisfy the promotion contract."""


def wheel_stem(name: str) -> str:
    return name.replace("-", "_")


def sha256_record(data: bytes) -> tuple[str, str]:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return f"sha256={digest}", str(len(data))


def parse_filename(path: Path) -> tuple[str, str, str]:
    match = WHEEL_FILENAME.fullmatch(path.name)
    if match is None:
        raise WheelError(f"unsupported wheel filename: {path.name}")
    return match.group("distribution"), match.group("version"), match.group("platform")


def parse_metadata(data: bytes) -> dict[str, str]:
    fields: dict[str, str] = {}
    for line in data.decode().splitlines():
        if not line:
            break
        if ": " in line:
            key, value = line.split(": ", 1)
            fields.setdefault(key, value)
    return fields


def update_metadata(data: bytes, version: str) -> bytes:
    text = data.decode()
    _, _, description = text.partition("\n\n")
    lines = text.splitlines(keepends=True)
    found_name = found_version = False
    updated: list[str] = []
    for line in lines:
        if not description.strip() and line.startswith("Description-Content-Type: "):
            updated.append("Description-Content-Type: text/plain\n")
            continue
        if line.startswith("Name: "):
            updated.append(f"Name: {DESTINATION_NAME}\n")
            found_name = True
        elif line.startswith("Version: "):
            updated.append(f"Version: {version}\n")
            found_version = True
        else:
            updated.append(line)
    if not found_name or not found_version:
        raise WheelError("METADATA must contain Name and Version fields")
    return "".join(updated).encode()


def csv_bytes(rows: list[list[str]]) -> bytes:
    output = StringIO()
    csv.writer(output, lineterminator="\n").writerows(rows)
    return output.getvalue().encode()


def source_payloads(entries: dict[str, bytes]) -> dict[str, str]:
    return {
        name: hashlib.sha256(data).hexdigest()
        for name, data in entries.items()
        if ".dist-info/" not in name
    }


def repack(
    source: Path, destination_version: str, output_directory: Path, expected_sha256: str | None
) -> Path:
    if expected_sha256 is not None:
        actual_sha256 = hashlib.sha256(source.read_bytes()).hexdigest()
        if actual_sha256 != expected_sha256:
            raise WheelError(f"source SHA-256 mismatch for {source.name}: {actual_sha256}")

    distribution, version, platform = parse_filename(source)
    if distribution != wheel_stem(SOURCE_NAME) or version != SOURCE_VERSION:
        raise WheelError(
            f"expected {SOURCE_NAME}=={SOURCE_VERSION}, found {distribution}=={version}"
        )

    with zipfile.ZipFile(source) as archive:
        entries = {
            info.filename: archive.read(info.filename)
            for info in archive.infolist()
            if not info.is_dir() and not info.filename.endswith(".dist-info/RECORD")
        }

    source_dist_info = f"{wheel_stem(SOURCE_NAME)}-{SOURCE_VERSION}.dist-info"
    destination_dist_info = f"{wheel_stem(DESTINATION_NAME)}-{destination_version}.dist-info"
    metadata_name = f"{source_dist_info}/METADATA"
    wheel_name = f"{source_dist_info}/WHEEL"
    if metadata_name not in entries or wheel_name not in entries:
        raise WheelError("wheel is missing METADATA or WHEEL")

    metadata = parse_metadata(entries[metadata_name])
    if metadata.get("Name") != SOURCE_NAME or metadata.get("Version") != SOURCE_VERSION:
        raise WheelError("source METADATA does not identify cua-train==0.1.2")
    wheel_metadata = entries[wheel_name].decode()
    if (
        "Root-Is-Purelib: false" not in wheel_metadata
        or f"Tag: py3-none-{platform}" not in wheel_metadata
    ):
        raise WheelError("source WHEEL metadata does not match its native platform tag")

    transformed: dict[str, bytes] = {}
    for name, data in entries.items():
        destination_name = name.replace(source_dist_info, destination_dist_info, 1)
        transformed[destination_name] = (
            update_metadata(data, destination_version) if name == metadata_name else data
        )

    record_name = f"{destination_dist_info}/RECORD"
    records = []
    for name in sorted(transformed):
        digest, size = sha256_record(transformed[name])
        records.append([name, digest, size])
    records.append([record_name, "", ""])
    transformed[record_name] = csv_bytes(records)

    output_directory.mkdir(parents=True, exist_ok=True)
    destination = (
        output_directory
        / f"{wheel_stem(DESTINATION_NAME)}-{destination_version}-py3-none-{platform}.whl"
    )
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(transformed):
            archive.writestr(name, transformed[name])

    verify(destination, source)
    return destination


def verify(path: Path, source: Path | None = None) -> None:
    distribution, version, platform = parse_filename(path)
    if distribution != wheel_stem(DESTINATION_NAME):
        raise WheelError(f"wheel filename does not identify {DESTINATION_NAME}: {path.name}")

    destination_dist_info = f"{wheel_stem(DESTINATION_NAME)}-{version}.dist-info"
    with zipfile.ZipFile(path) as archive:
        entries = {
            info.filename: archive.read(info.filename)
            for info in archive.infolist()
            if not info.is_dir()
        }

    metadata_name = f"{destination_dist_info}/METADATA"
    wheel_name = f"{destination_dist_info}/WHEEL"
    record_name = f"{destination_dist_info}/RECORD"
    if not {metadata_name, wheel_name, record_name}.issubset(entries):
        raise WheelError("destination wheel is missing dist-info metadata")

    metadata = parse_metadata(entries[metadata_name])
    if metadata.get("Name") != DESTINATION_NAME or metadata.get("Version") != version:
        raise WheelError("destination METADATA does not match the wheel filename")
    wheel_metadata = entries[wheel_name].decode()
    if (
        "Root-Is-Purelib: false" not in wheel_metadata
        or f"Tag: py3-none-{platform}" not in wheel_metadata
    ):
        raise WheelError("destination WHEEL metadata does not match the wheel filename")
    if not any(name.startswith("cua_train/") for name in entries):
        raise WheelError("destination wheel is missing cua_train")
    if not any(name.startswith("cyclops_sdk/") for name in entries):
        raise WheelError("destination wheel is missing cyclops_sdk")
    if not any(name.startswith("cyclops_sdk/libcyclops_sdk.") for name in entries):
        raise WheelError("destination wheel is missing the native cyclops library")

    record_rows = list(csv.reader(entries[record_name].decode().splitlines()))
    recorded = {row[0]: row[1:] for row in record_rows}
    if len(recorded) != len(record_rows):
        raise WheelError("RECORD contains duplicate entries")
    if set(recorded) != set(entries):
        raise WheelError("RECORD entries do not match wheel members")
    for name, data in entries.items():
        digest, size = recorded[name]
        if name == record_name:
            if digest or size:
                raise WheelError("RECORD must have empty hash and size")
        elif [digest, size] != list(sha256_record(data)):
            raise WheelError(f"RECORD does not match {name}")

    if source is not None:
        with zipfile.ZipFile(source) as archive:
            source_entries = {
                info.filename: archive.read(info.filename)
                for info in archive.infolist()
                if not info.is_dir() and ".dist-info/" not in info.filename
            }
        destination_entries = {
            name: data for name, data in entries.items() if ".dist-info/" not in name
        }
        if source_payloads(source_entries) != source_payloads(destination_entries):
            raise WheelError("non-dist-info payload bytes changed during repackaging")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path)
    parser.add_argument("--destination-version")
    parser.add_argument("--outdir", type=Path)
    parser.add_argument("--expected-sha256")
    parser.add_argument("--verify", type=Path, nargs="+")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.verify is not None:
            for path in args.verify:
                verify(path)
                print(f"Verified {path}")
            return 0
        if args.source is None or args.destination_version is None or args.outdir is None:
            raise WheelError(
                "--source, --destination-version, and --outdir are required when repacking"
            )
        destination = repack(
            args.source, args.destination_version, args.outdir, args.expected_sha256
        )
    except (WheelError, OSError, zipfile.BadZipFile) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(destination)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
