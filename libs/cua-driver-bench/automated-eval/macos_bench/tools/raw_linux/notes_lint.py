#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Lint for the docs arm's hand-written notes (notes.md): they must stay generic, so they may not name any bench app,
bench task or trial. The banned names come from the bench's own definitions (the v2 taskset, each probe's task.json
and the task pack's app folders) plus the product names below, so a new task or app is covered without editing this
file. Exit 1 and one line per hit when the notes name one."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
BENCH = HERE.parent.parent  # automated-eval/macos_bench
TASKPACK = BENCH.parent.parent / "taskpack"
# Products the tasks run that the definitions spell only inside longer phrases
PRODUCTS = ("BenchLab", "LibreOffice", "Calc", "soffice", "GnuCash", "Electron", "Chrome", "Chromium", "Signal Quay",
            "Copperfield", "Daymark", "Ledger Lantern", "Margin Lantern", "Rillwatch", "Lighthouse Desk")
TASK_ID = re.compile(r"\b(?:CDB|MB|IR|WEB)-[A-Z]?\d{2}\b", re.I)
TRIAL_ID = re.compile(r"-r\d+-(?:pi|cc|cx)-", re.I)


def banned_names() -> set[str]:
    names = set(PRODUCTS)
    taskset = json.loads((TASKPACK / "v2-taskset.json").read_text("utf-8"))
    for task in taskset["tasks"]:
        names.add(task["id"])
    for path in sorted((BENCH / "probes").glob("*/task.json")):
        spec = json.loads(path.read_text("utf-8"))
        names.add(spec.get("id") or path.parent.name)
        for key in spec.get("atspi_coverage") or {}:
            names.update(re.findall(r"'([^']+)'", key))  # the AT-SPI app names
            lead = key.split("(")[0]
            names.update(n.strip() for n in lead.split(",") if n.strip())
    for app in sorted(TASKPACK.glob("tasks/shared/*/apps/*")):
        names.add(app.name)
        names.add(app.name.replace("-", " "))
    return {n for n in names if len(n) >= 3}


def lint(notes: str) -> list[str]:
    hits = []
    for name in sorted(banned_names(), key=str.lower):
        if re.search(r"(?<![A-Za-z0-9])" + re.escape(name) + r"(?![A-Za-z0-9])", notes, re.I):
            hits.append(f"names a bench app or task: {name!r}")
    for pattern in (TASK_ID, TRIAL_ID):
        for m in sorted(set(pattern.findall(notes))):
            hits.append(f"names a bench task or trial id: {m!r}")
    return hits


def main(argv: list[str]) -> int:
    path = Path(argv[1]) if len(argv) > 1 else HERE / "notes.md"
    hits = lint(path.read_text("utf-8"))
    for h in hits:
        print(f"{path.name}: {h}")
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
