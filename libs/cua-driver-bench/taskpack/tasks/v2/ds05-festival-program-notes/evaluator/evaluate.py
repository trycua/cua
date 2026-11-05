"""Evaluate DS-05: every [[venue]] placeholder replaced by the hall's name, one footnote right after the ticket
sentence with the given text, only the given phrase bold, everything else unchanged."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, odf  # noqa: E402

TASK_ID = "DS-05"
DOCUMENT = "programme.fodt"
PLACEHOLDER = "[[venue]]"
VENUE = "Greywether Assembly Rooms"
SENTENCE = "Tickets for the evening concert are limited to 120."
NOTE = "Children under twelve go free but still need a ticket from the steward's table."
PHRASE = "Bring your own cup"
TITLE_STYLE = "Title"


def _key(text: str) -> str:
    # Writer's AutoCorrect turns a typed ' into a typographic apostrophe; either is fine
    text = text.replace("\u2019", "'").replace("\u2018", "'")
    return odf.norm(text).rstrip(".").lower()


def text_before_note(node) -> str | None:
    """Visible text of a paragraph up to its first footnote/endnote, or None without a note."""
    parts: list[str] = []

    def walk(n) -> bool:
        if n.text:
            parts.append(n.text)
        for child in n:
            if child.tag == odf.q("text:note"):
                return True
            if child.tag == odf.q("text:s"):
                parts.append(" " * int(child.get(odf.q("text:c"), "1")))
            elif child.tag == odf.q("text:tab"):
                parts.append("\t")
            elif child.tag != odf.q("office:annotation") and walk(child):
                return True
            if child.tail:
                parts.append(child.tail)
        return False

    return "".join(parts) if walk(node) else None


def evaluate(workspace: Path, checks: common.Checks) -> None:
    original = [p for p in odf.paragraphs(odf.load(BUNDLE / "fixture" / DOCUMENT)) if odf.norm(p.text)]
    expected_count = sum(p.text.count(PLACEHOLDER) for p in original)
    doc = odf.load(workspace / DOCUMENT)
    paras = [p for p in odf.paragraphs(doc) if odf.norm(p.text)]
    notes = odf.notes(doc)
    all_text = "\n".join(p.text for p in paras) + "\n" + "\n".join(n["body"] for n in notes)

    # the draft has no square brackets except in the placeholders
    checks.add("placeholders_gone", "[" not in all_text and "]" not in all_text and "venue" not in all_text.lower(),
               f"{all_text.count('[')} '[' left")
    count = sum(p.text.count(VENUE) for p in paras)
    checks.add("venue_inserted", count == expected_count, f"{count} times, want {expected_count}")

    sentence_para = next(p for p in original if SENTENCE in p.text)
    footnotes = [n for n in notes if n["class"] == "footnote"]
    target = [p for p in paras if SENTENCE in p.text]
    placed = False
    if len(target) == 1:
        before = text_before_note(target[0].node)
        placed = before is not None and odf.norm(before).endswith(SENTENCE)
    checks.add("footnote_added", len(notes) == 1 and len(footnotes) == 1 and _key(footnotes[0]["body"]) == _key(NOTE) and placed,
               f"{len(notes)} note(s) {[(n['class'], n['body'][:60]) for n in notes]}; after the sentence: {placed}")

    bold_runs = {}
    for p in paras:
        if TITLE_STYLE in odf.style_parent_names(doc, p.style) or p.kind == "h":
            continue
        bold = "".join(t if odf.is_bold(props) else "\x00" for t, props in odf.spans(doc, p))
        pieces = [odf.norm(x) for x in bold.split("\x00") if odf.norm(x)]
        if pieces:
            bold_runs[odf.norm(p.text)[:40]] = pieces
    phrase_para = [k for k, v in bold_runs.items() if v == [PHRASE]]
    checks.add("phrase_bold", len(phrase_para) == 1 and phrase_para[0].startswith("11:30"), f"bold runs {bold_runs}")
    others = {k: v for k, v in bold_runs.items() if v != [PHRASE]}
    checks.add("nothing_else_bold", not others, f"{others}", kind="preservation")

    def masked(text: str) -> str:
        return odf.norm(text.replace(VENUE, PLACEHOLDER))

    got = [(p.kind, masked(p.text)) for p in paras]
    want = [(p.kind, odf.norm(p.text)) for p in original]
    diff = next(((g, w) for g, w in zip(got, want) if g != w), (len(got), len(want)))
    checks.add("other_text_unchanged", got == want, f"first difference {diff}", kind="preservation")
    checks.diag("sentence_paragraph", odf.norm(sentence_para.text)[:60])


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
