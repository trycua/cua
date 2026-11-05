"""Evaluate DS-04: the listed section titles are Heading 1, the listed sub-sections Heading 2, nothing else is a
heading, a table of contents listing every heading sits right after the title, and the text is unchanged."""

from __future__ import annotations

import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, odf  # noqa: E402

TASK_ID = "DS-04"
DOCUMENT = "guide.fodt"
TITLE = "Lichens of Skerrowen: A Field Guide for Walkers"
HEADINGS = [  # (level, text) in document order
    (1, "Before You Set Out"),
    (2, "Reading the Thallus"),
    (2, "Colour Under Wet and Dry Conditions"),
    (1, "The Western Shore"),
    (2, "Ember Crust (Pyrocrusta skerrowensis)"),
    (2, "Tide-line Tar Spot (Tarrodiscus littoralis)"),
    (1, "Moorland and Peat"),
    (2, "Pale Antler (Cervicladia cronachensis)"),
    (2, "Peat Pixie Cup (Turfiscypha minuta)"),
    (1, "The Hazel Woods of Dunvarra"),
    (2, "Script Lichen (Runographa dunvarrae)"),
    (2, "Glen Netleaf (Pulmolobia hiberna)"),
    (1, "Recording Your Finds"),
]


def _key(text: str) -> str:
    return odf.norm(text).rstrip(".").lower()


def toc_entries(node) -> list[str]:
    body_node = node.find("text:index-body", odf.NS)
    if body_node is None:
        return []
    out = []
    for p in body_node.iter(odf.q("text:p")):
        if any(p is t for title in body_node.iter(odf.q("text:index-title")) for t in title.iter(odf.q("text:p"))):
            continue
        text = odf.text_of(p).split("\t")[0]
        if odf.norm(text):
            out.append(odf.norm(text))
    return out


def evaluate(workspace: Path, checks: common.Checks) -> None:
    original = [odf.norm(p.text) for p in odf.paragraphs(odf.load(BUNDLE / "fixture" / DOCUMENT)) if odf.norm(p.text)]
    doc = odf.load(workspace / DOCUMENT)
    paras = [p for p in odf.paragraphs(doc) if odf.norm(p.text)]

    by_text = {odf.norm(p.text): p for p in paras}
    for level in (1, 2):
        wrong = []
        style = f"Heading_20_{level}"
        for lvl, text in HEADINGS:
            if lvl != level:
                continue
            p = by_text.get(text)
            if p is None or p.kind != "h" or p.level != level or style not in odf.style_parent_names(doc, p.style):
                wrong.append(f"{text!r}: {(p.kind, p.level, p.style) if p else 'missing'}")
        checks.add(f"heading_{level}_applied", not wrong, "; ".join(wrong[:4]))

    wanted = {text for _, text in HEADINGS}
    extra = [p.text for p in paras if p.kind == "h" and odf.norm(p.text) not in wanted]
    extra += [p.text for p in paras if odf.norm(p.text) not in wanted
              and any(n.startswith("Heading_20_") for n in odf.style_parent_names(doc, p.style))]
    checks.add("no_other_headings", not extra, "; ".join(extra[:4]), kind="preservation")

    title = paras[0] if paras else None
    checks.add("title_style_kept", title is not None and title.kind == "p" and odf.norm(title.text) == TITLE
               and "Title" in odf.style_parent_names(doc, title.style), f"first paragraph {title.style if title else None}",
               kind="preservation")
    checks.add("text_unchanged", [odf.norm(p.text) for p in paras] == original,
               f"{len(paras)} non-empty paragraphs, want {len(original)}", kind="preservation")

    text_body = odf.body(doc, "text")
    blocks = [n for n in text_body if n.tag in (odf.q("text:p"), odf.q("text:h"), odf.q("text:table-of-content"),
                                                 odf.q("text:section"), odf.q("text:list"), odf.q("table:table"))]
    blocks = [n for n in blocks if n.tag != odf.q("text:p") or odf.norm(odf.text_of(n))]
    tocs = list(text_body.iter(odf.q("text:table-of-content")))
    placed = (len(blocks) >= 2 and blocks[0].tag == odf.q("text:p") and odf.norm(odf.text_of(blocks[0])) == TITLE
              and blocks[1].tag == odf.q("text:table-of-content"))
    checks.add("toc_after_title", len(tocs) == 1 and placed,
               f"{len(tocs)} table(s) of contents; block after the title: {blocks[1].tag if len(blocks) > 1 else None}")
    entries = [_key(e) for e in toc_entries(tocs[0])] if tocs else []
    checks.add("toc_lists_headings", entries == [_key(t) for _, t in HEADINGS], f"entries {entries[:6]}")


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
