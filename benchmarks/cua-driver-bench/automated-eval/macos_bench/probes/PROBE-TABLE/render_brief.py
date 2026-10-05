"""Render the participant brief for PROBE-TABLE from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    codes = C.derive_table(seed)["codes"]
    values = {"code_1": codes[0], "code_2": codes[1], "code_3": codes[2]}
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)
