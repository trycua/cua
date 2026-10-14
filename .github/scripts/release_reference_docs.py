"""Locate the generated reference pages that carry a release-version marker."""

from __future__ import annotations

import json
from pathlib import Path

GENERATED_MARKER = "AUTO-GENERATED FILE"


def reference_paths(root: Path, generator: str) -> tuple[str, ...]:
    """Repo-relative generated ``.mdx`` pages under a generator's output folder.

    The generators write one page per command group or tool group, so the set
    is discovered from the output folder rather than listed. Hand-written
    pages next to them carry no generated header and are left alone.
    """
    config = json.loads((root / "scripts/docs-generators/config.json").read_text())
    output = root / config["generators"][generator]["docsOutputPath"]
    paths = sorted(
        path.relative_to(root).as_posix()
        for path in output.rglob("*.mdx")
        if GENERATED_MARKER in path.read_text()
    )
    if not paths:
        raise RuntimeError(f"no generated {generator} reference pages under {output}")
    return tuple(paths)
