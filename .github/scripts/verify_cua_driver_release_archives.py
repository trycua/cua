#!/usr/bin/env python3
"""Verify that Cua Driver release archives satisfy the installer contract."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
import tarfile
import zipfile


class ContractError(RuntimeError):
    """Raised when a release archive is missing or malformed."""


@dataclass(frozen=True)
class ArchiveContract:
    filename: str
    members: tuple[str, ...]
    executable_members: tuple[str, ...] = ()


def release_contracts(version: str) -> tuple[ArchiveContract, ...]:
    """Return every archive and member required for a driver release."""

    contracts: list[ArchiveContract] = []

    linux_payload = (
        "cua-driver",
        "cua-cursor-theme",
        "libcua_driver_sdk.so",
        "cua_driver_node_runtime.node",
        "cua_driver_abi.h",
    )
    for arch in ("x86_64", "arm64"):
        stage = f"cua-driver-rs-{version}-linux-{arch}"
        contracts.extend(
            (
                ArchiveContract(
                    f"{stage}.tar.gz",
                    tuple(f"{stage}/{member}" for member in linux_payload),
                    (
                        f"{stage}/cua-driver",
                        f"{stage}/cua-cursor-theme",
                    ),
                ),
                ArchiveContract(
                    f"{stage}-binary.tar.gz",
                    linux_payload,
                    ("cua-driver", "cua-cursor-theme"),
                ),
            )
        )

    windows_payload = (
        "cua-driver.exe",
        "cua-cursor-theme.exe",
        "cua-driver-uia.exe",
        "cua_driver_sdk.dll",
        "cua_driver_node_runtime.node",
        "cua_driver_abi.h",
    )
    for arch in ("x86_64", "arm64"):
        stage = f"cua-driver-rs-{version}-windows-{arch}"
        contracts.extend(
            (
                ArchiveContract(
                    f"{stage}.zip",
                    tuple(f"{stage}/{member}" for member in windows_payload),
                ),
                ArchiveContract(f"{stage}-binary.zip", windows_payload),
            )
        )

    macos_payload = (
        "cua-driver",
        "cua-cursor-theme",
        "libcua_driver_sdk.dylib",
        "cua_driver_node_runtime.node",
        "cua_driver_abi.h",
        "CuaDriver.app/Contents/Info.plist",
        "CuaDriver.app/Contents/MacOS/cua-driver",
        "CuaDriver.app/Contents/MacOS/cua-cursor-theme",
    )
    for label in ("darwin-arm64", "darwin-x86_64", "darwin-universal"):
        stage = f"cua-driver-rs-{version}-{label}"
        contracts.append(
            ArchiveContract(
                f"{stage}.tar.gz",
                tuple(f"{stage}/{member}" for member in macos_payload),
                (
                    f"{stage}/cua-driver",
                    f"{stage}/cua-cursor-theme",
                    f"{stage}/CuaDriver.app/Contents/MacOS/cua-driver",
                    f"{stage}/CuaDriver.app/Contents/MacOS/cua-cursor-theme",
                ),
            )
        )

    contracts.append(
        ArchiveContract(
            f"cua-driver-rs-{version}-darwin-universal-binary.tar.gz",
            (
                "cua-driver",
                "cua-cursor-theme",
                "libcua_driver_sdk.dylib",
                "cua_driver_node_runtime.node",
                "cua_driver_abi.h",
            ),
            ("cua-driver", "cua-cursor-theme"),
        )
    )
    return tuple(contracts)


def _normalize_member(name: str) -> str:
    normalized = str(PurePosixPath(name.replace("\\", "/")))
    return normalized.removeprefix("./").rstrip("/")


def _find_archive(root: Path, filename: str) -> Path:
    matches = sorted(path for path in root.rglob(filename) if path.is_file())
    if not matches:
        raise ContractError(f"missing release archive: {filename}")
    if len(matches) != 1:
        rendered = ", ".join(str(path) for path in matches)
        raise ContractError(f"duplicate release archive {filename}: {rendered}")
    return matches[0]


def _verify_tar(path: Path, contract: ArchiveContract) -> None:
    with tarfile.open(path, "r:gz") as archive:
        members = {
            _normalize_member(member.name): member
            for member in archive.getmembers()
            if member.isfile()
        }

        for expected in contract.members:
            member = members.get(expected)
            if member is None:
                raise ContractError(f"{path.name} is missing {expected}")
            if member.size <= 0:
                raise ContractError(f"{path.name} contains empty member {expected}")

        for expected in contract.executable_members:
            member = members.get(expected)
            if member is None or member.mode & 0o111 == 0:
                raise ContractError(f"{path.name} contains non-executable member {expected}")


def _verify_zip(path: Path, contract: ArchiveContract) -> None:
    with zipfile.ZipFile(path) as archive:
        members = {
            _normalize_member(info.filename): info
            for info in archive.infolist()
            if not info.is_dir()
        }

        for expected in contract.members:
            member = members.get(expected)
            if member is None:
                raise ContractError(f"{path.name} is missing {expected}")
            if member.file_size <= 0:
                raise ContractError(f"{path.name} contains empty member {expected}")


def verify_release_archives(root: Path, version: str) -> tuple[Path, ...]:
    """Verify all archives for *version* below *root*."""

    verified: list[Path] = []
    for contract in release_contracts(version):
        path = _find_archive(root, contract.filename)
        if path.name.endswith(".tar.gz"):
            _verify_tar(path, contract)
        elif path.suffix == ".zip":
            _verify_zip(path, contract)
        else:  # pragma: no cover - contracts above define supported formats.
            raise ContractError(f"unsupported archive format: {path}")
        verified.append(path)
    return tuple(verified)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()

    try:
        verified = verify_release_archives(args.artifacts, args.version)
    except (ContractError, tarfile.TarError, zipfile.BadZipFile) as error:
        parser.error(str(error))

    print(f"Verified {len(verified)} Cua Driver release archives:")
    for path in verified:
        print(f"  - {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
