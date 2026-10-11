"""Render the agent prompt for MB-10 from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    values = {"target": C.derive_hover(seed)["target"]}
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)
