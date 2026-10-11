"""Render the agent prompt for MB-08 from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    p = C.derive_settings(seed)
    values = {
        "name": p["name"],
        "theme": p["theme"],
        "compact": "on" if p["compact_target"] else "off",
    }
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)
