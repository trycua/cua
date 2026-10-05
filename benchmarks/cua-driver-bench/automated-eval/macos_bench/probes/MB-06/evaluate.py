#!/usr/bin/env python3
"""Evaluate MB-06: type a paragraph, bold one sentence, replace one word, press Done.

Usage: evaluate.py --seed N --state PATH --events PATH --result OUT.json

The checker reads the document model of the editor (its text and the character ranges whose font is
bold), written by the app. Required: text_correct (final text equals the paragraph with the one word
replaced), replacement_applied, bold_covers_sentence (every non-space character of the stated sentence
is bold), no_bold_elsewhere, done_pressed (the Done snapshot equals the live document), doc_via_events
(plus integrity). Spaces at the sentence boundaries may be bold or not. Diagnostics: how the text got
in (typing events versus whole-value edits), selection and format event counts.
"""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "_common"))
import benchlab_common as C  # noqa: E402

MODE = "richtext"


def _bold_indices(doc: dict[str, Any], text: str) -> set[int] | None:
    ranges = doc.get("bold_ranges")
    if not isinstance(ranges, list):
        return None
    out: set[int] = set()
    for r in ranges:
        if not (isinstance(r, list) and len(r) == 2 and C.is_int(r[0]) and C.is_int(r[1])):
            return None
        out.update(range(r[0], r[0] + r[1]))
    return {i for i in out if 0 <= i < len(text) and not text[i].isspace()}


def evaluate(
    seed: int, state: dict[str, Any], events: list[dict[str, Any]], checks: C.Checks
) -> None:
    exp = C.derive_richtext(seed)
    doc = state.get("doc") if isinstance(state.get("doc"), dict) else {}
    text = doc.get("text") if isinstance(doc.get("text"), str) else ""
    want = exp["final_text"]

    checks.add(
        "text_correct", text.strip() == want, 0.2, f"document is {text!r}, expected {want!r}"
    )
    old, new = exp["old_word"], exp["new_word"]
    words = text.replace(".", " ").split()
    replaced = old not in words and new in words
    checks.add(
        "replacement_applied",
        replaced,
        0.1,
        f"{old!r} -> {new!r}: old word present={old in words}, new word present={new in words}",
    )

    bold = _bold_indices(doc, text)
    length = exp["bold_range"][1]
    pos = text.find(exp["bold_sentence"])
    loc = pos if pos >= 0 else exp["bold_range"][0]
    sentence = {i for i in range(loc, loc + length) if i < len(text) and not text[i].isspace()}
    if bold is None:
        checks.add("bold_covers_sentence", False, 0.25, "state has no bold ranges")
        checks.add("no_bold_elsewhere", False, 0.15, "state has no bold ranges")
    else:
        missing = sorted(sentence - bold)
        extra = sorted(bold - sentence)
        checks.add(
            "bold_covers_sentence",
            not missing and bool(sentence),
            0.25,
            f"{len(missing)} character(s) of the stated sentence are not bold; bold text is {doc.get('bold_texts')!r}",
        )
        checks.add(
            "no_bold_elsewhere",
            not extra,
            0.15,
            f"{len(extra)} bold character(s) outside the stated sentence",
        )

    done = C.events_of(events, "done_click")
    done_ok = (
        bool(done)
        and state.get("done") is True
        and state.get("done_count") == len(done)
        and state.get("done_doc") == doc
    )
    checks.add(
        "done_pressed",
        done_ok,
        0.1,
        f"{len(done)} done_click event(s); Done snapshot equals live document: {state.get('done_doc') == doc}",
    )

    changes = C.events_of(events, "text_change")
    via_ok = bool(changes) and C.details(changes[-1]).get("text") == text
    checks.add(
        "doc_via_events",
        via_ok,
        0.1,
        "the last logged text change equals the final text"
        if via_ok
        else "the logged text changes do not end at the final text",
    )

    vias: dict[str, int] = {}
    for e in changes:
        v = str(C.details(e).get("via"))
        vias[v] = vias.get(v, 0) + 1
    checks.diag("text_change_via", vias)
    checks.diag("text_change_events", len(changes))
    checks.diag("format_change_events", len(C.events_of(events, "format_change")))
    checks.diag("selection_events", len(C.events_of(events, "selection")))
    checks.diag(
        "max_selection_length",
        max(
            (
                C.details(e).get("length", 0)
                for e in C.events_of(events, "selection")
                if C.is_int(C.details(e).get("length"))
            ),
            default=0,
        ),
    )


if __name__ == "__main__":
    C.main_guard(MODE, evaluate)
