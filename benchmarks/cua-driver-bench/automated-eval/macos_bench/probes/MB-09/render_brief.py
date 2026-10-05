"""Render the agent prompt for MB-09 from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    p = C.derive_listdrag(seed)
    values = {f"i{n}": item for n, item in enumerate(p["target"], start=1)}
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)
