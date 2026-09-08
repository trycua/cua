#!/usr/bin/env python3
"""Build a platform wheel containing only the generated Fleet UniFFI binding."""

from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import io
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

DISTRIBUTION_NAME = "cua-fleet"
DISTRIBUTION_STEM = "cua_fleet"
PACKAGE_NAME = "fleet_sdk"
VERSION_PATTERN = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--outdir", type=Path, default=Path("dist"))
    parser.add_argument("--native-library", type=Path)
    parser.add_argument("--platform-tag")
    parser.add_argument("--cargo-target")
    return parser.parse_args()


def repository_root() -> Path:
    return Path(__file__).resolve().parents[2]


def native_library_name() -> str:
    if sys.platform == "darwin":
        return "libcyclops_sdk.dylib"
    if sys.platform.startswith("linux"):
        return "libcyclops_sdk.so"
    if sys.platform.startswith("win"):
        return "cyclops_sdk.dll"
    raise RuntimeError(f"unsupported wheel build platform: {sys.platform}")


def default_platform_tag() -> str:
    machine = platform.machine().lower()
    aliases = {"amd64": "x86_64", "aarch64": "arm64"}
    machine = aliases.get(machine, machine)

    if sys.platform.startswith("linux"):
        linux_arches = {"x86_64": "x86_64", "arm64": "aarch64"}
        try:
            return f"linux_{linux_arches[machine]}"
        except KeyError as error:
            raise RuntimeError(f"unsupported Linux architecture: {machine}") from error

    if sys.platform == "darwin":
        macos_versions = {"x86_64": "10_13", "arm64": "11_0"}
        try:
            return f"macosx_{macos_versions[machine]}_{machine}"
        except KeyError as error:
            raise RuntimeError(f"unsupported macOS architecture: {machine}") from error

    if sys.platform.startswith("win"):
        if machine != "x86_64":
            raise RuntimeError(f"unsupported Windows architecture: {machine}")
        return "win_amd64"

    raise RuntimeError(f"unsupported wheel build platform: {sys.platform}")


def native_library_name_for_platform_tag(platform_tag: str) -> str:
    if platform_tag == "win_amd64":
        return "cyclops_sdk.dll"
    if platform_tag.startswith("linux_") or platform_tag.startswith("manylinux_"):
        return "libcyclops_sdk.so"
    if platform_tag.startswith("macosx_"):
        return "libcyclops_sdk.dylib"
    raise RuntimeError(f"unsupported wheel platform tag: {platform_tag}")


def build_native_library(repo_root: Path, cargo_target: str | None, expected_name: str) -> Path:
    command = [
        "cargo",
        "build",
        "--locked",
        "--manifest-path",
        str(repo_root / "cyclops-cs" / "Cargo.toml"),
        "--package",
        "cyclops-sdk",
        "--release",
    ]
    if cargo_target:
        command.extend(["--target", cargo_target])
    subprocess.run(command, check=True)

    target_directory = repo_root / "cyclops-cs" / "target"
    if cargo_target:
        target_directory /= cargo_target
    library = target_directory / "release" / expected_name
    if not library.is_file():
        raise RuntimeError(f"Cargo did not produce expected native library: {library}")
    return library


def sha256_record(data: bytes) -> tuple[str, str]:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return f"sha256={digest}", str(len(data))


def csv_bytes(rows: list[list[str]]) -> bytes:
    output = io.StringIO()
    csv.writer(output, lineterminator="\n").writerows(rows)
    return output.getvalue().encode()


def package_entries(repo_root: Path, native_library: Path, expected_name: str) -> dict[str, bytes]:
    source = repo_root / "cyclops-cs" / "sdk-bindings" / "python" / PACKAGE_NAME
    if not source.is_dir():
        raise RuntimeError(f"generated Python binding is missing: {source}")

    entries: dict[str, bytes] = {}
    for path in sorted(source.rglob("*")):
        if not path.is_file() or "__pycache__" in path.parts or path.suffix in {".so", ".dylib", ".dll"}:
            continue
        archive_name = f"{PACKAGE_NAME}/{path.relative_to(source).as_posix()}"
        entries[archive_name] = path.read_bytes()
    entries[f"{PACKAGE_NAME}/{expected_name}"] = native_library.read_bytes()
    return entries


def build_wheel(
    repo_root: Path,
    version: str,
    output_directory: Path,
    native_library: Path | None,
    platform_tag: str | None,
    cargo_target: str | None,
) -> Path:
    if not VERSION_PATTERN.fullmatch(version):
        raise RuntimeError(f"cua-fleet version must be X.Y.Z, got: {version}")

    tag = platform_tag or default_platform_tag()
    expected_name = native_library_name_for_platform_tag(tag)
    library = native_library or build_native_library(repo_root, cargo_target, expected_name)
    library = library.resolve()
    if not library.is_file():
        raise RuntimeError(f"native library does not exist: {library}")
    if library.name != expected_name:
        raise RuntimeError(f"native library must be named {expected_name}, got {library.name}")

    entries = package_entries(repo_root, library, expected_name)
    dist_info = f"{DISTRIBUTION_STEM}-{version}.dist-info"
    entries[f"{dist_info}/METADATA"] = (
        "Metadata-Version: 2.4\n"
        f"Name: {DISTRIBUTION_NAME}\n"
        f"Version: {version}\n"
        "Summary: Python bindings for the Cua Fleet SDK\n"
        "Requires-Python: >=3.10\n"
        "License-Expression: MIT\n"
        "\n"
    ).encode()
    entries[f"{dist_info}/WHEEL"] = (
        "Wheel-Version: 1.0\n"
        "Generator: build-python-sdk-wheel.py\n"
        "Root-Is-Purelib: false\n"
        f"Tag: py3-none-{tag}\n"
        "\n"
    ).encode()

    allowed_roots = {PACKAGE_NAME, dist_info}
    roots = {name.split("/", 1)[0] for name in entries}
    if roots != allowed_roots:
        raise RuntimeError(f"unexpected wheel payload roots: {sorted(roots)}")

    record_name = f"{dist_info}/RECORD"
    record_rows = []
    for name in sorted(entries):
        digest, size = sha256_record(entries[name])
        record_rows.append([name, digest, size])
    record_rows.append([record_name, "", ""])
    entries[record_name] = csv_bytes(record_rows)

    output_directory = output_directory.resolve()
    output_directory.mkdir(parents=True, exist_ok=True)
    wheel = output_directory / f"{DISTRIBUTION_STEM}-{version}-py3-none-{tag}.whl"
    with tempfile.NamedTemporaryFile(dir=output_directory, suffix=".whl", delete=False) as temporary:
        temporary_path = Path(temporary.name)
    try:
        with zipfile.ZipFile(temporary_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name in sorted(entries):
                archive.writestr(name, entries[name])
        shutil.move(temporary_path, wheel)
    finally:
        temporary_path.unlink(missing_ok=True)
    return wheel


def main() -> int:
    args = parse_args()
    try:
        wheel = build_wheel(
            repository_root(),
            args.version,
            args.outdir,
            args.native_library,
            args.platform_tag,
            args.cargo_target,
        )
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(wheel)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
