"""Render the agent prompt for IR-03 from a seed (Amendment 14)."""

from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    d = C.derive_irunsaved(seed)
    values = {
        "status_note": d["titles"][d["status_index"]],
        "status": d["status"],
        "rename_note": d["titles"][d["rename_index"]],
        "new_title": d["new_title"],
    }
    return C.render_template(HERE / "brief.md", values)
