"""Render the agent prompt for MB-12 from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    p = C.derive_forms(seed)
    values = {
        "customer_name": p["customer_name"],
        "category": p["category_title"],
        "priority": p["priority"],
    }
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)
