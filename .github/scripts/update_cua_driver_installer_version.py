#!/usr/bin/env python3
"""Advance the baked Cua Driver installer version after publication."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
from typing import Sequence


class InstallerVersionError(RuntimeError):
    """Raised when installer version state cannot be updated safely."""


STABLE_VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
SHELL_VERSION = re.compile(
    r'^(CUA_DRIVER_RS_BAKED_VERSION=")([^"]+)(" # published-installer-version)$',
    re.MULTILINE,
)
POWERSHELL_VERSION = re.compile(
    r'^(\$Script:CuaDriverRsBakedVersion\s*=\s*")([^"]+)(" # published-installer-version)$',
    re.MULTILINE,
)


def version_tuple(version: str) -> tuple[int, int, int]:
    if not STABLE_VERSION.fullmatch(version):
        raise InstallerVersionError(
            f"installer version must be an exact stable x.y.z value: {version!r}"
        )
    major, minor, patch = version.split(".")
    return int(major), int(minor), int(patch)


def read_version(path: Path, pattern: re.Pattern[str]) -> str:
    matches = list(pattern.finditer(path.read_text(encoding="utf-8-sig")))
    if len(matches) != 1:
        raise InstallerVersionError(
            f"expected exactly one baked-version sentinel in {path}, found {len(matches)}"
        )
    version = matches[0].group(2)
    version_tuple(version)
    return version


def _replace_version(path: Path, pattern: re.Pattern[str], version: str) -> bool:
    source = path.read_text(encoding="utf-8-sig")
    matches = list(pattern.finditer(source))
    if len(matches) != 1:
        raise InstallerVersionError(
            f"expected exactly one baked-version sentinel in {path}, found {len(matches)}"
        )
    updated = pattern.sub(rf"\g<1>{version}\g<3>", source, count=1)
    if updated == source:
        return False
    path.write_text(updated, encoding="utf-8")
    return True


def _replace_state_version(path: Path, version: str) -> bool:
    current = path.read_text(encoding="utf-8").strip()
    version_tuple(current)
    if current == version:
        return False
    path.write_text(f"{version}\n", encoding="utf-8")
    return True


def update_installer_versions(
    shell_path: Path,
    powershell_path: Path,
    version: str,
    *,
    state_path: Path | None = None,
    allow_newer: bool = False,
) -> tuple[Path, ...]:
    requested = version_tuple(version)
    current = {
        shell_path: read_version(shell_path, SHELL_VERSION),
        powershell_path: read_version(powershell_path, POWERSHELL_VERSION),
    }
    if state_path is not None:
        state_version = state_path.read_text(encoding="utf-8").strip()
        version_tuple(state_version)
        current[state_path] = state_version
    if len(set(current.values())) != 1:
        rendered = ", ".join(f"{path}={value}" for path, value in current.items())
        raise InstallerVersionError(f"baked installer versions disagree: {rendered}")

    installed = version_tuple(next(iter(current.values())))
    if installed > requested:
        if allow_newer:
            return ()
        raise InstallerVersionError(
            f"refusing to move baked installer version backward "
            f"from {next(iter(current.values()))} to {version}"
        )

    changed: list[Path] = []
    for path, pattern in (
        (shell_path, SHELL_VERSION),
        (powershell_path, POWERSHELL_VERSION),
    ):
        if _replace_version(path, pattern, version):
            changed.append(path)
    if state_path is not None and _replace_state_version(state_path, version):
        changed.append(state_path)
    return tuple(changed)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--shell-path", type=Path, required=True)
    parser.add_argument("--powershell-path", type=Path, required=True)
    parser.add_argument(
        "--state-path",
        type=Path,
        help="published-version state file to update with the public installers",
    )
    parser.add_argument(
        "--allow-newer",
        action="store_true",
        help="leave a newer, already-published baked version unchanged",
    )
    args = parser.parse_args(argv)

    try:
        changed = update_installer_versions(
            args.shell_path,
            args.powershell_path,
            args.version,
            state_path=args.state_path,
            allow_newer=args.allow_newer,
        )
    except (InstallerVersionError, OSError) as error:
        parser.error(str(error))

    if changed:
        for path in changed:
            print(f"updated {path} to {args.version}")
    else:
        print(f"installer baked version already satisfies {args.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
