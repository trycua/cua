"""Render the agent prompt for MB-06 from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    p = C.derive_richtext(seed)
    values = {
        "paragraph": p["paragraph"],
        "bold_ordinal": C.ORDINALS[p["bold_index"]],
        "replace_ordinal": C.ORDINALS[p["replace_index"]],
        "old_word": p["old_word"],
        "new_word": p["new_word"],
    }
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)
