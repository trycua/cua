"""Render the agent prompt for MB-02 from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    p = C.derive_canvasmenu(seed)
    values = {}
    for i, (c, a) in enumerate(zip(p["targets"], p["actions"]), start=1):
        values[f"c{i}"] = c
        values[f"a{i}"] = a
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)
