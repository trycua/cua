# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""The `lookup` tool of the `pi-raw-macos-docs` arm: a query in, the matching documentation entries out.

The entries are the generated index of the running macOS build (docs/gen_docs.py writes
``<RAW_DOCS_DIR>/<build>/index.jsonl``) and the hand-written notes (docs/notes.md, one entry per ``## `` section).
Ranking: an exact name first, then entries that match more of the query's words, then BM25 over each entry's name
(weighted), declaration, doc text and framework (pi's tool_search orders tools the same way), with identifiers split at
camelCase and underscores, so ``CGImageDestination`` finds ``CGImageDestinationCreateWithURL`` and ``save image png``
finds the note and the ImageIO calls. An exact name query ranks that entry first. Ties break on the name, so the same
query always returns the same entries. Nothing here runs code or touches the desktop.
"""

from __future__ import annotations

import json
import math
import plistlib
import re
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
NOTES = HERE / "docs" / "notes.md"
DEFAULT_LIMIT, MAX_LIMIT = 8, 20
DOC_CHARS = 700  # an API entry's doc text is cut here; a note comes back whole
NAME_WEIGHT = 3.0
K1, B = 1.2, 0.75
STOP = {
    "a",
    "an",
    "and",
    "are",
    "as",
    "at",
    "be",
    "by",
    "for",
    "from",
    "in",
    "is",
    "it",
    "of",
    "on",
    "or",
    "that",
    "the",
    "this",
    "to",
    "with",
    "how",
    "do",
    "i",
    "can",
    "use",
}


def stem(word: str) -> str:
    if len(word) > 4 and word.endswith("ies"):
        return word[:-3] + "y"
    if len(word) > 4 and re.search(r"(ches|shes|sses|xes|zes)$", word):
        return word[:-2]
    if len(word) > 3 and word.endswith("s") and not word.endswith("ss"):
        return word[:-1]
    return word


def tokens(text: str) -> list[str]:
    text = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", text)
    text = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1 \2", text)
    words = re.sub(r"[^a-z0-9]+", " ", text.lower()).split()
    return [stem(w) for w in words if w not in STOP]


def os_build() -> str:
    with open("/System/Library/CoreServices/SystemVersion.plist", "rb") as handle:
        return str(plistlib.load(handle)["ProductBuildVersion"])


def load_notes(path: Path = NOTES) -> list[dict]:
    notes, title, body = [], None, []
    for line in path.read_text("utf-8").splitlines():
        if line.startswith("## "):
            if title:
                notes.append(
                    {
                        "kind": "note",
                        "name": title,
                        "framework": "notes",
                        "decl": "",
                        "doc": "\n".join(body).strip(),
                    }
                )
            title, body = line[3:].strip(), []
        elif title:
            body.append(line)
    if title:
        notes.append(
            {
                "kind": "note",
                "name": title,
                "framework": "notes",
                "decl": "",
                "doc": "\n".join(body).strip(),
            }
        )
    return notes


class Index:
    def __init__(self, entries: list[dict]) -> None:
        self.entries = entries
        self.name_terms: list[set[str]] = []
        self.text: list[Counter] = []
        self.lengths: list[int] = []
        df: Counter = Counter()
        for e in entries:
            name = tokens(e["name"].replace(":", " ").replace(".", " "))
            text = Counter(
                tokens(
                    " ".join((e["name"], e.get("pyobjc", ""), e["decl"], e["doc"], e["framework"]))
                )
            )
            self.name_terms.append(set(name))
            self.text.append(text)
            self.lengths.append(sum(text.values()))
            df.update(set(text))
        n = len(entries)
        self.avg = sum(self.lengths) / max(n, 1)
        self.idf = {t: math.log(1 + (n - c + 0.5) / (c + 0.5)) for t, c in df.items()}
        self.by_name: dict[str, list[int]] = {}
        for i, e in enumerate(entries):
            for key in {
                e["name"].lower(),
                e["name"].split(".")[-1].lower(),
                e.get("pyobjc", "").lower(),
            } - {""}:
                self.by_name.setdefault(key, []).append(i)

    @classmethod
    def load(cls, docs_dir: Path, build: str | None = None) -> "Index":
        build = build or os_build()
        index = docs_dir / build / "index.jsonl"
        if not index.is_file():
            raise FileNotFoundError(f"no generated docs for macOS build {build} ({index})")
        entries = [json.loads(line) for line in index.read_text("utf-8").splitlines() if line]
        return cls(load_notes() + entries)

    def search(self, query: str, limit: int = DEFAULT_LIMIT) -> list[dict]:
        limit = max(1, min(int(limit or DEFAULT_LIMIT), MAX_LIMIT))
        exact = self.by_name.get(query.strip().lower().lstrip("_"), [])
        terms = list(dict.fromkeys(tokens(query)))
        scores: dict[int, float] = {}
        covered: dict[int, int] = {}
        for t in terms:
            idf = self.idf.get(t)
            if idf is None:
                continue
            for i, counts in enumerate(self.text):
                tf = counts.get(t)
                if not tf:
                    continue
                norm = tf * (K1 + 1) / (tf + K1 * (1 - B + B * self.lengths[i] / self.avg))
                scores[i] = (
                    scores.get(i, 0.0)
                    + idf * norm
                    + (NAME_WEIGHT * idf if t in self.name_terms[i] else 0.0)
                )
                covered[i] = covered.get(i, 0) + 1
        for i in exact:
            scores.setdefault(i, 0.0)
        exact_set = set(exact)
        # an exact name first, then entries matching more of the query's words, then BM25 (as pi's tool_search)
        ranked = sorted(
            scores,
            key=lambda i: (
                i not in exact_set,
                -covered.get(i, 0),
                -round(scores[i], 9),
                self.entries[i]["name"],
                i,
            ),
        )
        return [self.entries[i] for i in ranked[:limit]]


def render(entries: list[dict]) -> str:
    if not entries:
        return "No matching entries."
    out = []
    for e in entries:
        if e["kind"] == "note":
            out.append(f"[note] {e['name']}\n{e['doc']}")
            continue
        head = f"[{e['framework']}, {e['kind']}] {e['name']}"
        if e.get("pyobjc"):
            head += f"  (pyobjc: {e['pyobjc']})"
        if e.get("deprecated"):
            head += f"  (deprecated in macOS {e['deprecated']})"
        doc = e["doc"] if len(e["doc"]) <= DOC_CHARS else e["doc"][:DOC_CHARS] + "..."
        out.append("\n".join(x for x in (head, e["decl"], doc) if x))
    return "\n\n".join(out)
