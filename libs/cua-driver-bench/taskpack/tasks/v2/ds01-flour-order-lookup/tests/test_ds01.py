"""Checker tests for DS-01 on untouched, solved and near-miss documents (and LibreOffice round-trips)."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))
sys.path.insert(0, str(BUNDLE.parent / "_kit" / "devtools"))
sys.path.insert(0, str(BUNDLE / "evaluator"))

from v2kit import common, odf  # noqa: E402
import evaluate as ev  # noqa: E402
import lo_roundtrip  # noqa: E402

DOC = "orders.fods"
EMPTY_PAIR = '<table:table-cell table:number-columns-repeated="2"/>'


def prices() -> dict[str, float]:
    rates = odf.sheets(odf.load(BUNDLE / "fixture" / DOC))["Rates"]
    return {r[0].display: r[2].number for r in rates[1:]}


def cell_xml(value: float, formula: str | None) -> str:
    f = f' table:formula="{formula}"' if formula else ""
    return f'<table:table-cell{f} office:value-type="float" office:value="{value:g}"><text:p>{value:g}</text:p></table:table-cell>'


def build(rate_formula=None, cost_formula=None, price_of=None) -> str:
    """The fixture with E/F filled; a formula function returning None means a typed constant."""
    text = (BUNDLE / "fixture" / DOC).read_text("utf-8")
    p = prices()
    orders = odf.sheets(odf.load(BUNDLE / "fixture" / DOC))["Orders"]
    for r in range(2, 22):
        code, kilos = orders[r - 1][2].display, orders[r - 1][3].number
        rate = (price_of or p.__getitem__)(code)
        cost = round(kilos * rate, 2)
        e = cell_xml(rate, rate_formula(r) if rate_formula else None)
        f = cell_xml(cost, cost_formula(r) if cost_formula else None)
        text = text.replace(EMPTY_PAIR, e + f, 1)
    return text


def vlookup(r: int) -> str:
    return f"of:=VLOOKUP([.C{r}];$Rates.$A$2:$C$9;3;0)"


def product(r: int) -> str:
    return f"of:=[.D{r}]*[.E{r}]"


class DS01(unittest.TestCase):
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

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        self.put(build(vlookup, product))
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_index_match_passes(self) -> None:
        self.put(build(lambda r: f"of:=INDEX($Rates.$C$2:$C$9;MATCH([.C{r}];$Rates.$A$2:$A$9;0))", product))
        self.assertTrue(self.run_eval()["passed"])

    def test_typed_rates_fail(self) -> None:
        self.put(build(None, product))
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["rate_formulas_use_rates_sheet"]["passed"])

    def test_approximate_match_values_fail(self) -> None:
        # VLOOKUP without an exact-match flag on the unsorted Rates sheet returns wrong prices
        wrong = dict(prices())
        wrong["SP-W"] = 1.18
        self.put(build(lambda r: f"of:=VLOOKUP([.C{r}];$Rates.$A$2:$C$9;3)", product, wrong.__getitem__))
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["rate_values_ok"]["passed"])

    def test_hard_wired_rates_cell_fails(self) -> None:
        # each row pointing straight at a Rates price cell is not a lookup of the row's flour code
        row_of = {code: i + 2 for i, code in enumerate(prices())}
        orders = odf.sheets(odf.load(BUNDLE / "fixture" / DOC))["Orders"]
        self.put(build(lambda r: f"of:=[$Rates.C{row_of[orders[r - 1][2].display]}]", product))
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["rate_formulas_use_rates_sheet"]["passed"])

    def test_whole_column_lookup_passes(self) -> None:
        self.put(build(lambda r: f"of:=VLOOKUP([.$C{r}];[$Rates.A:.C];3;0)", product))
        self.assertTrue(self.run_eval()["passed"])

    def test_typed_line_costs_fail(self) -> None:
        self.put(build(vlookup, None))
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["line_cost_formulas"]["passed"])

    def test_constant_formula_line_costs_fail(self) -> None:
        self.put(build(vlookup, lambda r: "of:=1+1"))
        self.assertFalse(self.run_eval()["checks"]["line_cost_formulas"]["passed"])

    def test_product_range_passes(self) -> None:
        self.put(build(vlookup, lambda r: f"of:=PRODUCT([.$D{r}:.E{r}])"))
        self.assertTrue(self.run_eval()["passed"])

    def test_changed_rates_sheet_fails(self) -> None:
        head, _, tail = build(vlookup, product).rpartition('office:value="1.64"')
        self.put(head + 'office:value="1.65"' + tail)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["rates_sheet_unchanged"]["passed"])

    def test_total_row_added_fails(self) -> None:
        text = build(vlookup, product)
        total = '<table:table-row><table:table-cell table:number-columns-repeated="5"/>' + cell_xml(999.0, "of:=SUM([.F2:.F21])") + "</table:table-row>"
        self.put(text.replace("</table:table>", total + "</table:table>", 1))
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["nothing_else_added_to_orders"]["passed"])

    @unittest.skipUnless(lo_roundtrip.available(), "LibreOffice not installed")
    def test_solved_survives_libreoffice_roundtrip(self) -> None:
        self.put(build(vlookup, product))
        self.roundtrip_workspace()
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
