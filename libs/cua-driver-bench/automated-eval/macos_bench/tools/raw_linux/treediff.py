# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Tree diffs for the pi-raw-linux-diff variant: a re-dump of the same root in one trial returns only what changed
since the previous dump. Standard library only, so it is testable without AT-SPI."""

from __future__ import annotations

from pathlib import Path


def dump_diff(store: Path, key: str, text: str, full: bool) -> str:
    """Save ``text`` as the latest dump under ``key`` and return it whole (first dump, or ``full``), or only what
    changed since the previous dump: removed lines with "-", new or changed lines with "+", each change under the
    nearest unchanged line above it ("@") so its place in the tree stays readable."""
    import difflib

    store.mkdir(parents=True, exist_ok=True)
    path = store / f"{key}.txt"
    previous = path.read_text("utf-8").splitlines() if path.is_file() else None
    path.write_text(text, "utf-8")
    lines = text.splitlines()
    if previous is None or full:
        return text
    if previous == lines:
        return f"[tree diff] unchanged since your last dump of this root ({len(lines)} lines); full=True for all"
    out = [
        f"[tree diff] changes since your last dump of this root (now {len(lines)} lines; "
        "raw.dump(root, full=True) for the whole tree):"
    ]
    for op, a0, a1, b0, b1 in difflib.SequenceMatcher(
        a=previous, b=lines, autojunk=False
    ).get_opcodes():
        if op == "equal":
            continue
        if b0 > 0:
            out.append("@ " + lines[b0 - 1])
        out += ["- " + ln for ln in previous[a0:a1]]
        out += ["+ " + ln for ln in lines[b0:b1]]
    diff = "\n".join(out)
    # a diff longer than the tree it describes is no saving: return the tree
    return diff if len(diff) < len(text) else text
