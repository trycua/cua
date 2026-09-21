#!/usr/bin/env python
"""Renders the combined results chart embedded in libs/cua-bench-s1/README.md.

Reads no external files -- the numbers here are transcribed directly from the
"## Results" table in this package's README.md and must be kept in sync with
it by hand when that table changes. This keeps the chart script dependency-free
(matplotlib only) and reproducible without needing the original eval run
directories on disk.

Usage:
    python libs/cua-bench-s1/scripts/render_results_chart.py
"""
from __future__ import annotations

from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np

OUT_DIR = Path(__file__).resolve().parent.parent / "docs" / "assets"

CORE_FAMILIES = [
    "consent_checkbox", "form_filling", "login_auth",
    "multi_step_submit", "pagination", "search_filter",
]
# Text, hard cross-dataset split (615 tasks total, GUI-360 held out).
CORE_TEXT_HARD = {
    "jev": [0.000, 0.576, 0.083, 0.034, 0.000, 0.021],
    "djev": [0.500, 0.939, 0.250, 0.412, 0.714, 0.271],
    "semif": [0.250, 0.212, 0.167, 0.270, 0.286, 0.292],
    "cua-s1-nano-0.1": [0.000, 0.273, 0.250, 0.256, 0.286, 0.208],
    "cua-s1-4b-0.1": [0.250, 0.455, 0.167, 0.322, 0.571, 0.271],
}
COLORS = {
    "jev": "#8C8C8C",
    "djev": "#55A868",
    "semif": "#DD8452",
    "cua-s1-nano-0.1": "#4C72B0",
    "cua-s1-4b-0.1": "#C44E52",
}

GENERAL_DECISION = {
    "jev": 0.667,
    "djev": 0.623,
    "semif": 0.563,
    "cua-s1-4b-0.1": 0.632,
}

# Multimodal, same-distribution split (86 tasks total, per-family N=9-20).
CORE_MULTIMODAL = {
    "djev": [0.071, 0.000, 0.000, 0.750, 0.444, 0.429],
    "semif": [0.357, 0.100, 0.235, 0.167, 0.000, 0.214],
    "cua-s1-nano-0.1": [1.000, 1.000, 1.000, 1.000, 1.000, 1.000],
    "cua-s1-4b-0.1": [1.000, 1.000, 1.000, 1.000, 1.000, 1.000],
}

# Mean per-task inference latency (seconds), one entry per row of the
# "### Latency" table in the README, `None` where a model was not measured.
LATENCY_ROWS = {
    "jev": [0.556, None, 0.553, None, 0.534, None, None, 0.597],
    "djev": [0.770, 1.195, 3.061, 3.389, 3.168, 3.450, 1.172, 0.858],
    "semif": [0.120, 0.293, 1.078, 1.557, 1.117, 1.151, 0.300, 0.157],
    "cua-s1-nano-0.1": [0.003, 0.222, 0.0145, None, 0.015, 0.739, 0.206, None],
    "cua-s1-4b-0.1": [0.1245, 0.345, 1.096, None, 1.274, 1.355, 0.343, 0.203],
}


def render() -> Path:
    OUT_DIR.mkdir(parents=True, exist_ok=True)

    fig, (ax_core, ax_gd) = plt.subplots(1, 2, figsize=(14, 5), width_ratios=[3, 1])
    models = list(CORE_TEXT_HARD.keys())
    x = np.arange(len(CORE_FAMILIES))
    width = 0.8 / len(models)
    for i, model in enumerate(models):
        offsets = x + (i - (len(models) - 1) / 2) * width
        ax_core.bar(offsets, CORE_TEXT_HARD[model], width, label=model, color=COLORS[model])
    ax_core.set_xticks(x)
    ax_core.set_xticklabels(CORE_FAMILIES, rotation=30, ha="right")
    ax_core.set_ylabel("Task accuracy")
    ax_core.set_ylim(0, 1)
    ax_core.set_title("6 core GUI families -- text, hard cross-dataset split (N=615)")
    ax_core.legend(fontsize=8)
    ax_core.grid(axis="y", alpha=0.3)

    gd_models = list(GENERAL_DECISION.keys())
    ax_gd.bar(gd_models, [GENERAL_DECISION[m] for m in gd_models],
              color=[COLORS[m] for m in gd_models])
    ax_gd.set_xticks(range(len(gd_models)))
    ax_gd.set_xticklabels(gd_models, rotation=30, ha="right")
    ax_gd.set_ylim(0, 1)
    ax_gd.set_title("general_decision\n(external jevbench, N=231)")
    ax_gd.grid(axis="y", alpha=0.3)

    fig.tight_layout()
    out_path = OUT_DIR / "results_chart.png"
    fig.savefig(out_path, dpi=150)
    plt.close(fig)

    fig2, (ax_mm, ax_lat) = plt.subplots(1, 2, figsize=(14, 5), width_ratios=[3, 1])
    mm_models = list(CORE_MULTIMODAL.keys())
    width2 = 0.8 / len(mm_models)
    for i, model in enumerate(mm_models):
        offsets = x + (i - (len(mm_models) - 1) / 2) * width2
        ax_mm.bar(offsets, CORE_MULTIMODAL[model], width2, label=model, color=COLORS[model])
    ax_mm.set_xticks(x)
    ax_mm.set_xticklabels(CORE_FAMILIES, rotation=30, ha="right")
    ax_mm.set_ylabel("Task accuracy")
    ax_mm.set_ylim(0, 1.05)
    ax_mm.set_title("6 core GUI families -- multimodal, same-distribution split (N=86)")
    ax_mm.legend(fontsize=8)
    ax_mm.grid(axis="y", alpha=0.3)

    lat_models = list(LATENCY_ROWS.keys())
    lat_means = [float(np.mean([v for v in LATENCY_ROWS[m] if v is not None])) for m in lat_models]
    ax_lat.bar(lat_models, lat_means, color=[COLORS[m] for m in lat_models])
    ax_lat.set_xticks(range(len(lat_models)))
    ax_lat.set_xticklabels(lat_models, rotation=30, ha="right")
    ax_lat.set_ylabel("Seconds")
    ax_lat.set_title("Mean per-task latency\n(averaged across all measured rows)")
    ax_lat.grid(axis="y", alpha=0.3)

    fig2.tight_layout()
    out_path2 = OUT_DIR / "results_chart_mm_latency.png"
    fig2.savefig(out_path2, dpi=150)
    plt.close(fig2)

    return out_path, out_path2


if __name__ == "__main__":
    p1, p2 = render()
    print(f"wrote {p1}")
    print(f"wrote {p2}")
