# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Render the agent prompts for PA-01 from a seed: one brief per copy of BenchLab."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402


def setting(instances: int) -> str:
    if instances == 1:
        return "One BenchLab window is open on your Mac, at the top left of the screen."
    return (
        f"{instances} BenchLab windows are open on your Mac, laid out in a grid. Each is a separate copy of "
        f"the app, and a different agent is working in each of the other {instances - 1} copies at the same "
        "time as you."
    )


def render_cell(seed: int, cell: int, instances: int = C.PA_DEFAULT_INSTANCES) -> str:
    c = C.derive_parallel(seed, instances)["cells"][cell]
    values = {
        "setting": setting(instances),
        "label": c["label"],
        "position": c["position"],
        "code": c["code"],
        "shelf": c["shelf"],
    }
    return C.render_template(Path(__file__).resolve().parent / "brief.md", values)


def render_all(seed: int, instances: int = C.PA_DEFAULT_INSTANCES) -> list[str]:
    return [render_cell(seed, k, instances) for k in range(instances)]


def render(seed: int) -> str:
    """The brief of the agent in cell 0 at the default N (for listings; the runner uses render_all)."""
    return render_cell(seed, 0)
