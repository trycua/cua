#!/usr/bin/env python3
"""Build the deterministic CDB-S03 reconciliation workpaper."""

from __future__ import annotations

import argparse
import hashlib
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape

HERE = Path(__file__).resolve().parent
BUNDLE = HERE.parent
OUTPUT = BUNDLE / "fixture" / "planning" / "kestrel-wharf-reconciliation.ods"
STAMP = (2026, 1, 1, 0, 0, 0)
MIMETYPE = b"application/vnd.oasis.opendocument.spreadsheet"

ROWS = [
    ("GNC-512", "Orchard & Coast Paper", "1248.60"),
    ("GNC-514", "Orchard & Coast Paper", "864.50"),
    ("GNC-518", "Sable Vertex Logistics", "392.40"),
    ("GNC-519", "Sable Vertex Logistics", "392.40"),
    ("GNC-521", "Harbor Rack Supply", "117.25"),
    ("GNC-406", "Old Canal Packaging", "73.10"),
]

CONTROLS = [
    ("Book", "Kestrel Wharf Operations"),
    ("Currency", "USD"),
    ("Review scope", "October 2026 payment review"),
    ("Status vocabulary", "pending | matched | discrepancy | duplicate | unmatched"),
    ("Matching rule", "Vendor and amount must agree; one source invoice may support at most one ledger transaction"),
    ("Reconciliation rule", "Only exact supported matches may be reconciled"),
    ("Evidence rule", "Record the source invoice ID or the exact exception memo"),
]


def text_cell(value: str, style: str = "ce-body") -> str:
    return f'<table:table-cell table:style-name="{style}" office:value-type="string"><text:p>{escape(value)}</text:p></table:table-cell>'


def money_cell(value: str) -> str:
    return f'<table:table-cell table:style-name="ce-money" office:value-type="currency" office:currency="USD" office:value="{value}"><text:p>${float(value):,.2f}</text:p></table:table-cell>'


def formula_cell(row: int) -> str:
    formula = f'of:=IF(ISNUMBER([.F{row}]);[.D{row}]-[.F{row}];0)'
    encoded = escape(formula, {'"': "&quot;"})
    return f'<table:table-cell table:style-name="ce-money" table:formula="{encoded}" office:value-type="currency" office:currency="USD" office:value="0"><text:p>$0.00</text:p></table:table-cell>'


def table_row(cells: list[str], style: str = "ro-body") -> str:
    return f'<table:table-row table:style-name="{style}">{"".join(cells)}</table:table-row>'


def content() -> bytes:
    header = ["Record ID", "Ledger Number", "Counterparty", "Book Amount", "Source Invoice", "Source Amount", "Status", "Evidence / Note", "Difference"]
    reconciliation = [table_row([text_cell(value, "ce-head") for value in header], "ro-head")]
    for row_number, (ledger_id, vendor, amount) in enumerate(ROWS, 2):
        reconciliation.append(table_row([
            text_cell(ledger_id), text_cell(ledger_id), text_cell(vendor), money_cell(amount), text_cell(""), text_cell(""),
            text_cell("pending" if ledger_id != "GNC-406" else "prior-period"),
            text_cell("Prior-period control; do not alter" if ledger_id == "GNC-406" else ""),
            formula_cell(row_number),
        ]))
    reconciliation.append(table_row([
        text_cell(""), text_cell(""), text_cell(""), text_cell(""), text_cell(""), text_cell(""),
        text_cell("pending"), text_cell(""), formula_cell(8),
    ]))
    controls = [table_row([text_cell("Control", "ce-head"), text_cell("Value", "ce-head")], "ro-head")]
    controls.extend(table_row([text_cell(key), text_cell(value)]) for key, value in CONTROLS)
    controls.extend([
        table_row([text_cell("Ledger row count"), '<table:table-cell table:style-name="ce-number" table:formula="of:=COUNTA([Reconciliation.B2:.B8])" office:value-type="float" office:value="6"><text:p>6</text:p></table:table-cell>']),
        table_row([text_cell("Ledger control total"), '<table:table-cell table:style-name="ce-money" table:formula="of:=SUM([Reconciliation.D2:.D8])" office:value-type="currency" office:currency="USD" office:value="3088.25"><text:p>$3,088.25</text:p></table:table-cell>']),
        table_row([text_cell("Matched row count"), '<table:table-cell table:style-name="ce-number" table:formula="of:=COUNTIF([Reconciliation.G2:.G8];&quot;matched&quot;)" office:value-type="float" office:value="0"><text:p>0</text:p></table:table-cell>']),
    ])
    xml = f'''<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:number="urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0" xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" office:version="1.3">
 <office:automatic-styles>
  <number:currency-style style:name="money"><number:currency-symbol>$</number:currency-symbol><number:number number:decimal-places="2" number:min-integer-digits="1" number:grouping="true"/></number:currency-style>
  <style:style style:name="ce-body" style:family="table-cell"><style:text-properties fo:font-family="Liberation Sans" fo:font-size="10pt"/><style:paragraph-properties fo:margin="0.04in"/></style:style>
  <style:style style:name="ce-head" style:family="table-cell" style:parent-style-name="ce-body"><style:table-cell-properties fo:background-color="#244A5A" fo:border-bottom="0.02in solid #16323C"/><style:text-properties fo:color="#FFFFFF" fo:font-weight="bold"/></style:style>
  <style:style style:name="ce-money" style:family="table-cell" style:data-style-name="money"><style:text-properties fo:font-family="Liberation Sans" fo:font-size="10pt"/><style:paragraph-properties fo:text-align="end" fo:margin="0.04in"/></style:style>
  <style:style style:name="ce-number" style:family="table-cell"><style:text-properties fo:font-family="Liberation Sans" fo:font-size="10pt"/><style:paragraph-properties fo:text-align="end" fo:margin="0.04in"/></style:style>
  <style:style style:name="ro-head" style:family="table-row"><style:table-row-properties style:row-height="0.32in"/></style:style>
  <style:style style:name="ro-body" style:family="table-row"><style:table-row-properties style:min-row-height="0.27in"/></style:style>
  <style:style style:name="co-id" style:family="table-column"><style:table-column-properties style:column-width="1.05in"/></style:style>
  <style:style style:name="co-vendor" style:family="table-column"><style:table-column-properties style:column-width="2.15in"/></style:style>
  <style:style style:name="co-money" style:family="table-column"><style:table-column-properties style:column-width="1.15in"/></style:style>
  <style:style style:name="co-status" style:family="table-column"><style:table-column-properties style:column-width="1.25in"/></style:style>
  <style:style style:name="co-note" style:family="table-column"><style:table-column-properties style:column-width="5.25in"/></style:style>
  <style:style style:name="ta-print" style:family="table" style:master-page-name="Default"/>
 </office:automatic-styles>
 <office:body><office:spreadsheet>
  <table:table table:name="Reconciliation" table:style-name="ta-print"><table:table-column table:style-name="co-id"/><table:table-column table:style-name="co-id"/><table:table-column table:style-name="co-vendor"/><table:table-column table:style-name="co-money"/><table:table-column table:style-name="co-id"/><table:table-column table:style-name="co-money"/><table:table-column table:style-name="co-status"/><table:table-column table:style-name="co-note"/><table:table-column table:style-name="co-money"/>{''.join(reconciliation)}</table:table>
  <table:table table:name="Controls" table:style-name="ta-print"><table:table-column table:style-name="co-vendor"/><table:table-column table:style-name="co-note"/>{''.join(controls)}</table:table>
 </office:spreadsheet></office:body>
</office:document-content>
'''
    return xml.encode("utf-8")


STYLES = b'''<?xml version="1.0" encoding="UTF-8"?><office:document-styles xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3"><office:styles><style:default-style style:family="table-cell"/></office:styles><office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="11in" fo:page-height="8.5in" style:print-orientation="landscape" style:scale-to-X="1" style:scale-to-Y="1" fo:margin="0.35in"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Default" style:page-layout-name="pm1"/></office:master-styles></office:document-styles>'''
META = b'''<?xml version="1.0" encoding="UTF-8"?><office:document-meta xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" office:version="1.3"><office:meta><meta:generator>Cua Driver Bench</meta:generator></office:meta></office:document-meta>'''
MANIFEST = b'''<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/></manifest:manifest>'''


def add(archive: zipfile.ZipFile, name: str, payload: bytes, compression: int) -> None:
    info = zipfile.ZipInfo(name, STAMP)
    info.create_system = 3
    info.compress_type = compression
    info.external_attr = 0o100644 << 16
    archive.writestr(info, payload)


def build(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w") as archive:
        add(archive, "mimetype", MIMETYPE, zipfile.ZIP_STORED)
        add(archive, "content.xml", content(), zipfile.ZIP_DEFLATED)
        add(archive, "styles.xml", STYLES, zipfile.ZIP_DEFLATED)
        add(archive, "meta.xml", META, zipfile.ZIP_DEFLATED)
        add(archive, "META-INF/manifest.xml", MANIFEST, zipfile.ZIP_DEFLATED)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        try:
            with zipfile.ZipFile(OUTPUT) as archive:
                valid = archive.read("mimetype") == MIMETYPE and archive.read("content.xml") == content()
        except (OSError, KeyError, zipfile.BadZipFile):
            valid = False
        print("fixture ODS up to date" if valid else "fixture ODS stale")
        return 0 if valid else 1
    build(OUTPUT)
    print(hashlib.sha256(OUTPUT.read_bytes()).hexdigest())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
