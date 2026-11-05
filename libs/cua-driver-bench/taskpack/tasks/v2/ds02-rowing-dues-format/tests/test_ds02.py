"""Checker tests for DS-02 on untouched, solved and near-miss documents (and LibreOffice round-trips)."""

from __future__ import annotations

import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))
sys.path.insert(0, str(BUNDLE.parent / "_kit" / "devtools"))
sys.path.insert(0, str(BUNDLE / "evaluator"))

from v2kit import common  # noqa: E402
import evaluate as ev  # noqa: E402
import lo_roundtrip  # noqa: E402

DOC = "dues.fods"
CONFIG_NS = 'xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0" xmlns:ooo="http://openoffice.org/2004/office"'


def view_settings(v_pos: int, h_pos: int) -> str:
    """The ooo:view-settings set LibreOffice writes from a window (key names as in its own files)."""
    def item(name: str, kind: str, value) -> str:
        return f'<config:config-item config:name="{name}" config:type="{kind}">{value}</config:config-item>'

    table = "".join([
        item("CursorPositionX", "int", 0), item("CursorPositionY", "int", 0),
        item("HorizontalSplitMode", "short", 2 if h_pos else 0), item("VerticalSplitMode", "short", 2 if v_pos else 0),
        item("HorizontalSplitPosition", "int", h_pos), item("VerticalSplitPosition", "int", v_pos),
        item("ActiveSplitRange", "short", 2), item("PositionLeft", "int", 0), item("PositionRight", "int", h_pos),
        item("PositionTop", "int", 0), item("PositionBottom", "int", v_pos),
    ])
    return (
        '<config:config-item-set config:name="ooo:view-settings">'
        '<config:config-item-map-indexed config:name="Views"><config:config-item-map-entry>'
        + item("ViewId", "string", "view1")
        + '<config:config-item-map-named config:name="Tables">'
        f'<config:config-item-map-entry config:name="{ev.SHEET}">{table}</config:config-item-map-entry>'
        "</config:config-item-map-named>"
        + item("ActiveTable", "string", ev.SHEET)
        + "</config:config-item-map-entry></config:config-item-map-indexed></config:config-item-set>"
    )


def settings(v_pos: int, h_pos: int) -> str:
    return "<office:settings>" + view_settings(v_pos, h_pos) + "</office:settings>"


def build(freeze=(1, 0), bold_header=True, currency=("€", 2), wrap=True, bold_all=False, change=None) -> str:
    text = (BUNDLE / "fixture" / DOC).read_text("utf-8")
    text = text.replace("<office:document ", f"<office:document {CONFIG_NS} ", 1)
    if freeze:
        text = text.replace(" <office:styles>", settings(*freeze) + "\n <office:styles>", 1)
    styles = []
    if currency:
        symbol, places = currency
        styles.append(
            f'<number:currency-style style:name="N_cur"><number:currency-symbol>{symbol}</number:currency-symbol>'
            f'<number:text> </number:text><number:number number:decimal-places="{places}" number:min-decimal-places="{places}" '
            'number:min-integer-digits="1" number:grouping="true"/></number:currency-style>'
        )
    styles.append('<style:style style:name="ce_head" style:family="table-cell"><style:text-properties fo:font-weight="bold"/></style:style>')
    styles.append('<style:style style:name="ce_money" style:family="table-cell"' + (' style:data-style-name="N_cur"' if currency else "")
                  + ">" + ('<style:text-properties fo:font-weight="bold"/>' if bold_all else "") + "</style:style>")
    styles.append('<style:style style:name="ce_wrap" style:family="table-cell"><style:table-cell-properties fo:wrap-option="wrap"/></style:style>')
    text = text.replace(" </office:automatic-styles>", "".join(styles) + "\n </office:automatic-styles>", 1)
    rows = re.findall(r"<table:table-row>.*?</table:table-row>", text)
    out_rows = []
    for i, row in enumerate(rows):
        cells = re.findall(r"<table:table-cell(?: [^>]*)?/>|<table:table-cell[^>]*>.*?</table:table-cell>", row)
        assert len(cells) == 6, row
        if i == 0:
            if bold_header:
                cells = [c.replace("<table:table-cell", '<table:table-cell table:style-name="ce_head"', 1) for c in cells]
        else:
            cells[3] = cells[3].replace("<table:table-cell", '<table:table-cell table:style-name="ce_money"', 1)
            if wrap:
                cells[5] = cells[5].replace("<table:table-cell", '<table:table-cell table:style-name="ce_wrap"', 1)
        out_rows.append("<table:table-row>" + "".join(cells) + "</table:table-row>")
    for old, new in zip(rows, out_rows):
        text = text.replace(old, new, 1)
    if change:
        text = text.replace(*change, 1)
    return text


class DS02(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def put(self, text: str) -> None:
        (self.ws / DOC).write_text(text, "utf-8")

    def run_eval(self) -> dict:
        return common.evaluate_dir(ev.TASK_ID, ev.evaluate, self.ws)

    def roundtrip_workspace(self) -> None:
        out = lo_roundtrip.roundtrip(self.ws / DOC, Path(self.tmp.name) / "rt")
        (self.ws / DOC).write_bytes(out.read_bytes())

    def assert_fails(self, check: str) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"][check]["passed"], r)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        self.put(build())
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_freeze_at_b2_fails(self) -> None:
        self.put(build(freeze=(1, 1)))
        self.assert_fails("header_row_frozen")

    def test_freeze_two_rows_fails(self) -> None:
        self.put(build(freeze=(2, 0)))
        self.assert_fails("header_row_frozen")

    def test_no_freeze_fails(self) -> None:
        self.put(build(freeze=None))
        self.assert_fails("header_row_frozen")

    def test_pound_sign_fails(self) -> None:
        self.put(build(currency=("£", 2)))
        self.assert_fails("amount_euro_two_decimals")

    def test_plain_number_format_fails_without_crashing(self) -> None:
        # a number style (not currency) on Amount: the checker must report it, not crash, and grade the rest
        text = build(currency=("€", 2)).replace(
            '<number:currency-style style:name="N_cur"><number:currency-symbol>€</number:currency-symbol><number:text> </number:text>',
            '<number:number-style style:name="N_cur">',
        ).replace("</number:currency-style>", "</number:number-style>")
        self.put(text)
        r = self.run_eval()
        self.assertNotIn("evaluator_ok", r["checks"])
        self.assertFalse(r["checks"]["amount_euro_two_decimals"]["passed"])
        self.assertTrue(r["checks"]["header_row_bold"]["passed"], r)
        self.assertTrue(r["checks"]["notes_wrapped"]["passed"], r)

    def test_no_decimals_fails(self) -> None:
        self.put(build(currency=("€", 0)))
        self.assert_fails("amount_euro_two_decimals")

    def test_missing_wrap_fails(self) -> None:
        self.put(build(wrap=False))
        self.assert_fails("notes_wrapped")

    def test_bold_amounts_fail(self) -> None:
        self.put(build(bold_all=True))
        self.assert_fails("data_rows_not_bold")

    def test_changed_value_fails(self) -> None:
        self.put(build(change=('office:value="72.5"', 'office:value="72.05"')))
        self.assert_fails("values_unchanged")

    @unittest.skipUnless(lo_roundtrip.available(), "LibreOffice not installed")
    def test_solved_survives_libreoffice_roundtrip(self) -> None:
        # A headless conversion has no view, so LibreOffice drops the view settings (the freeze). Put back
        # the view settings a window would write, then everything else is LibreOffice's own serialisation.
        self.put(build())
        self.roundtrip_workspace()
        text = (self.ws / DOC).read_text("utf-8")
        text, n = re.subn(r'<config:config-item-set config:name="ooo:view-settings">.*?</config:config-item-set>',
                          lambda m: view_settings(1, 0), text, count=1, flags=re.S)
        self.assertEqual(n, 1)
        self.put(text)
        r = self.run_eval()
        self.assertTrue(r["passed"], r)

    @unittest.skipUnless(lo_roundtrip.available(), "LibreOffice not installed")
    def test_whole_column_formatting_survives_roundtrip(self) -> None:
        # Formatting whole columns D and F: LibreOffice stores the style as the column default and leaves
        # it off the cells, which the checker must resolve.
        text = build(wrap=False).replace(' table:style-name="ce_money"', "")
        text = text.replace(
            '<table:table-column table:style-name="co_mid" table:number-columns-repeated="4"/><table:table-column table:style-name="co_notes"/>',
            '<table:table-column table:style-name="co_mid" table:number-columns-repeated="2"/>'
            '<table:table-column table:style-name="co_mid" table:default-cell-style-name="ce_money"/>'
            '<table:table-column table:style-name="co_mid"/>'
            '<table:table-column table:style-name="co_notes" table:default-cell-style-name="ce_wrap"/>', 1)
        self.put(text)
        self.roundtrip_workspace()
        text = (self.ws / DOC).read_text("utf-8")
        text = re.sub(r'<config:config-item-set config:name="ooo:view-settings">.*?</config:config-item-set>',
                      lambda m: view_settings(1, 0), text, count=1, flags=re.S)
        self.put(text)
        r = self.run_eval()
        self.assertTrue(r["passed"], r)

    @unittest.skipUnless(lo_roundtrip.available(), "LibreOffice not installed")
    def test_untouched_roundtrip_still_zero(self) -> None:
        self.roundtrip_workspace()
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)
        self.assertTrue(all(c["passed"] for c in r["checks"].values() if c["kind"] == "preservation"), r)


if __name__ == "__main__":
    unittest.main()
