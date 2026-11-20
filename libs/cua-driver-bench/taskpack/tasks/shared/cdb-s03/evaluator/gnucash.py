"""Bounded logical reader for the small GnuCash XML used by CDB-S03."""

from __future__ import annotations

import gzip
import io
import re
import xml.etree.ElementTree as ET
from datetime import datetime, timezone
from fractions import Fraction
from pathlib import Path

MAX_COMPRESSED_BYTES = 4 * 1024 * 1024
MAX_XML_BYTES = 8 * 1024 * 1024
MAX_ELEMENTS = 20_000
MAX_TRANSACTIONS = 2_000
MAX_SPLITS = 64
MAX_TEXT = 16_384

NS = {
    "act": "http://www.gnucash.org/XML/act",
    "book": "http://www.gnucash.org/XML/book",
    "cmdty": "http://www.gnucash.org/XML/cmdty",
    "gnc": "http://www.gnucash.org/XML/gnc",
    "split": "http://www.gnucash.org/XML/split",
    "trn": "http://www.gnucash.org/XML/trn",
    "ts": "http://www.gnucash.org/XML/ts",
}


def q(prefix: str, local: str) -> str:
    return "{" + NS[prefix] + "}" + local


def _read_payload(path: Path) -> tuple[bytes, str]:
    if path.is_symlink() or not path.is_file():
        raise ValueError("GnuCash path must be a regular file")
    size = path.stat().st_size
    if size > MAX_COMPRESSED_BYTES:
        raise ValueError("GnuCash file is too large")
    raw = path.read_bytes()
    encoding = "xml"
    if raw.startswith(b"\x1f\x8b"):
        encoding = "gzip"
        with gzip.GzipFile(fileobj=io.BytesIO(raw)) as stream:
            payload = stream.read(MAX_XML_BYTES + 1)
    else:
        payload = raw
    if len(payload) > MAX_XML_BYTES:
        raise ValueError("expanded GnuCash XML is too large")
    upper = payload.upper()
    if b"<!DOCTYPE" in upper or b"<!ENTITY" in upper:
        raise ValueError("DTD and entity declarations are not allowed")
    return payload, encoding


def _text(parent: ET.Element, tag: str, *, required: bool = True) -> str:
    children = parent.findall(tag)
    if len(children) != 1:
        if not children and not required:
            return ""
        raise ValueError(f"expected one {tag}")
    value = children[0].text or ""
    if len(value) > MAX_TEXT:
        raise ValueError("GnuCash text value is too long")
    return value.strip()


def _fraction(value: str) -> Fraction:
    if not re.fullmatch(r"-?\d{1,18}/[1-9]\d{0,17}", value):
        raise ValueError("invalid GnuCash rational")
    numerator, denominator = value.split("/", 1)
    return Fraction(int(numerator), int(denominator))


def _timestamp(value: str) -> str:
    match = re.fullmatch(
        r"(\d{4}-\d{2}-\d{2})[ T](\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?)\s+(Z|[+-]\d{2}:?\d{2})",
        value,
    )
    if not match:
        raise ValueError("invalid GnuCash timestamp")
    offset = match.group(3)
    if offset == "Z":
        offset = "+00:00"
    elif ":" not in offset:
        offset = offset[:3] + ":" + offset[3:]
    try:
        parsed = datetime.fromisoformat(f"{match.group(1)}T{match.group(2)}{offset}")
    except ValueError as error:
        raise ValueError("invalid GnuCash timestamp") from error
    return parsed.astimezone(timezone.utc).isoformat()


def _commodity(node: ET.Element) -> tuple[str, str]:
    space = _text(node, q("cmdty", "space")).upper()
    identifier = _text(node, q("cmdty", "id")).upper()
    if space in {"CURRENCY", "ISO4217"}:
        space = "ISO4217"
    return space, identifier


def _is_template_commodity(value: tuple[str, str]) -> bool:
    return value[0].casefold() == "template" and value[1].casefold() == "template"


def read_book(path: Path) -> dict:
    payload, encoding = _read_payload(path)
    root = ET.fromstring(payload)
    elements = list(root.iter())
    if len(elements) > MAX_ELEMENTS:
        raise ValueError("GnuCash XML contains too many elements")
    books = root.findall(q("gnc", "book"))
    if len(books) != 1:
        raise ValueError("expected exactly one GnuCash book")
    book = books[0]

    accounts: dict[str, dict] = {}
    account_nodes = book.findall(q("gnc", "account"))
    for node in account_nodes:
        guid = _text(node, q("act", "id"))
        if not re.fullmatch(r"[0-9a-fA-F]{32}", guid) or guid in accounts:
            raise ValueError("invalid or duplicate account GUID")
        scu_text = _text(node, q("act", "commodity-scu"), required=False)
        if scu_text and (not scu_text.isdigit() or not 1 <= int(scu_text) <= 1_000_000):
            raise ValueError("invalid account commodity SCU")
        accounts[guid] = {
            "name": _text(node, q("act", "name")),
            "type": _text(node, q("act", "type")),
            "parent": _text(node, q("act", "parent"), required=False),
            "commodity": tuple(sorted(_commodity(item) for item in node.findall(q("act", "commodity")))),
            "commodity_scu": int(scu_text) if scu_text else None,
        }

    transactions: dict[str, dict] = {}
    transaction_nodes = book.findall(q("gnc", "transaction"))
    if len(transaction_nodes) > MAX_TRANSACTIONS:
        raise ValueError("too many GnuCash transactions")
    for node in transaction_nodes:
        guid = _text(node, q("trn", "id"))
        number = _text(node, q("trn", "num"))
        if not re.fullmatch(r"[0-9a-fA-F]{32}", guid) or not number or number in transactions:
            raise ValueError("invalid GUID or duplicate transaction number")
        split_parents = node.findall(q("trn", "splits"))
        if len(split_parents) != 1:
            raise ValueError(f"{number} must have one split collection")
        split_nodes = split_parents[0].findall(q("trn", "split"))
        if not (2 <= len(split_nodes) <= MAX_SPLITS):
            raise ValueError(f"{number} has invalid split count")
        splits = []
        seen_split_ids = set()
        for split in split_nodes:
            split_id = _text(split, q("split", "id"))
            account = _text(split, q("split", "account"))
            if split_id in seen_split_ids or account not in accounts:
                raise ValueError(f"{number} has duplicate split or unknown account")
            seen_split_ids.add(split_id)
            state = _text(split, q("split", "reconciled-state"))
            if state not in {"n", "c", "y", "f", "v"}:
                raise ValueError(f"{number} has invalid reconciliation state")
            splits.append({
                "id": split_id,
                "account": account,
                "value": _fraction(_text(split, q("split", "value"))),
                "quantity": _fraction(_text(split, q("split", "quantity"))),
                "state": state,
                "memo": _text(split, q("split", "memo"), required=False),
                "reconcile_date": (
                    _timestamp(_text(split, q("split", "reconcile-date") + "/" + q("ts", "date")))
                    if split.findall(q("split", "reconcile-date") + "/" + q("ts", "date"))
                    else ""
                ),
            })
        currency_nodes = node.findall(q("trn", "currency"))
        if len(currency_nodes) != 1:
            raise ValueError(f"{number} has invalid currency")
        transactions[number] = {
            "guid": guid,
            "description": _text(node, q("trn", "description")),
            "posted": _timestamp(_text(node, q("trn", "date-posted") + "/" + q("ts", "date"))),
            "entered": _timestamp(_text(node, q("trn", "date-entered") + "/" + q("ts", "date"))),
            "currency": _commodity(currency_nodes[0]),
            "splits": splits,
            "balanced": sum((item["value"] for item in splits), Fraction()) == 0,
        }
    return {
        "encoding": encoding,
        "accounts": accounts,
        "transactions": transactions,
        "book_core": (
            _text(book, q("book", "id")),
            tuple(sorted(
                commodity
                for commodity in (_commodity(node) for node in book.findall(q("gnc", "commodity")))
                if not _is_template_commodity(commodity)
            )),
        ),
    }
