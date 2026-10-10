"""Render the agent prompt for IR-02 from a seed (Amendment 14)."""

from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    d = C.derive_irbanner(seed)
    return C.render_template(HERE / "brief.md", d)
