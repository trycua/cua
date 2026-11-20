"""Bounded logical-cell ODS reader for CDB-S03."""

from __future__ import annotations

import re
import xml.etree.ElementTree as ET
import zipfile
from dataclasses import dataclass
from decimal import Decimal, InvalidOperation
from pathlib import Path

NS = {
    "office": "urn:oasis:names:tc:opendocument:xmlns:office:1.0",
    "table": "urn:oasis:names:tc:opendocument:xmlns:table:1.0",
    "text": "urn:oasis:names:tc:opendocument:xmlns:text:1.0",
}
TABLE = "{" + NS["table"] + "}"
OFFICE = "{" + NS["office"] + "}"
MAX_ARCHIVE_MEMBERS = 64
MAX_CONTENT_BYTES = 3 * 1024 * 1024
MAX_TABLES = 16
MAX_ROWS = 5_000
MAX_COLUMNS = 128
MAX_REPEAT = 1_000
MAX_BLANK_ROW_REPEAT = 1_048_576
MAX_BLANK_COLUMN_REPEAT = 16_384
MAX_CELL_TEXT = 16_384


@dataclass(frozen=True)
class Cell:
    value: str
    formula: str = ""
    value_type: str = ""
    number: Decimal | None = None


def _number(value: str) -> Decimal:
    if not re.fullmatch(r"-?\d{1,18}(?:\.\d{1,12})?(?:[eE][+-]?\d{1,3})?", value):
        raise ValueError("invalid numeric ODS cell")
    try:
        number = Decimal(value)
    except InvalidOperation as error:
        raise ValueError("invalid numeric ODS cell") from error
    if not number.is_finite() or number.adjusted() > 18 or number.adjusted() < -12:
        raise ValueError("numeric ODS cell is out of bounds")
    return Decimal(0) if number == 0 else number.normalize()


def _cell(cell: ET.Element) -> Cell:
    value_type = cell.get(OFFICE + "value-type", "")
    formula = cell.get(TABLE + "formula", "")
    if len(formula) > MAX_CELL_TEXT:
        raise ValueError("ODS formula is too long")
    if value_type in {"float", "currency", "percentage"}:
        raw = cell.get(OFFICE + "value", "")
        number = _number(raw)
        value = format(number, "f")
    else:
        number = None
        paragraphs = []
        for paragraph in cell.findall(".//text:p", NS):
            rendered = re.sub(r"\s+", " ", "".join(paragraph.itertext())).strip()
            paragraphs.append(rendered)
        value = "\n".join(paragraphs).strip()
        if not value:
            value = cell.get(OFFICE + "string-value", "")
    if len(value) > MAX_CELL_TEXT:
        raise ValueError("ODS cell text is too long")
    return Cell(value=value, formula=formula, value_type=value_type, number=number)


def read_tables(path: Path) -> dict[str, list[list[Cell]]]:
    if path.is_symlink() or not path.is_file():
        raise ValueError("ODS path must be a regular file")
    with zipfile.ZipFile(path) as archive:
        infos = archive.infolist()
        if len(infos) > MAX_ARCHIVE_MEMBERS or any(info.flag_bits & 1 for info in infos):
            raise ValueError("invalid ODS archive")
        content = [info for info in infos if info.filename == "content.xml"]
        if len(content) != 1 or content[0].file_size > MAX_CONTENT_BYTES:
            raise ValueError("ODS must contain one bounded content.xml")
        payload = archive.read(content[0])
    upper = payload.upper()
    if b"<!DOCTYPE" in upper or b"<!ENTITY" in upper:
        raise ValueError("DTD and entity declarations are not allowed")
    root = ET.fromstring(payload)
    if sum(1 for _ in root.iter()) > 30_000:
        raise ValueError("ODS contains too many XML elements")
    nodes = root.findall(".//table:table", NS)
    if len(nodes) > MAX_TABLES:
        raise ValueError("ODS contains too many sheets")
    result: dict[str, list[list[Cell]]] = {}
    total_rows = 0
    for table in nodes:
        name = table.get(TABLE + "name", "")
        if not name or name in result:
            raise ValueError("ODS has blank or duplicate sheet name")
        rows: list[list[Cell]] = []
        pending_blank_rows = 0
        for row in table.findall(".//table:table-row", NS):
            try:
                repeat_rows = int(row.get(TABLE + "number-rows-repeated", "1"))
            except ValueError as error:
                raise ValueError("invalid repeated row count") from error
            if not 1 <= repeat_rows <= MAX_BLANK_ROW_REPEAT:
                raise ValueError("invalid repeated row count")
            cells: list[Cell] = []
            pending_blank = 0
            for node in list(row):
                if node.tag not in {TABLE + "table-cell", TABLE + "covered-table-cell"}:
                    continue
                try:
                    repeat = int(node.get(TABLE + "number-columns-repeated", "1"))
                except ValueError as error:
                    raise ValueError("invalid repeated column count") from error
                if not 1 <= repeat <= MAX_BLANK_COLUMN_REPEAT:
                    raise ValueError("invalid repeated column count")
                item = _cell(node)
                if item.value == "" and not item.formula:
                    pending_blank += repeat
                    continue
                if len(cells) + pending_blank + repeat > MAX_COLUMNS:
                    raise ValueError("ODS row contains too many columns")
                cells.extend([Cell("")] * pending_blank)
                pending_blank = 0
                cells.extend([item] * repeat)
            if not cells:
                pending_blank_rows += repeat_rows
                continue
            if repeat_rows > MAX_REPEAT:
                raise ValueError("invalid repeated nonblank row count")
            total_rows += pending_blank_rows + repeat_rows
            if total_rows > MAX_ROWS:
                raise ValueError("ODS contains too many rows")
            rows.extend([[] for _ in range(pending_blank_rows)])
            pending_blank_rows = 0
            rows.extend([list(cells) for _ in range(repeat_rows)])
        result[name] = rows
    return result


def values(row: list[Cell], width: int) -> list[str]:
    return [*(cell.value for cell in row), *([""] * width)][:width]
