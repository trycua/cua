"""Render the participant brief for PROBE-CANVAS from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    # The layout is seed dependent but the instructions are the same for every seed.
    C.derive_canvas(seed)
    return C.render_template(Path(__file__).resolve().parent / "brief.md", {})
