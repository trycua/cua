# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Render the agent prompt for UM-02 (the busy user) from a seed."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def render(seed: int) -> str:
    c = C.derive_busy_desk(seed)
    values = {"label": c["label"], "code": c["code"], "shelf": c["shelf"]}
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)
