#!/usr/bin/env python3
"""Verify that Cua Driver release archives satisfy the installer contract."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
import tarfile
import zipfile


FORBIDDEN_MODEL_SUFFIXES = (".onnx", ".ort", ".gguf", ".safetensors")
AGPL_MARKERS = (b"agpl-", b"agpl ", b"gnu affero general public license")
NOTICE_NAMES = ("license", "notice", "copying")
FORBIDDEN_BINARY_MARKERS = (
    b"onnxruntime",
    b"OrtGetApiBase",
    b"onnxruntime_providers",
)
MAX_BINARY_SIZE = 128 * 1024 * 1024
MAX_ARCHIVE_UNCOMPRESSED_SIZE = 512 * 1024 * 1024

BINARY_MAGIC = {
    "elf": (b"\x7fELF",),
    "pe": (b"MZ",),
    "mach-o": (
        b"\xca\xfe\xba\xbe",
        b"\xca\xfe\xba\xbf",
        b"\xce\xfa\xed\xfe",
        b"\xcf\xfa\xed\xfe",
        b"\xfe\xed\xfa\xce",
        b"\xfe\xed\xfa\xcf",
    ),
}


class ContractError(RuntimeError):
    """Raised when a release archive is missing or malformed."""


@dataclass(frozen=True)
class ArchiveContract:
    filename: str
    members: tuple[str, ...]
    executable_members: tuple[str, ...] = ()
    optional_members: tuple[str, ...] = ()
    binary_members: tuple[str, ...] = ()
    binary_format: str = ""


def release_contracts(version: str) -> tuple[ArchiveContract, ...]:
    """Return every archive and member required for a driver release."""

    contracts: list[ArchiveContract] = []

    linux_runtime = (
        "cua-driver",
        "cua-cursor-theme",
        "libcua_driver_sdk.so",
        "cua_driver_node_runtime.node",
        "cua_driver_abi.h",
    )
    linux_helper = (
        "wayland-helper/README.md",
        "wayland-helper/install.sh",
        "wayland-helper/winrects@cua/extension.js",
        "wayland-helper/winrects@cua/metadata.json",
    )
    linux_payload = linux_runtime + linux_helper
    linux_binaries = linux_runtime[:-1]
    for arch in ("x86_64", "arm64"):
        stage = f"cua-driver-rs-{version}-linux-{arch}"
        contracts.extend(
            (
                ArchiveContract(
                    f"{stage}.tar.gz",
                    tuple(f"{stage}/{member}" for member in (*linux_payload, "LICENSE")),
                    (
                        f"{stage}/cua-driver",
                        f"{stage}/cua-cursor-theme",
                    ),
                    binary_members=tuple(
                        f"{stage}/{member}" for member in linux_binaries
                    ),
                    binary_format="elf",
                ),
                ArchiveContract(
                    f"{stage}-binary.tar.gz",
                    linux_payload,
                    ("cua-driver", "cua-cursor-theme"),
                    binary_members=linux_binaries,
                    binary_format="elf",
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
                    tuple(f"{stage}/{member}" for member in (*windows_payload, "LICENSE")),
                    binary_members=tuple(
                        f"{stage}/{member}" for member in windows_payload[:-1]
                    ),
                    binary_format="pe",
                ),
                ArchiveContract(
                    f"{stage}-binary.zip",
                    windows_payload,
                    binary_members=windows_payload[:-1],
                    binary_format="pe",
                ),
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
        "CuaDriver.app/Contents/Resources/AppIcon.icns",
    )
    macos_optional = (
        "CuaDriver.app/Contents/_CodeSignature/CodeResources",
        "CuaDriver.app/Contents/CodeResources",
        "CuaDriver.app/Contents/embedded.provisionprofile",
    )
    macos_binaries = (
        "cua-driver",
        "cua-cursor-theme",
        "libcua_driver_sdk.dylib",
        "cua_driver_node_runtime.node",
        "CuaDriver.app/Contents/MacOS/cua-driver",
        "CuaDriver.app/Contents/MacOS/cua-cursor-theme",
    )
    for label in ("darwin-arm64", "darwin-x86_64", "darwin-universal"):
        stage = f"cua-driver-rs-{version}-{label}"
        contracts.append(
            ArchiveContract(
                f"{stage}.tar.gz",
                tuple(f"{stage}/{member}" for member in (*macos_payload, "LICENSE")),
                (
                    f"{stage}/cua-driver",
                    f"{stage}/cua-cursor-theme",
                    f"{stage}/CuaDriver.app/Contents/MacOS/cua-driver",
                    f"{stage}/CuaDriver.app/Contents/MacOS/cua-cursor-theme",
                ),
                tuple(f"{stage}/{member}" for member in macos_optional),
                tuple(f"{stage}/{member}" for member in macos_binaries),
                "mach-o",
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
            binary_members=(
                "cua-driver",
                "cua-cursor-theme",
                "libcua_driver_sdk.dylib",
                "cua_driver_node_runtime.node",
            ),
            binary_format="mach-o",
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


def _forbidden_member_reason(name: str) -> str | None:
    normalized = _normalize_member(name).lower()
    path = f"/{normalized.strip('/')}"
    basename = PurePosixPath(normalized).name
    if "cua-perception" in normalized:
        return "cua-perception worker"
    if "/models/" in f"{path}/" or basename == "models":
        return "model directory"
    if basename.endswith(FORBIDDEN_MODEL_SUFFIXES):
        return "model payload"
    if "onnxruntime" in normalized:
        return "ONNX Runtime"
    if "catalog" in basename:
        return "extension catalog"
    if "agpl" in normalized:
        return "AGPL notice"
    return None


def _verify_member_names(path: Path, names: tuple[str, ...]) -> None:
    for name in names:
        if reason := _forbidden_member_reason(name):
            raise ContractError(
                f"{path.name} contains forbidden optional perception payload "
                f"({reason}): {_normalize_member(name)}"
            )


def _verify_notice_content(path: Path, name: str, payload: bytes) -> None:
    basename = PurePosixPath(_normalize_member(name)).name.lower()
    if not any(token in basename for token in NOTICE_NAMES):
        return
    lowered = payload.lower()
    if any(marker in lowered for marker in AGPL_MARKERS):
        raise ContractError(
            f"{path.name} contains forbidden optional perception payload "
            f"(AGPL notice): {_normalize_member(name)}"
        )


def _allowed_directories(names: set[str]) -> set[str]:
    directories: set[str] = set()
    for name in names:
        parent = PurePosixPath(name).parent
        while str(parent) != ".":
            directories.add(str(parent))
            parent = parent.parent
    return directories


def _verify_exact_members(
    path: Path,
    contract: ArchiveContract,
    file_names: tuple[str, ...],
    directory_names: tuple[str, ...],
) -> None:
    required = set(contract.members)
    allowed = required | set(contract.optional_members)
    actual = set(file_names)
    missing = sorted(required - actual)
    if missing:
        raise ContractError(f"{path.name} is missing {missing[0]}")
    unexpected = sorted(actual - allowed)
    if unexpected:
        raise ContractError(f"{path.name} contains unexpected member {unexpected[0]}")
    unexpected_directories = sorted(set(directory_names) - _allowed_directories(allowed))
    if unexpected_directories:
        raise ContractError(
            f"{path.name} contains unexpected directory {unexpected_directories[0]}"
        )
    if len(file_names) != len(actual):
        raise ContractError(f"{path.name} contains duplicate file members")


def _verify_binary(path: Path, contract: ArchiveContract, name: str, payload: bytes) -> None:
    if len(payload) > MAX_BINARY_SIZE:
        raise ContractError(
            f"{path.name} binary exceeds {MAX_BINARY_SIZE} bytes: {name}"
        )
    magic = BINARY_MAGIC[contract.binary_format]
    if not payload.startswith(magic):
        raise ContractError(
            f"{path.name} member is not {contract.binary_format} binary: {name}"
        )
    lowered = payload.lower()
    for marker in FORBIDDEN_BINARY_MARKERS:
        if marker.lower() in lowered:
            raise ContractError(
                f"{path.name} binary links or vendors optional perception runtime "
                f"({marker.decode()}): {name}"
            )


def _verify_tar(path: Path, contract: ArchiveContract) -> None:
    with tarfile.open(path, "r:gz") as archive:
        archive_members = archive.getmembers()
        _verify_member_names(path, tuple(member.name for member in archive_members))
        members = {
            _normalize_member(member.name): member for member in archive_members if member.isfile()
        }

        files = tuple(_normalize_member(member.name) for member in archive_members if member.isfile())
        directories = tuple(
            _normalize_member(member.name) for member in archive_members if member.isdir()
        )
        unsupported = next(
            (member for member in archive_members if not member.isfile() and not member.isdir()),
            None,
        )
        if unsupported is not None:
            raise ContractError(
                f"{path.name} contains unsupported archive member "
                f"{_normalize_member(unsupported.name)}"
            )
        _verify_exact_members(path, contract, files, directories)
        total_size = sum(member.size for member in members.values())
        if total_size > MAX_ARCHIVE_UNCOMPRESSED_SIZE:
            raise ContractError(f"{path.name} uncompressed payload is too large: {total_size}")

        for name, member in members.items():
            if member.size <= 0:
                raise ContractError(f"{path.name} contains empty member {name}")
            if any(token in PurePosixPath(name).name.lower() for token in NOTICE_NAMES):
                extracted = archive.extractfile(member)
                if extracted is not None:
                    _verify_notice_content(path, name, extracted.read())

        for expected in contract.members:
            member = members.get(expected)
            if member is None:
                raise ContractError(f"{path.name} is missing {expected}")

        for expected in contract.executable_members:
            member = members.get(expected)
            if member is None or member.mode & 0o111 == 0:
                raise ContractError(f"{path.name} contains non-executable member {expected}")

        for expected in contract.binary_members:
            extracted = archive.extractfile(members[expected])
            if extracted is None:  # pragma: no cover - member is already a regular file.
                raise ContractError(f"{path.name} cannot read binary member {expected}")
            _verify_binary(path, contract, expected, extracted.read())


def _verify_zip(path: Path, contract: ArchiveContract) -> None:
    with zipfile.ZipFile(path) as archive:
        archive_members = archive.infolist()
        _verify_member_names(path, tuple(info.filename for info in archive_members))
        members = {
            _normalize_member(info.filename): info for info in archive_members if not info.is_dir()
        }
        files = tuple(_normalize_member(info.filename) for info in archive_members if not info.is_dir())
        directories = tuple(
            _normalize_member(info.filename) for info in archive_members if info.is_dir()
        )
        _verify_exact_members(path, contract, files, directories)
        total_size = sum(member.file_size for member in members.values())
        if total_size > MAX_ARCHIVE_UNCOMPRESSED_SIZE:
            raise ContractError(f"{path.name} uncompressed payload is too large: {total_size}")

        for name, member in members.items():
            if member.file_size <= 0:
                raise ContractError(f"{path.name} contains empty member {name}")
            if any(token in PurePosixPath(name).name.lower() for token in NOTICE_NAMES):
                _verify_notice_content(path, name, archive.read(member))

        for expected in contract.members:
            member = members.get(expected)
            if member is None:
                raise ContractError(f"{path.name} is missing {expected}")

        for expected in contract.binary_members:
            _verify_binary(path, contract, expected, archive.read(members[expected]))


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
