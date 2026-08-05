#!/usr/bin/env python3
"""Build a platform wheel containing the generated Fleet Python binding."""

from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import platform
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

PACKAGE_NAME = "cua_train"
NATIVE_PACKAGE = "fleet_sdk"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--outdir", type=Path, default=Path("dist"))
    parser.add_argument("--native-library", type=Path)
    parser.add_argument("--platform-tag")
    parser.add_argument("--cargo-target")
    return parser.parse_args()


def repository_root() -> Path:
    return Path(__file__).resolve().parents[3]


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
    if platform_tag.startswith("linux_"):
        return "libcyclops_sdk.so"
    if platform_tag.startswith("macosx_"):
        return "libcyclops_sdk.dylib"
    raise RuntimeError(f"unsupported wheel platform tag: {platform_tag}")


def build_native_library(repo_root: Path, cargo_target: str | None) -> Path:
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
    library = target_directory / "release" / native_library_name()
    if not library.is_file():
        raise RuntimeError(f"Cargo did not produce expected native library: {library}")
    return library


def stage_project(repo_root: Path, stage_root: Path, library: Path, native_library: str) -> None:
    source = repo_root / "cyclops-cs" / "python-sdk"
    shutil.copytree(
        source,
        stage_root,
        ignore=shutil.ignore_patterns("build", "dist", "*.egg-info", ".pytest_cache", "__pycache__", ".venv"),
    )
    shutil.copytree(repo_root / "cyclops-cs" / "sdk-bindings" / "python" / NATIVE_PACKAGE, stage_root / NATIVE_PACKAGE)
    shutil.copy2(library, stage_root / NATIVE_PACKAGE / native_library)

    pyproject = stage_root / "pyproject.toml"
    contents = pyproject.read_text()
    expected = 'packages = ["cua_train"]'
    replacement = 'packages = ["cua_train", "fleet_sdk"]'
    if expected not in contents:
        raise RuntimeError(f"could not configure staged package list in {pyproject}")
    pyproject.write_text(contents.replace(expected, replacement, 1))


def record_line(filename: str, data: bytes) -> list[str]:
    digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
    return [filename, f"sha256={digest}", str(len(data))]


def retag_wheel(wheel: Path, platform_tag: str) -> Path:
    with zipfile.ZipFile(wheel) as source:
        entries = {
            entry.filename: source.read(entry.filename)
            for entry in source.infolist()
            if not entry.filename.endswith(".dist-info/RECORD")
        }

    wheel_metadata_name = next(name for name in entries if name.endswith(".dist-info/WHEEL"))
    wheel_metadata = entries[wheel_metadata_name].decode().splitlines()
    updated_metadata = [
        "Root-Is-Purelib: false" if line == "Root-Is-Purelib: true" else line
        for line in wheel_metadata
        if not line.startswith("Tag: ")
    ]
    updated_metadata.append(f"Tag: py3-none-{platform_tag}")
    entries[wheel_metadata_name] = ("\n".join(updated_metadata) + "\n").encode()

    distribution = wheel.name.split("-")[0]
    version = wheel.name.split("-")[1]
    retagged = wheel.with_name(f"{distribution}-{version}-py3-none-{platform_tag}.whl")
    record_name = next(name for name in entries if name.endswith(".dist-info/WHEEL"))
    record_name = record_name.rsplit("/", 1)[0] + "/RECORD"

    with zipfile.ZipFile(retagged, "w", compression=zipfile.ZIP_DEFLATED) as target:
        records: list[list[str]] = []
        for name in sorted(entries):
            data = entries[name]
            target.writestr(name, data)
            records.append(record_line(name, data))
        records.append([record_name, "", ""])
        target.writestr(record_name, csv_text(records))

    wheel.unlink()
    return retagged


def csv_text(rows: list[list[str]]) -> str:
    from io import StringIO

    output = StringIO()
    csv.writer(output, lineterminator="\n").writerows(rows)
    return output.getvalue()


def main() -> None:
    args = parse_args()
    repo_root = repository_root()
    platform_tag = args.platform_tag or default_platform_tag()
    expected_library_name = native_library_name_for_platform_tag(platform_tag)
    library = (
        args.native_library.resolve()
        if args.native_library
        else build_native_library(repo_root, args.cargo_target)
    )
    if not library.is_file():
        raise RuntimeError(f"native library does not exist: {library}")
    if library.name != expected_library_name:
        raise RuntimeError(
            f"native library must be named {expected_library_name} for {platform_tag}: {library}"
        )

    output = args.outdir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="cua-train-wheel-") as temporary_directory:
        stage_root = Path(temporary_directory) / "python-sdk"
        stage_project(repo_root, stage_root, library, expected_library_name)
        subprocess.run(
            [sys.executable, "-m", "build", "--wheel", "--outdir", str(output), str(stage_root)],
            check=True,
        )

    wheels = sorted(output.glob(f"{PACKAGE_NAME}-*.whl"))
    if len(wheels) != 1:
        raise RuntimeError(f"expected exactly one built wheel in {output}, found: {wheels}")
    wheel = retag_wheel(wheels[0], platform_tag)
    print(wheel)


if __name__ == "__main__":
    main()
