#!/usr/bin/env python3
"""Write the immutable hash manifest for a verified private wheel."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

from pydantic import BaseModel, ConfigDict

from gate_list_tools_schema import verify_wheel_provenance


class HcompCuaDriverBuild(BaseModel):
    """Published private wheel identity and hashes."""

    model_config = ConfigDict(extra="forbid")

    distribution: str
    version: str
    source_sha: str
    packaging_sha: str
    platform: str
    architecture: str
    wheel_sha256: str
    executable_sha256: str
    sdk_sha256: str
    tool_inventory_sha256: str


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_manifest(wheel: Path, inventory: Path, output: Path) -> HcompCuaDriverBuild:
    provenance = verify_wheel_provenance(wheel)
    json.loads(inventory.read_text(encoding="utf-8"))
    manifest = HcompCuaDriverBuild(
        distribution=provenance.distribution,
        version=provenance.version,
        source_sha=provenance.source_sha,
        packaging_sha=provenance.packaging_sha,
        platform=provenance.platform,
        architecture=provenance.architecture,
        wheel_sha256=sha256_file(wheel),
        executable_sha256=provenance.executable_sha256,
        sdk_sha256=provenance.sdk_sha256,
        tool_inventory_sha256=sha256_file(inventory),
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(
        json.dumps(manifest.model_dump(), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return manifest


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("wheel", type=Path)
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    write_manifest(args.wheel, args.inventory, args.output)


if __name__ == "__main__":
    main()
