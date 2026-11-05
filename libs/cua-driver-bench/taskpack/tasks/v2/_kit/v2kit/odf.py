"""Read OpenDocument files saved by LibreOffice: flat XML (.fods/.fodt/.fodp) or zipped (.ods/.odt/.odp).

Only what the v2 checkers need: sheet cells (value, type, formula, text, style), text paragraphs and
headings with their styles, slides with titles and notes, and style properties resolved through the
automatic and common styles.
"""

from __future__ import annotations

import re
import xml.etree.ElementTree as ET
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Iterator

NS = {
    "office": "urn:oasis:names:tc:opendocument:xmlns:office:1.0",
    "table": "urn:oasis:names:tc:opendocument:xmlns:table:1.0",
    "text": "urn:oasis:names:tc:opendocument:xmlns:text:1.0",
    "style": "urn:oasis:names:tc:opendocument:xmlns:style:1.0",
    "fo": "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0",
    "draw": "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0",
    "presentation": "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0",
    "number": "urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0",
    "config": "urn:oasis:names:tc:opendocument:xmlns:config:1.0",
    "xlink": "http://www.w3.org/1999/xlink",
    "calcext": "urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0",
    "loext": "urn:org:documentfoundation:names:experimental:office:xmlns:loext:1.0",
}
MAX_BYTES = 8 * 1024 * 1024
MAX_ROWS = 5000
MAX_COLS = 256


def q(prefix_name: str) -> str:
    prefix, name = prefix_name.split(":", 1)
    return "{" + NS[prefix] + "}" + name


# ------------------------------------------------------------------ loading


@dataclass
class Doc:
    content: ET.Element  # the element holding office:body and office:automatic-styles
    styles: ET.Element | None  # the element holding office:styles (same as content for flat files)
    settings: ET.Element | None
    path: Path
    style_index: dict[str, ET.Element] = field(default_factory=dict)


def load(path: Path) -> Doc:
    if path.stat().st_size > MAX_BYTES:
        raise ValueError(f"{path.name} is too large")
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            names = set(archive.namelist())

            def part(name: str) -> ET.Element | None:
                if name not in names:
                    return None
                if archive.getinfo(name).file_size > MAX_BYTES:
                    raise ValueError(f"{name} is too large")
                return ET.fromstring(archive.read(name))

            content = part("content.xml")
            if content is None:
                raise ValueError("no content.xml")
            doc = Doc(content, part("styles.xml"), part("settings.xml"), path)
    else:
        root = ET.parse(path).getroot()
        doc = Doc(root, root, root, path)
    for holder in (doc.styles, doc.content):
        if holder is None:
            continue
        for container in ("office:styles", "office:automatic-styles"):
            for node in holder.iter(q(container)):
                for style in node:
                    name = style.get(q("style:name"))
                    if name:
                        doc.style_index.setdefault(name, style)
    return doc


def body(doc: Doc, kind: str) -> ET.Element:
    node = doc.content.find(f"office:body/office:{kind}", NS)
    if node is None:
        raise ValueError(f"no office:{kind} body")
    return node


def text_of(node: ET.Element) -> str:
    """Visible text of a paragraph-like node: spaces, tabs and line breaks expanded, notes left out."""
    parts: list[str] = []

    def walk(n: ET.Element) -> None:
        if n.text:
            parts.append(n.text)
        for child in n:
            tag = child.tag
            if tag == q("text:s"):
                parts.append(" " * int(child.get(q("text:c"), "1")))
            elif tag == q("text:tab"):
                parts.append("\t")
            elif tag == q("text:line-break"):
                parts.append("\n")
            elif tag in (q("text:note"), q("office:annotation")):
                pass
            else:
                walk(child)
            if child.tail:
                parts.append(child.tail)

    walk(node)
    return "".join(parts)


def norm(value: str) -> str:
    return re.sub(r"\s+", " ", value or "").strip()


# ------------------------------------------------------------------ styles


def style_props(doc: Doc, name: str | None, family_prop: str = "style:text-properties") -> dict[str, str]:
    """Properties of a style and its parents (child wins), for one property element kind."""
    out: dict[str, str] = {}
    seen: set[str] = set()
    chain: list[ET.Element] = []
    while name and name not in seen:
        seen.add(name)
        style = doc.style_index.get(name)
        if style is None:
            break
        chain.append(style)
        name = style.get(q("style:parent-style-name"))
    for style in reversed(chain):
        props = style.find(family_prop, NS)
        if props is not None:
            for key, value in props.attrib.items():
                out[key] = value
    return out


def style_parent_names(doc: Doc, name: str | None) -> list[str]:
    """The style and its ancestors, by name (automatic styles point at the named style they derive from)."""
    out: list[str] = []
    while name and name not in out:
        out.append(name)
        style = doc.style_index.get(name)
        if style is None:
            break
        name = style.get(q("style:parent-style-name"))
    return out


def display_name(doc: Doc, name: str) -> str:
    style = doc.style_index.get(name)
    if style is not None and style.get(q("style:display-name")):
        return style.get(q("style:display-name")) or name
    return name.replace("_20_", " ")


def is_bold(props: dict[str, str]) -> bool:
    return props.get(q("fo:font-weight"), "normal") in ("bold", "700", "800", "900")


# ------------------------------------------------------------------ spreadsheets


@dataclass
class Cell:
    value_type: str | None
    value: str | None  # office:value / date-value / boolean-value / string-value
    text: str
    formula: str | None
    style: str | None

    @property
    def number(self) -> float | None:
        if self.value_type in ("float", "currency", "percentage") and self.value is not None:
            try:
                return float(self.value)
            except ValueError:
                return None
        return None

    @property
    def display(self) -> str:
        return norm(self.text)


EMPTY = Cell(None, None, "", None, None)


def _cell(node: ET.Element, column_style: str | None) -> Cell:
    vtype = node.get(q("office:value-type")) or node.get(q("calcext:value-type"))
    value = None
    for key in ("office:value", "office:date-value", "office:time-value", "office:boolean-value", "office:string-value"):
        if node.get(q(key)) is not None:
            value = node.get(q(key))
            break
    paragraphs = [text_of(p) for p in node.findall("text:p", NS)]
    return Cell(vtype, value, "\n".join(paragraphs), node.get(q("table:formula")), node.get(q("table:style-name")) or column_style)


def sheets(doc: Doc) -> dict[str, list[list[Cell]]]:
    """Sheet name -> rows of cells (repeats expanded, trailing empties trimmed)."""
    out: dict[str, list[list[Cell]]] = {}
    for table in body(doc, "spreadsheet").findall("table:table", NS):
        col_styles: list[str | None] = []
        for col in table.iter(q("table:table-column")):
            rep = min(int(col.get(q("table:number-columns-repeated"), "1")), MAX_COLS)
            col_styles += [col.get(q("table:default-cell-style-name"))] * rep
        rows: list[list[Cell]] = []
        for row in table.iter(q("table:table-row")):
            rep = int(row.get(q("table:number-rows-repeated"), "1"))
            cells: list[Cell] = []
            for node in row:
                if node.tag not in (q("table:table-cell"), q("table:covered-table-cell")):
                    continue
                crep = min(int(node.get(q("table:number-columns-repeated"), "1")), MAX_COLS)
                col_style = col_styles[len(cells)] if len(cells) < len(col_styles) else None
                cell = _cell(node, col_style)
                cells += [cell] * crep
                if len(cells) >= MAX_COLS:
                    break
            while cells and cells[-1].value_type is None and not cells[-1].text:
                cells.pop()
            rows += [cells] * min(rep, MAX_ROWS)
            if len(rows) >= MAX_ROWS:
                break
        while rows and not rows[-1]:
            rows.pop()
        out[table.get(q("table:name")) or f"Sheet{len(out) + 1}"] = rows
    return out


def cell(rows: list[list[Cell]], ref: str) -> Cell:
    """Cell by A1 reference (0-based lookups past the end give an empty cell)."""
    match = re.fullmatch(r"([A-Z]+)(\d+)", ref)
    if not match:
        raise ValueError(ref)
    col = 0
    for ch in match.group(1):
        col = col * 26 + (ord(ch) - 64)
    r, c = int(match.group(2)) - 1, col - 1
    if r >= len(rows) or c >= len(rows[r]):
        return EMPTY
    return rows[r][c]


def table_dicts(rows: list[list[Cell]], header_row: int = 0) -> list[dict[str, Cell]]:
    """Rows below a header row as {header text: cell}; blank rows skipped."""
    if len(rows) <= header_row:
        return []
    headers = [c.display for c in rows[header_row]]
    out = []
    for row in rows[header_row + 1 :]:
        if not any(c.display or c.value is not None for c in row):
            continue
        out.append({h: (row[i] if i < len(row) else EMPTY) for i, h in enumerate(headers) if h})
    return out


def config_items(doc: Doc) -> dict[str, str]:
    """Flattened view settings: 'Sheet/Name' keys for per-sheet items, plain names otherwise (first wins)."""
    out: dict[str, str] = {}
    if doc.settings is None:
        return out
    for entry in doc.settings.iter(q("config:config-item-map-entry")):
        name = entry.get(q("config:name"))
        for item in entry.findall("config:config-item", NS):
            key = f"{name}/{item.get(q('config:name'))}" if name else item.get(q("config:name")) or ""
            out.setdefault(key, item.text or "")
    return out


def data_style_of(doc: Doc, cell_style: str | None) -> ET.Element | None:
    """The number/currency/date data style a cell style points at, if any."""
    for name in style_parent_names(doc, cell_style):
        style = doc.style_index.get(name)
        if style is None:
            continue
        data_name = style.get(q("style:data-style-name"))
        if data_name and data_name in doc.style_index:
            return doc.style_index[data_name]
    return None


# ------------------------------------------------------------------ text documents


@dataclass
class Para:
    kind: str  # "h" or "p" or "li" (a paragraph inside a list)
    style: str | None
    level: int | None
    text: str
    node: ET.Element


def paragraphs(doc: Doc) -> list[Para]:
    """Headings and paragraphs of the text body in document order (inside sections and lists too)."""
    out: list[Para] = []

    def walk(node: ET.Element, in_list: bool) -> None:
        for child in node:
            if child.tag == q("text:h"):
                out.append(Para("h", child.get(q("text:style-name")), int(child.get(q("text:outline-level"), "1")), text_of(child), child))
            elif child.tag == q("text:p"):
                out.append(Para("li" if in_list else "p", child.get(q("text:style-name")), None, text_of(child), child))
            elif child.tag in (q("text:list"), q("text:list-item"), q("text:list-header")):
                walk(child, True)
            elif child.tag in (q("text:section"), q("table:table"), q("table:table-row"), q("table:table-cell")):
                walk(child, in_list)

    walk(body(doc, "text"), False)
    return out


def notes(doc: Doc) -> list[dict[str, str]]:
    """Footnotes and endnotes: class, citation, body text, and the text of the paragraph that holds them."""
    out = []
    for para in body(doc, "text").iter():
        if para.tag not in (q("text:p"), q("text:h")):
            continue
        for note in para.iter(q("text:note")):
            body_node = note.find("text:note-body", NS)
            out.append(
                {
                    "class": note.get(q("text:note-class"), ""),
                    "body": norm(" ".join(text_of(p) for p in body_node.iter(q("text:p")))) if body_node is not None else "",
                    "anchor_text": norm(text_of(para)),
                }
            )
    return out


def tables(doc: Doc) -> list[list[list[str]]]:
    """Writer tables as rows of cell texts."""
    out = []
    for table in body(doc, "text").iter(q("table:table")):
        rows = []
        for row in table.iter(q("table:table-row")):
            rows.append([norm(" ".join(text_of(p) for p in c.findall("text:p", NS))) for c in row if c.tag == q("table:table-cell")])
        out.append(rows)
    return out


def spans(doc: Doc, para: Para) -> Iterator[tuple[str, dict[str, str]]]:
    """(text, text properties) runs of a paragraph, paragraph style first, span style on top."""
    base = style_props(doc, para.style)

    def walk(node: ET.Element, props: dict[str, str]) -> Iterator[tuple[str, dict[str, str]]]:
        if node.text:
            yield node.text, props
        for child in node:
            if child.tag == q("text:span"):
                yield from walk(child, {**props, **style_props(doc, child.get(q("text:style-name")))})
            elif child.tag == q("text:s"):
                yield " " * int(child.get(q("text:c"), "1")), props
            elif child.tag in (q("text:note"), q("office:annotation")):
                pass
            elif child.tag == q("text:a"):
                yield from walk(child, props)
            else:
                yield from walk(child, props)
            if child.tail:
                yield child.tail, props

    yield from walk(para.node, base)


# ------------------------------------------------------------------ presentations


def slides(doc: Doc) -> list[dict[str, Any]]:
    """Slides in order: name, title text, all text, and speaker-notes text."""
    out = []
    for page in body(doc, "presentation").findall("draw:page", NS):
        title = ""
        texts = []
        notes_text = ""
        for frame in page.findall("draw:frame", NS):
            content = norm(" ".join(text_of(p) for p in frame.iter(q("text:p"))))
            if frame.get(q("presentation:class")) == "title" and not title:
                title = content
            if content:
                texts.append(content)
        notes_node = page.find("presentation:notes", NS)
        if notes_node is not None:
            parts = []
            for frame in notes_node.findall("draw:frame", NS):
                if frame.get(q("presentation:class")) == "notes":
                    parts.append(norm(" ".join(text_of(p) for p in frame.iter(q("text:p")))))
            notes_text = norm(" ".join(parts))
        out.append({"name": page.get(q("draw:name")), "title": title, "texts": texts, "notes": notes_text})
    return out
