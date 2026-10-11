"""Render the agent prompt for MB-05 from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    p = C.derive_forms(seed)
    values = {
        "customer_name": p["customer_name"],
        "invoice_amount": p["invoice_amount"],
        "category": p["category_title"],
        "priority": p["priority"],
        "notify_instruction": "tick the Notify me checkbox"
        if p["notify"]
        else "leave the Notify me checkbox unticked",
        "quantity": p["quantity"],
        "notes": p["notes"],
    }
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)
