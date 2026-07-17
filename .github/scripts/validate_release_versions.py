#!/usr/bin/env python3
"""Verify that checked-in Driver and Lume release versions agree."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 and earlier
    import tomli as tomllib
from typing import Sequence


class VersionError(RuntimeError):
    """Checked-in version sources do not describe one release."""


def read_match(path: Path, pattern: str) -> str:
    match = re.search(pattern, path.read_text(), re.MULTILINE)
    if not match:
        raise VersionError(f"could not read a release version from {path}")
    return match.group(1)


def require_equal(product: str, expected: str, values: dict[str, str]) -> None:
    mismatches = {name: value for name, value in values.items() if value != expected}
    if mismatches:
        details = ", ".join(f"{name}={value}" for name, value in sorted(mismatches.items()))
        raise VersionError(f"{product} expects {expected}; mismatched sources: {details}")


def driver_versions(root: Path) -> tuple[str, dict[str, str]]:
    base = root / "libs/cua-driver"
    expected = (base / "rust/VERSION").read_text().strip()
    cargo = tomllib.loads((base / "rust/Cargo.toml").read_text())
    python_project = tomllib.loads((base / "python/pyproject.toml").read_text())
    values = {
        "rust/Cargo.toml": str(cargo["workspace"]["package"]["version"]),
        "python/pyproject.toml": str(python_project["project"]["version"]),
        "python/src/cua_driver/__init__.py": read_match(
            base / "python/src/cua_driver/__init__.py", r'^__version__\s*=\s*"([^"]+)"'
        ),
        "scripts/_install-rust.sh": read_match(
            base / "scripts/_install-rust.sh", r'^CUA_DRIVER_RS_BAKED_VERSION="([^"]+)"'
        ),
        "scripts/install.ps1": read_match(
            base / "scripts/install.ps1",
            r'^\$Script:CuaDriverRsBakedVersion\s*=\s*"([^"]+)"',
        ),
    }

    members = [base / "rust" / member for member in cargo["workspace"]["members"]]
    local_names = {
        tomllib.loads((member / "Cargo.toml").read_text())["package"]["name"] for member in members
    }
    lock = tomllib.loads((base / "rust/Cargo.lock").read_text())
    for package in lock["package"]:
        if package["name"] in local_names:
            values[f"rust/Cargo.lock:{package['name']}"] = str(package["version"])
    missing = local_names - {
        key.split(":", 1)[1] for key in values if key.startswith("rust/Cargo.lock:")
    }
    if missing:
        raise VersionError(f"Cargo.lock is missing workspace packages: {sorted(missing)}")
    return expected, values


def lume_versions(root: Path) -> tuple[str, dict[str, str]]:
    base = root / "libs/lume"
    expected = (base / "VERSION").read_text().strip()
    return expected, {
        "src/Main.swift": read_match(
            base / "src/Main.swift", r'static let current: String = "([^"]+)"'
        ),
        "scripts/install.sh": read_match(
            base / "scripts/install.sh", r'^LUME_BAKED_VERSION="([^"]+)"'
        ),
    }


def validate(root: Path, product: str) -> None:
    manifest = json.loads((root / ".release-please-manifest.json").read_text())
    if product in {"all", "driver"}:
        expected, values = driver_versions(root)
        values[".release-please-manifest.json"] = str(manifest["libs/cua-driver"])
        require_equal("Cua Driver", expected, values)
    if product in {"all", "lume"}:
        expected, values = lume_versions(root)
        values[".release-please-manifest.json"] = str(manifest["libs/lume"])
        require_equal("Lume", expected, values)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path.cwd())
    parser.add_argument("--product", choices=("all", "driver", "lume"), default="all")
    args = parser.parse_args(argv)
    try:
        validate(args.repo_root.resolve(), args.product)
    except (KeyError, OSError, VersionError, ValueError) as error:
        print(f"release version error: {error}", file=sys.stderr)
        return 1
    print(f"release versions agree for {args.product}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
