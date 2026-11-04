"""Small ODS logical-cell reader used by the CDB-S02 evaluator."""

from __future__ import annotations

import re
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

NS = {
    "office": "urn:oasis:names:tc:opendocument:xmlns:office:1.0",
    "table": "urn:oasis:names:tc:opendocument:xmlns:table:1.0",
    "text": "urn:oasis:names:tc:opendocument:xmlns:text:1.0",
}
TABLE = "{" + NS["table"] + "}"
OFFICE = "{" + NS["office"] + "}"
MAX_CONTENT_BYTES = 2 * 1024 * 1024
MAX_TABLES = 16
MAX_ROWS = 5000
MAX_COLUMNS = 256


def _time_value(value: str) -> str | None:
    match = re.fullmatch(r"PT(?:(\d+)H)?(?:(\d+)M)?(?:(\d+(?:\.\d+)?)S)?", value)
    if not match:
        return None
    hour = int(match.group(1) or "0")
    minute = int(match.group(2) or "0")
    seconds = float(match.group(3) or "0")
    if hour > 23 or minute > 59 or seconds >= 60:
        return None
    return f"{hour:02d}:{minute:02d}" if seconds == 0 else f"{hour:02d}:{minute:02d}:{seconds:02g}"


def _text(cell: ET.Element) -> str:
    if cell.get(OFFICE + "value-type") == "time":
        logical = cell.get(OFFICE + "time-value")
        normalized = _time_value(logical or "")
        if normalized is not None:
            return normalized
    paragraphs = []
    for node in cell.findall(".//text:p", NS):
        value = "".join(node.itertext())
        paragraphs.append(re.sub(r"\s+", " ", value).strip())
    if paragraphs:
        return "\n".join(paragraphs).strip()
    for key in ("string-value", "value", "date-value", "time-value"):
        value = cell.get("{" + NS["office"] + "}" + key)
        if value is not None:
            return value
    return ""


def read_tables(path: Path) -> dict[str, list[list[str]]]:
    with zipfile.ZipFile(path) as archive:
        content_members = [info for info in archive.infolist() if info.filename == "content.xml"]
        if len(content_members) != 1:
            raise ValueError("ODS must contain exactly one content.xml")
        if content_members[0].file_size > MAX_CONTENT_BYTES:
            raise ValueError("ODS content.xml is too large")
        root = ET.fromstring(archive.read(content_members[0]))
    result: dict[str, list[list[str]]] = {}
    tables = root.findall(".//table:table", NS)
    if len(tables) > MAX_TABLES:
        raise ValueError("ODS contains too many tables")
    total_rows = 0
    for table in tables:
        name = table.get(TABLE + "name", "")
        rows: list[list[str]] = []
        pending_empty_rows = 0
        for row in table.findall(".//table:table-row", NS):
            row_repeat = int(row.get(TABLE + "number-rows-repeated", "1"))
            if row_repeat < 1:
                raise ValueError("invalid repeated-row count")
            cells: list[str] = []
            pending_empty = 0
            for cell in list(row):
                if cell.tag not in {TABLE + "table-cell", TABLE + "covered-table-cell"}:
                    continue
                repeat = int(cell.get(TABLE + "number-columns-repeated", "1"))
                if repeat < 1:
                    raise ValueError("ODS row contains too many logical columns")
                value = _text(cell)
                if value == "":
                    pending_empty += repeat
                    continue
                if repeat > MAX_COLUMNS or len(cells) + pending_empty + repeat > MAX_COLUMNS:
                    raise ValueError("ODS row contains too many logical columns")
                cells.extend([""] * pending_empty)
                pending_empty = 0
                cells.extend([value] * repeat)
            if not cells:
                pending_empty_rows += row_repeat
                continue
            if row_repeat > 1000:
                raise ValueError("invalid repeated-row count")
            total_rows += pending_empty_rows + row_repeat
            if total_rows > MAX_ROWS:
                raise ValueError("ODS contains too many logical rows")
            rows.extend([[] for _ in range(pending_empty_rows)])
            pending_empty_rows = 0
            for _ in range(row_repeat):
                rows.append(list(cells))
        result[name] = rows
    return result


def keyed_rows(rows: list[list[str]]) -> dict[str, list[str]]:
    return {row[0]: row[1:] for row in rows[1:] if row and row[0]}
