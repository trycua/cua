"""Adversarial evaluator, fixture, and reset tests for CDB-S03."""

from __future__ import annotations

import gzip
import hashlib
import importlib.util
import json
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
import zipfile
from datetime import datetime, timedelta, timezone
from decimal import Decimal
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
SETUP = BUNDLE / "reset" / "setup.py"
VERIFY = BUNDLE / "reset" / "verify.py"
EVALUATE = BUNDLE / "evaluator" / "evaluate.py"
BUILDER = BUNDLE / "tools" / "build_fixture_ods.py"
FAILURES = BUNDLE / "reset" / "failure-fixtures"

NS = {
    "act": "http://www.gnucash.org/XML/act",
    "book": "http://www.gnucash.org/XML/book",
    "cd": "http://www.gnucash.org/XML/cd",
    "cmdty": "http://www.gnucash.org/XML/cmdty",
    "gnc": "http://www.gnucash.org/XML/gnc",
    "trn": "http://www.gnucash.org/XML/trn",
    "split": "http://www.gnucash.org/XML/split",
    "slot": "http://www.gnucash.org/XML/slot",
    "ts": "http://www.gnucash.org/XML/ts",
    "table": "urn:oasis:names:tc:opendocument:xmlns:table:1.0",
    "office": "urn:oasis:names:tc:opendocument:xmlns:office:1.0",
    "text": "urn:oasis:names:tc:opendocument:xmlns:text:1.0",
}
for prefix, uri in NS.items():
    ET.register_namespace(prefix, uri)
TABLE = "{" + NS["table"] + "}"
OFFICE = "{" + NS["office"] + "}"
TEXT = "{" + NS["text"] + "}"


def run(command, check=True):
    completed = subprocess.run(command, capture_output=True, text=True)
    if check and completed.returncode != 0:
        raise AssertionError(f"command failed: {command}\n{completed.stdout}\n{completed.stderr}")
    return completed


def transaction(root: ET.Element, number: str) -> ET.Element:
    for item in root.findall(".//gnc:transaction", NS):
        if item.findtext("trn:num", namespaces=NS) == number:
            return item
    raise KeyError(number)


def checking_split(item: ET.Element) -> ET.Element:
    for split in item.findall("trn:splits/trn:split", NS):
        if split.findtext("split:account", namespaces=NS) == "20000000000000000000000000000003":
            return split
    raise KeyError("checking split")


def set_split_memo(split: ET.Element, value: str) -> None:
    memo = split.find("split:memo", NS)
    if memo is None:
        memo = ET.SubElement(split, "{" + NS["split"] + "}memo")
    memo.text = value


def set_reconciled(split: ET.Element, *, with_date: bool = False) -> None:
    split.find("split:reconciled-state", NS).text = "y"
    if with_date:
        container = ET.SubElement(split, "{" + NS["split"] + "}reconcile-date")
        ET.SubElement(container, "{" + NS["ts"] + "}date").text = "2026-10-31 12:00:00 +0000"


def read_book_xml(path: Path) -> ET.Element:
    payload = path.read_bytes()
    if payload.startswith(b"\x1f\x8b"):
        payload = gzip.decompress(payload)
    return ET.fromstring(payload)


def write_book_xml(path: Path, root: ET.Element, *, compressed: bool = False) -> None:
    payload = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    path.write_bytes(gzip.compress(payload, mtime=0) if compressed else payload)


def rewrite_ods(path: Path, transform) -> None:
    with zipfile.ZipFile(path) as source:
        members = [(info, source.read(info.filename)) for info in source.infolist()]
    payloads = {info.filename: data for info, data in members}
    root = ET.fromstring(payloads["content.xml"])
    transform(root)
    rendered = ET.tostring(root, encoding="utf-8", xml_declaration=True)
    with zipfile.ZipFile(path, "w") as target:
        for info, data in members:
            target.writestr(info, rendered if info.filename == "content.xml" else data)


def ods_rows(root: ET.Element) -> list[ET.Element]:
    sheet = next(node for node in root.findall(".//table:table", NS) if node.get(TABLE + "name") == "Reconciliation")
    return sheet.findall("table:table-row", NS)


def cell_value(cell: ET.Element) -> str:
    if cell.get(OFFICE + "value-type") in {"float", "currency", "percentage"}:
        return cell.get(OFFICE + "value", "")
    return "".join(cell.itertext()).strip()


def set_cell(cell: ET.Element, value: str, *, numeric: bool = False) -> None:
    formula = cell.get(TABLE + "formula")
    style = cell.get(TABLE + "style-name")
    cell.attrib.clear()
    if style:
        cell.set(TABLE + "style-name", style)
    if formula:
        cell.set(TABLE + "formula", formula)
    if numeric:
        cell.set(OFFICE + "value-type", "currency")
        cell.set(OFFICE + "currency", "USD")
        cell.set(OFFICE + "value", value)
    else:
        cell.set(OFFICE + "value-type", "string")
    for child in list(cell):
        cell.remove(child)
    paragraph = ET.SubElement(cell, TEXT + "p")
    paragraph.text = value


def make_native_gnucash_shape(root: ET.Element) -> None:
    for space in root.findall(".//cmdty:space", NS):
        if (space.text or "").upper() == "ISO4217":
            space.text = "CURRENCY"
    book = root.find("gnc:book", NS)
    template = ET.Element("{" + NS["gnc"] + "}commodity", {"version": "2.0.0"})
    ET.SubElement(template, "{" + NS["cmdty"] + "}space").text = "template"
    ET.SubElement(template, "{" + NS["cmdty"] + "}id").text = "template"
    book.insert(1, template)
    count = ET.SubElement(book, "{" + NS["gnc"] + "}count-data", {"{" + NS["cd"] + "}type": "commodity"})
    count.text = "2"
    book_slots = ET.SubElement(book, "{" + NS["book"] + "}slots")
    slot = ET.SubElement(book_slots, "slot")
    ET.SubElement(slot, "{" + NS["slot"] + "}key").text = "native-save-marker"
    ET.SubElement(slot, "{" + NS["slot"] + "}value", {"type": "string"}).text = "gnucash"
    for item in root.findall(".//gnc:transaction", NS):
        for date in item.findall(".//ts:date", NS):
            parsed = datetime.strptime(date.text, "%Y-%m-%d %H:%M:%S %z")
            local = parsed.astimezone(timezone(timedelta(hours=-5)))
            date.text = local.strftime("%Y-%m-%d %H:%M:%S -05:00")
        slots = ET.SubElement(item, "{" + NS["trn"] + "}slots")
        slot = ET.SubElement(slots, "slot")
        ET.SubElement(slot, "{" + NS["slot"] + "}key").text = "date-posted-source"
        ET.SubElement(slot, "{" + NS["slot"] + "}value", {"type": "string"}).text = "native"


def make_native_libreoffice_shape(root: ET.Element) -> None:
    for cell in root.findall(".//table:table-cell", NS):
        raw = cell.get(OFFICE + "value")
        if raw is not None:
            value = Decimal(raw)
            cell.set(OFFICE + "value", f"{value:.4f}")
        formula = cell.get(TABLE + "formula")
        if formula:
            native = "of:=" + formula
            native = native.replace("Reconciliation.", "'Reconciliation'.")
            native = re.sub(r"([.:])([A-Z])(\d+)", r"\1$\2$\3", native)
            cell.set(TABLE + "formula", " " + native + " ")
    reconciliation = next(
        node for node in root.findall(".//table:table", NS) if node.get(TABLE + "name") == "Reconciliation"
    )
    rows = reconciliation.findall("table:table-row", NS)
    for row in rows[1:]:
        cells = list(row)
        if len(cells) >= 9 and cells[8].get(TABLE + "formula"):
            book = Decimal(cells[3].get(OFFICE + "value", "0"))
            source = cells[5].get(OFFICE + "value")
            result = book - Decimal(source) if source is not None else Decimal(0)
            cells[8].set(OFFICE + "value", f"{result:.4f}")
    controls = next(node for node in root.findall(".//table:table", NS) if node.get(TABLE + "name") == "Controls")
    cached_controls = {"Ledger row count": "6.0000", "Ledger control total": "3088.2500", "Matched row count": "2.0000"}
    for row in controls.findall("table:table-row", NS):
        cells = list(row)
        if cells and cell_value(cells[0]) in cached_controls:
            cells[1].set(OFFICE + "value", cached_controls[cell_value(cells[0])])
    for row in rows:
        trailing = ET.SubElement(row, TABLE + "table-cell")
        trailing.set(TABLE + "number-columns-repeated", "1024")
        trailing.set(OFFICE + "value-type", "string")
    trailing_row = ET.SubElement(reconciliation, TABLE + "table-row")
    trailing_row.set(TABLE + "number-rows-repeated", "1048568")
    trailing_cell = ET.SubElement(trailing_row, TABLE + "table-cell")
    trailing_cell.set(TABLE + "number-columns-repeated", "16384")
    trailing_cell.set(OFFICE + "value-type", "string")


FINAL_ROWS = {
    "GNC-512": {4: "OCP-4821", 5: "1248.60", 6: "matched", 7: "OCP-4821"},
    "GNC-514": {4: "OCP-4827", 5: "846.50", 6: "discrepancy", 7: "DISCREPANCY: OCP-4827 amount mismatch; book $864.50 vs source $846.50"},
    "GNC-518": {4: "SVL-119", 5: "392.40", 6: "matched", 7: "SVL-119"},
    "GNC-519": {4: "SVL-119", 5: "392.40", 6: "duplicate", 7: "DUPLICATE: SVL-119 already matched to GNC-518; left unreconciled"},
    "GNC-521": {4: "", 5: "", 6: "unmatched", 7: "No source invoice found"},
}


class Workspace:
    def __init__(self, root: Path):
        self.root = root / "workspace"
        self.artifacts = root / "artifacts"
        self.artifacts.mkdir()
        run([sys.executable, str(SETUP), "--workspace", str(self.root)])

    @property
    def book(self) -> Path:
        return self.root / "book" / "kestrel-wharf.gnucash"

    @property
    def workbook(self) -> Path:
        return self.root / "planning" / "kestrel-wharf-reconciliation.ods"

    def complete_book(self, *, compressed: bool = False, dates: bool = False) -> None:
        root = read_book_xml(self.book)
        for number in ("GNC-512", "GNC-518"):
            set_reconciled(checking_split(transaction(root, number)), with_date=dates)
        set_split_memo(checking_split(transaction(root, "GNC-514")), FINAL_ROWS["GNC-514"][7])
        set_split_memo(checking_split(transaction(root, "GNC-519")), FINAL_ROWS["GNC-519"][7])
        write_book_xml(self.book, root, compressed=compressed)

    def complete_workbook(self) -> None:
        def transform(root):
            rows = ods_rows(root)
            by_id = {cell_value(list(row)[0]): row for row in rows[1:] if list(row)}
            for number, updates in FINAL_ROWS.items():
                cells = list(by_id[number])
                for column, value in updates.items():
                    set_cell(cells[column], value, numeric=column == 5 and bool(value))
            blank = rows[-1]
            cells = list(blank)
            for column, value in enumerate(["NAR-773", "", "Nacre Audio Repair", "", "NAR-773", "275.00", "unmatched", "No ledger payment found"]):
                set_cell(cells[column], value, numeric=column == 5)
        rewrite_ods(self.workbook, transform)

    def complete(self, *, compressed=False, dates=False) -> None:
        self.complete_book(compressed=compressed, dates=dates)
        self.complete_workbook()

    def evaluate(self) -> dict:
        result = self.root.parent / "result.json"
        completed = run([
            sys.executable, str(EVALUATE), "--workspace", str(self.root), "--artifacts", str(self.artifacts),
            "--result", str(result), "--agent-exit-code", "0",
        ])
        self.last_stderr = completed.stderr
        return json.loads(result.read_text())


class EvaluatorCase(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cdb-s03-test-")
        self.ws = Workspace(Path(self.temp.name))

    def tearDown(self):
        self.temp.cleanup()

    def check(self, result, name, expected):
        self.assertEqual(result["detail"]["checks"][name]["passed"], expected, result["detail"]["checks"][name])

    def test_correct_reconciliation_passes(self):
        self.ws.complete()
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:8000])
        self.assertEqual(result["score"], 1.0)

    def test_gzip_real_save_and_reconcile_dates_pass(self):
        self.ws.complete(compressed=True, dates=True)
        self.assertTrue(self.ws.evaluate()["passed"])

    def test_gnucash_native_shape_is_logically_equivalent(self):
        self.ws.complete(dates=True)
        root = read_book_xml(self.ws.book)
        make_native_gnucash_shape(root)
        write_book_xml(self.ws.book, root, compressed=True)
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:8000])

    def test_libreoffice_native_shape_is_logically_equivalent(self):
        self.ws.complete()
        rewrite_ods(self.ws.workbook, make_native_libreoffice_shape)
        result = self.ws.evaluate()
        self.assertTrue(result["passed"], json.dumps(result, indent=2)[:8000])

    def test_xml_reordering_does_not_break_logical_preservation(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        book = root.find("gnc:book", NS)
        transactions = book.findall("gnc:transaction", NS)
        for item in transactions:
            book.remove(item)
        for item in reversed(transactions):
            book.append(item)
        write_book_xml(self.ws.book, root)
        self.assertTrue(self.ws.evaluate()["passed"])

    def test_changed_timestamp_instant_fails_preservation(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        transaction(root, "GNC-521").find("trn:date-posted/ts:date", NS).text = "2026-10-09 12:01:00 +0000"
        write_book_xml(self.ws.book, root)
        result = self.ws.evaluate()
        self.check(result, "unmatched_preserved", False)
        self.check(result, "preservation", False)

    def test_non_template_commodity_is_not_ignored(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        book = root.find("gnc:book", NS)
        commodity = ET.Element("{" + NS["gnc"] + "}commodity", {"version": "2.0.0"})
        ET.SubElement(commodity, "{" + NS["cmdty"] + "}space").text = "CRYPTO"
        ET.SubElement(commodity, "{" + NS["cmdty"] + "}id").text = "BTC"
        book.insert(1, commodity)
        write_book_xml(self.ws.book, root)
        self.check(self.ws.evaluate(), "preservation", False)

    def test_no_op_fails_but_sources_and_ledger_are_intact(self):
        result = self.ws.evaluate()
        self.assertFalse(result["passed"])
        self.check(result, "ledger_balanced", True)
        self.check(result, "source_preserved", True)
        self.check(result, "preservation", True)
        self.check(result, "supported_matches", False)

    def test_wrong_duplicate_tie_break_fails(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        checking_split(transaction(root, "GNC-518")).find("split:reconciled-state", NS).text = "n"
        checking_split(transaction(root, "GNC-519")).find("split:reconciled-state", NS).text = "y"
        write_book_xml(self.ws.book, root)
        result = self.ws.evaluate()
        self.check(result, "supported_matches", False)
        self.check(result, "exact_discrepancies_flagged", False)

    def test_wrong_mismatch_amount_in_memo_fails(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        set_split_memo(checking_split(transaction(root, "GNC-514")), "DISCREPANCY: OCP-4827 amount mismatch; book $864.50 vs source $864.50")
        write_book_xml(self.ws.book, root)
        self.check(self.ws.evaluate(), "exact_discrepancies_flagged", False)

    def test_exception_memo_on_nonchecking_split_fails(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        item = transaction(root, "GNC-514")
        checking = checking_split(item)
        checking.remove(checking.find("split:memo", NS))
        other = next(split for split in item.findall("trn:splits/trn:split", NS) if split is not checking)
        set_split_memo(other, FINAL_ROWS["GNC-514"][7])
        write_book_xml(self.ws.book, root)
        self.check(self.ws.evaluate(), "exact_discrepancies_flagged", False)

    def test_exact_match_memo_injection_fails(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        set_split_memo(checking_split(transaction(root, "GNC-512")), "unrequested note")
        write_book_xml(self.ws.book, root)
        self.check(self.ws.evaluate(), "supported_matches", False)

    def test_template_punctuation_and_spacing_are_normalized(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        set_split_memo(checking_split(transaction(root, "GNC-514")), "Discrepancy — OCP-4827 amount mismatch; book $864.50, vs source $846.50")
        write_book_xml(self.ws.book, root)
        self.assertTrue(self.ws.evaluate()["passed"])

    def test_unbalanced_transaction_fails(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        checking_split(transaction(root, "GNC-512")).find("split:value", NS).text = "-124859/100"
        write_book_xml(self.ws.book, root)
        self.check(self.ws.evaluate(), "ledger_balanced", False)

    def test_amount_and_split_tampering_fail_preservation(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        target = transaction(root, "GNC-514")
        first = target.findall("trn:splits/trn:split", NS)[0]
        first.find("split:value", NS).text = "86550/100"
        first.find("split:quantity", NS).text = "86550/100"
        checking = checking_split(target)
        checking.find("split:value", NS).text = "-86550/100"
        checking.find("split:quantity", NS).text = "-86550/100"
        write_book_xml(self.ws.book, root)
        result = self.ws.evaluate()
        self.check(result, "ledger_balanced", True)
        self.check(result, "preservation", False)
        self.check(result, "exact_discrepancies_flagged", False)

    def test_ledger_only_record_must_remain_exact(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        set_split_memo(checking_split(transaction(root, "GNC-521")), "invented source")
        write_book_xml(self.ws.book, root)
        result = self.ws.evaluate()
        self.check(result, "unmatched_preserved", False)
        self.check(result, "preservation", False)

    def test_source_only_record_must_not_be_created(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        transaction(root, "GNC-406").find("trn:num", NS).text = "NAR-773"
        write_book_xml(self.ws.book, root)
        result = self.ws.evaluate()
        self.check(result, "unmatched_preserved", False)
        self.check(result, "preservation", False)

    def test_decoy_state_and_memo_are_protected(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        decoy = checking_split(transaction(root, "GNC-406"))
        decoy.find("split:reconciled-state", NS).text = "y"
        set_split_memo(decoy, "changed")
        write_book_xml(self.ws.book, root)
        self.check(self.ws.evaluate(), "preservation", False)

    def test_reconcile_date_on_exception_fails(self):
        self.ws.complete()
        root = read_book_xml(self.ws.book)
        split = checking_split(transaction(root, "GNC-514"))
        container = ET.SubElement(split, "{" + NS["split"] + "}reconcile-date")
        ET.SubElement(container, "{" + NS["ts"] + "}date").text = "2026-10-31 12:00:00 +0000"
        write_book_xml(self.ws.book, root)
        self.check(self.ws.evaluate(), "exact_discrepancies_flagged", False)

    def test_source_only_workbook_row_is_required(self):
        self.ws.complete()
        rewrite_ods(self.ws.workbook, lambda root: set_cell(list(ods_rows(root)[-1])[0], ""))
        self.check(self.ws.evaluate(), "workbook_agreement", False)

    def test_source_only_counterparty_accepts_source_case(self):
        self.ws.complete()
        rewrite_ods(
            self.ws.workbook,
            lambda root: set_cell(list(ods_rows(root)[-1])[2], "NACRE AUDIO REPAIR"),
        )
        self.check(self.ws.evaluate(), "workbook_agreement", True)

    def test_workbook_control_change_fails(self):
        self.ws.complete()
        def transform(root):
            controls = next(node for node in root.findall(".//table:table", NS) if node.get(TABLE + "name") == "Controls")
            set_cell(list(controls.findall("table:table-row", NS)[1])[1], "EUR")
        rewrite_ods(self.ws.workbook, transform)
        self.check(self.ws.evaluate(), "workbook_agreement", False)

    def test_workbook_formula_change_fails(self):
        self.ws.complete()
        def transform(root):
            list(ods_rows(root)[1])[-1].set(TABLE + "formula", "of:=1+1")
        rewrite_ods(self.ws.workbook, transform)
        self.check(self.ws.evaluate(), "workbook_agreement", False)

    def test_workbook_numeric_tamper_fails_despite_decimal_normalization(self):
        self.ws.complete()
        def transform(root):
            row = next(row for row in ods_rows(root)[1:] if cell_value(list(row)[0]) == "GNC-512")
            list(row)[3].set(OFFICE + "value", "1248.6100")
        rewrite_ods(self.ws.workbook, transform)
        self.check(self.ws.evaluate(), "workbook_agreement", False)

    def test_mail_mutation_and_extra_file_fail(self):
        self.ws.complete()
        messages = self.ws.root / "mail" / "messages.json"
        messages.write_bytes(messages.read_bytes() + b" ")
        (self.ws.root / "mail" / "extra.txt").write_text("extra")
        self.check(self.ws.evaluate(), "source_preserved", False)

    def test_benign_os_metadata_does_not_mutate_mail_source(self):
        self.ws.complete()
        for name in (".DS_Store", "Thumbs.db", "desktop.ini"):
            (self.ws.root / "mail" / name).write_bytes(b"host metadata")
        self.check(self.ws.evaluate(), "source_preserved", True)

    def test_os_metadata_symlink_is_still_rejected(self):
        self.ws.complete()
        target = self.ws.root.parent / "outside-metadata"
        target.write_bytes(b"outside")
        link = self.ws.root / "mail" / ".DS_Store"
        try:
            link.symlink_to(target)
        except OSError as error:
            self.skipTest(f"symlink creation unavailable: {error}")
        self.check(self.ws.evaluate(), "source_preserved", False)

    def test_corrupt_inputs_are_graded_not_crashes(self):
        self.ws.book.write_bytes(b"not XML")
        self.ws.workbook.write_bytes(b"not ODS")
        result = self.ws.evaluate()
        self.assertEqual(self.ws.last_stderr, "")
        self.assertFalse(result["passed"])
        self.check(result, "ledger_balanced", False)
        self.check(result, "workbook_agreement", False)

    def test_dtd_and_entity_input_is_rejected(self):
        self.ws.book.write_text('<?xml version="1.0"?><!DOCTYPE x [<!ENTITY a "x">]><x>&a;</x>')
        self.check(self.ws.evaluate(), "ledger_balanced", False)

    def test_late_dtd_declaration_is_rejected(self):
        self.ws.book.write_bytes(b" " * 5000 + b"<!DOCTYPE gnucash-v2><gnucash-v2/>")
        self.check(self.ws.evaluate(), "ledger_balanced", False)

    def test_primary_book_symlink_is_rejected(self):
        target = self.ws.root.parent / "outside.gnucash"
        target.write_bytes(self.ws.book.read_bytes())
        self.ws.book.unlink()
        try:
            self.ws.book.symlink_to(target)
        except OSError as error:
            self.skipTest(f"symlink creation unavailable: {error}")
        self.check(self.ws.evaluate(), "ledger_balanced", False)

    def test_oversized_gzip_is_bounded_failure(self):
        self.ws.book.write_bytes(gzip.compress(b" " * (8 * 1024 * 1024 + 1), mtime=0))
        self.check(self.ws.evaluate(), "ledger_balanced", False)

    def test_repeated_column_bomb_is_rejected(self):
        def transform(root):
            list(ods_rows(root)[1])[0].set(TABLE + "number-columns-repeated", "999999999")
        rewrite_ods(self.ws.workbook, transform)
        self.check(self.ws.evaluate(), "workbook_agreement", False)

    def test_nonblank_repeated_row_bomb_is_rejected(self):
        def transform(root):
            ods_rows(root)[1].set(TABLE + "number-rows-repeated", "5001")
        rewrite_ods(self.ws.workbook, transform)
        self.check(self.ws.evaluate(), "workbook_agreement", False)


class FixtureAndResetCase(unittest.TestCase):
    def test_ods_builder_is_deterministic(self):
        fixture = BUNDLE / "fixture" / "planning" / "kestrel-wharf-reconciliation.ods"
        before = hashlib.sha256(fixture.read_bytes()).hexdigest()
        spec = importlib.util.spec_from_file_location("cdb_s03_ods_builder", BUILDER)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory(prefix="cdb-s03-builder-") as temp:
            first_path = Path(temp) / "first.ods"
            second_path = Path(temp) / "second.ods"
            module.build(first_path)
            module.build(second_path)
            first = hashlib.sha256(first_path.read_bytes()).hexdigest()
            second = hashlib.sha256(second_path.read_bytes()).hexdigest()
            self.assertEqual(first, second)
            with zipfile.ZipFile(first_path) as archive:
                self.assertTrue(all(info.create_system == 3 for info in archive.infolist()))
        self.assertEqual(before, hashlib.sha256(fixture.read_bytes()).hexdigest())

    def test_reset_detects_stale_extra_book_sidecar(self):
        with tempfile.TemporaryDirectory(prefix="cdb-s03-reset-") as temp:
            workspace = Path(temp) / "workspace"
            run([sys.executable, str(SETUP), "--workspace", str(workspace)])
            shutil.copyfile(FAILURES / "stale-book.txt", workspace / "book" / "stale-book.txt")
            result = run([sys.executable, str(VERIFY), "--workspace", str(workspace)], check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("book/stale-book.txt", result.stdout)

    def test_reset_detects_stale_workbook_replacement(self):
        with tempfile.TemporaryDirectory(prefix="cdb-s03-reset-") as temp:
            workspace = Path(temp) / "workspace"
            run([sys.executable, str(SETUP), "--workspace", str(workspace)])
            shutil.copyfile(FAILURES / "stale-workbook.txt", workspace / "planning" / "kestrel-wharf-reconciliation.ods")
            result = run([sys.executable, str(VERIFY), "--workspace", str(workspace)], check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("planning/kestrel-wharf-reconciliation.ods", result.stdout)


if __name__ == "__main__":
    unittest.main()
