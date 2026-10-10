"""Render the agent prompt for IR-04 from a seed (Amendment 14)."""

from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    d = C.derive_irconsent(seed)
    phrase = (
        "tick \"Send me the weekly digest\""
        if d["digest"]
        else "leave \"Send me the weekly digest\" unticked"
    )
    return C.render_template(HERE / "brief.md", {"email": d["email"], "plan": d["plan"], "digest_phrase": phrase})
