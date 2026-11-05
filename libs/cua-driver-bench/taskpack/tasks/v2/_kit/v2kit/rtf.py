"""A small reader for the RTF that TextEdit (Cocoa) writes: paragraphs with alignment and bold runs.

It handles what TextEdit emits for plain styled text: groups, \\par, \\pard, \\qc/\\ql/\\qr/\\qj,
\\b/\\b0, \\i/\\i0, \\'hh, \\uN with \\ucN skipping, \\tab, and skips destinations (font and color
tables, \\*\\..., info, pictures). Good enough for checks on text, bold and centring; not a full parser.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

SKIP_DESTS = {"fonttbl", "colortbl", "expandedcolortbl", "stylesheet", "info", "pict", "listtable",
              "listoverridetable", "header", "footer", "*"}
TOKEN = re.compile(r"\\([a-zA-Z]+)(-?\d+)? ?|\\'([0-9a-fA-F]{2})|\\(.)|([{}])|([^\\{}\r\n]+)|[\r\n]+", re.S)


@dataclass
class Run:
    text: str
    bold: bool
    italic: bool


@dataclass
class Paragraph:
    align: str = "left"
    runs: list[Run] = field(default_factory=list)

    @property
    def text(self) -> str:
        return "".join(r.text for r in self.runs)

    def bold_text(self) -> str:
        return "".join(r.text for r in self.runs if r.bold)

    @property
    def all_bold(self) -> bool:
        visible = [r for r in self.runs if r.text.strip()]
        return bool(visible) and all(r.bold for r in visible)


def parse(data: str) -> list[Paragraph]:
    paragraphs: list[Paragraph] = [Paragraph()]
    stack: list[dict] = []
    st = {"bold": False, "italic": False, "align": "left", "skip": False, "uc": 1}
    pending_skip = 0
    for m in TOKEN.finditer(data):
        word, num, hexv, sym, brace, text = m.groups()
        if brace == "{":
            stack.append(dict(st))
            continue
        if brace == "}":
            if stack:
                st = stack.pop()
            continue
        if word is not None:
            if st["skip"]:
                continue
            if word in SKIP_DESTS:
                st["skip"] = True
                continue
            if word == "par" or word == "line":
                paragraphs.append(Paragraph(align=st["align"]))
            elif word == "pard":
                st["align"] = "left"
                paragraphs[-1].align = "left" if not paragraphs[-1].runs else paragraphs[-1].align
            elif word in ("qc", "ql", "qr", "qj"):
                st["align"] = {"qc": "center", "ql": "left", "qr": "right", "qj": "justified"}[word]
                if not paragraphs[-1].runs:
                    paragraphs[-1].align = st["align"]
            elif word == "b":
                st["bold"] = num != "0"
            elif word == "i":
                st["italic"] = num != "0"
            elif word == "plain":
                st["bold"] = st["italic"] = False
            elif word == "tab":
                paragraphs[-1].runs.append(Run("\t", st["bold"], st["italic"]))
            elif word == "uc":
                st["uc"] = int(num or 1)
            elif word == "u":
                code = int(num or 0)
                if code < 0:
                    code += 65536
                paragraphs[-1].runs.append(Run(chr(code), st["bold"], st["italic"]))
                pending_skip = st["uc"]
            continue
        if sym is not None:
            if sym == "*":
                st["skip"] = True
            elif not st["skip"] and sym in "\\{}":
                paragraphs[-1].runs.append(Run(sym, st["bold"], st["italic"]))
            elif not st["skip"] and sym in "\n\r":
                paragraphs.append(Paragraph(align=st["align"]))
            continue
        if hexv is not None:
            if pending_skip:
                pending_skip -= 1
                continue
            if not st["skip"]:
                paragraphs[-1].runs.append(Run(bytes([int(hexv, 16)]).decode("cp1252", "replace"), st["bold"], st["italic"]))
            continue
        if text is not None and not st["skip"]:
            if pending_skip:
                cut = min(pending_skip, len(text))
                text = text[cut:]
                pending_skip -= cut
            if text:
                if not paragraphs[-1].runs:
                    paragraphs[-1].align = st["align"]
                paragraphs[-1].runs.append(Run(text, st["bold"], st["italic"]))
    return [p for p in paragraphs if p.text.strip() or p.runs]


def load(path: Path) -> list[Paragraph]:
    return parse(path.read_text("latin-1"))
