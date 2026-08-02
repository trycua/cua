#!/usr/bin/env python3
"""Reconcile a component release manifest with the latest main manifest."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


COMPONENT_PATHS = {
    "cua-driver-rs": "libs/cua-driver",
    "lume": "libs/lume",
}


def load_manifest(path: Path) -> dict[str, Any]:
    with path.open(encoding="utf-8") as manifest_file:
        manifest = json.load(manifest_file)
    if not isinstance(manifest, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return manifest


def merge_component_versions(
    main_manifest: dict[str, Any],
    release_manifest: dict[str, Any],
    components: list[str],
) -> dict[str, Any]:
    """Keep main's versions except for the component(s) owned by this release PR."""

    merged = dict(main_manifest)
    for component in components:
        component_path = COMPONENT_PATHS[component]
        if component_path not in release_manifest:
            raise ValueError(
                f"release manifest does not contain {component_path!r} for {component}"
            )
        merged[component_path] = release_manifest[component_path]
    return merged


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--main", type=Path, required=True)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument(
        "--component",
        action="append",
        choices=sorted(COMPONENT_PATHS),
        required=True,
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    merged = merge_component_versions(
        load_manifest(args.main),
        load_manifest(args.release),
        args.component,
    )
    print(json.dumps(merged, indent=2))


if __name__ == "__main__":
    main()
