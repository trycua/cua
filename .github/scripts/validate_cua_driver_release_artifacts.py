#!/usr/bin/env python3
"""Validate the native payload contract for Cua Driver release archives."""

from __future__ import annotations

import argparse
from pathlib import Path
import tarfile
import zipfile


class ArtifactValidationError(RuntimeError):
    """Raised when a release archive is absent or incomplete."""


def _runtime_files(platform: str) -> tuple[str, ...]:
    if platform == "linux":
        return (
            "cua-driver",
            "cua-cursor-theme",
            "libcua_driver_sdk.so",
            "cua_driver_node_runtime.node",
            "cua_driver_abi.h",
        )
    if platform == "windows":
        return (
            "cua-driver.exe",
            "cua-cursor-theme.exe",
            "cua-driver-uia.exe",
            "cua_driver_sdk.dll",
            "cua_driver_node_runtime.node",
            "cua_driver_abi.h",
        )
    if platform == "darwin":
        return (
            "cua-driver",
            "cua-cursor-theme",
            "libcua_driver_sdk.dylib",
            "cua_driver_node_runtime.node",
            "cua_driver_abi.h",
        )
    raise ValueError(f"unsupported platform: {platform}")


def expected_archives(version: str) -> dict[str, set[str]]:
    """Return every publishable runtime archive and its required members."""
    expected: dict[str, set[str]] = {}

    for arch in ("arm64", "x86_64"):
        for platform, extension in (("linux", "tar.gz"), ("windows", "zip")):
            stage = f"cua-driver-rs-{version}-{platform}-{arch}"
            files = _runtime_files(platform)
            expected[f"{stage}.{extension}"] = {f"{stage}/{name}" for name in files}
            expected[f"{stage}-binary.{extension}"] = set(files)

    darwin_files = _runtime_files("darwin")
    for arch in ("arm64", "x86_64", "universal"):
        stage = f"cua-driver-rs-{version}-darwin-{arch}"
        members = {f"{stage}/{name}" for name in darwin_files}
        members.update(
            {
                f"{stage}/CuaDriver.app/Contents/Info.plist",
                f"{stage}/CuaDriver.app/Contents/MacOS/cua-driver",
                f"{stage}/CuaDriver.app/Contents/MacOS/cua-cursor-theme",
            }
        )
        expected[f"{stage}.tar.gz"] = members

    expected[f"cua-driver-rs-{version}-darwin-universal-binary.tar.gz"] = set(
        darwin_files
    )
    return expected


def _archive_members(path: Path) -> set[str]:
    if path.name.endswith(".tar.gz"):
        with tarfile.open(path, "r:gz") as archive:
            names = archive.getnames()
    elif path.suffix == ".zip":
        with zipfile.ZipFile(path) as archive:
            names = archive.namelist()
    else:
        raise ArtifactValidationError(f"unsupported archive format: {path.name}")
    return {name.replace("\\", "/").lstrip("./") for name in names}


def validate_release_artifacts(asset_dir: Path, version: str) -> None:
    failures: list[str] = []
    for filename, required_members in expected_archives(version).items():
        path = asset_dir / filename
        if not path.is_file():
            failures.append(f"{filename}: archive is missing")
            continue
        try:
            members = _archive_members(path)
        except (OSError, tarfile.TarError, zipfile.BadZipFile) as exc:
            failures.append(f"{filename}: cannot read archive: {exc}")
            continue
        missing = sorted(required_members - members)
        if missing:
            failures.append(f"{filename}: missing {', '.join(missing)}")

    if failures:
        details = "\n".join(f"  - {failure}" for failure in failures)
        raise ArtifactValidationError(
            f"Cua Driver {version} release artifact validation failed:\n{details}"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--asset-dir", type=Path, required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    try:
        validate_release_artifacts(args.asset_dir, args.version)
    except ArtifactValidationError as exc:
        parser.error(str(exc))
    print(f"validated Cua Driver {args.version} runtime archives in {args.asset_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
