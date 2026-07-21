#!/usr/bin/env python3
"""Resolve a targeted Release Please component and optional SemVer bump."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
from typing import Any


COMPONENT_PATHS = {
    "cua-driver-rs": "libs/cua-driver",
    "lume": "libs/lume",
}
SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
BUMP_TYPES = {"automatic", "patch", "minor", "major"}


def bump_version(version: str, bump: str) -> str | None:
    """Return the requested stable SemVer, or None for automatic inference."""
    if bump not in BUMP_TYPES:
        raise ValueError(f"unsupported bump type: {bump}")
    if bump == "automatic":
        return None

    match = SEMVER.fullmatch(version)
    if not match:
        raise ValueError(f"manifest version is not stable SemVer: {version}")
    major, minor, patch = (int(part) for part in match.groups())
    if bump == "patch":
        patch += 1
    elif bump == "minor":
        minor += 1
        patch = 0
    else:
        major += 1
        minor = 0
        patch = 0
    return f"{major}.{minor}.{patch}"


def resolve_request(manifest: dict[str, Any], component: str, bump: str) -> dict[str, Any]:
    """Resolve a user-facing component name into Release Please inputs."""
    try:
        path = COMPONENT_PATHS[component]
    except KeyError as exc:
        raise ValueError(f"unsupported component: {component}") from exc

    version = manifest.get(path)
    if not isinstance(version, str):
        raise ValueError(f"manifest does not contain a version for {path}")
    return {
        "component": component,
        "path": path,
        "current_version": version,
        "release_as": bump_version(version, bump),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--component", required=True)
    parser.add_argument("--bump", required=True)
    args = parser.parse_args()

    manifest = json.loads(args.manifest.read_text())
    if not isinstance(manifest, dict):
        raise ValueError("release manifest must be a JSON object")
    print(json.dumps(resolve_request(manifest, args.component, args.bump), sort_keys=True))


if __name__ == "__main__":
    main()
