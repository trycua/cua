"""Evaluate DS-01: Rate looked up from the Rates sheet by formula, Line cost = kilos x rate by formula
(a formula that refers to the row's kilos cell)."""

from __future__ import annotations

import re
import sys
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, odf  # noqa: E402

TASK_ID = "DS-01"
DOCUMENT = "orders.fods"
FIRST, LAST = 2, 21  # order rows on the Orders sheet


def _same(a: odf.Cell, b: odf.Cell) -> bool:
    if a.number is not None or b.number is not None:
        return a.number == b.number
    return (a.value_type, a.value, a.display) == (b.value_type, b.value, b.display)


def evaluate(workspace: Path, checks: common.Checks) -> None:
    original = odf.sheets(odf.load(BUNDLE / "fixture" / DOCUMENT))
    doc = odf.sheets(odf.load(workspace / DOCUMENT))
    orders = doc.get("Orders", [])
    prices = {r[0].display: r[2].number for r in original["Rates"][1:]}

    rate_formula, rate_value, cost_formula, cost_value = [], [], [], []
    for r in range(FIRST, LAST + 1):
        code = odf.cell(original["Orders"], f"C{r}").display
        kilos = odf.cell(original["Orders"], f"D{r}").number or 0.0
        want_rate = prices[code]
        want_cost = kilos * want_rate
        e, f = odf.cell(orders, f"E{r}"), odf.cell(orders, f"F{r}")
        # a lookup: refers to the Rates sheet and to this row's flour code (not a hard-wired Rates cell)
        own_code = e.formula and re.search(rf"\[(?:\$?'?Orders'?)?\.\$?C\$?{r}[\]:]", e.formula)
        if not (e.formula and "rates." in e.formula.lower().replace("'", "") and own_code):
            rate_formula.append(f"E{r}={e.formula!r}")
        if e.number is None or abs(e.number - want_rate) > 0.005:
            rate_value.append(f"E{r}={e.number!r} want {want_rate}")
        if not (f.formula and re.search(rf"\.\$?D\$?{r}(?!\d)", f.formula)):
            cost_formula.append(f"F{r}={f.formula!r}")
        if f.number is None or abs(f.number - want_cost) > 0.0051:
            cost_value.append(f"F{r}={f.number!r} want {want_cost}")
    checks.add("rate_formulas_use_rates_sheet", not rate_formula, "; ".join(rate_formula[:5]))
    checks.add("rate_values_ok", not rate_value, "; ".join(rate_value[:5]))
    checks.add("line_cost_formulas", not cost_formula, "; ".join(cost_formula[:5]))
    checks.add("line_cost_values_ok", not cost_value, "; ".join(cost_value[:5]))

    changed = []
    for r in range(1, LAST + 1):
        for col in "ABCD":
            if not _same(odf.cell(orders, f"{col}{r}"), odf.cell(original["Orders"], f"{col}{r}")):
                changed.append(f"{col}{r}")
    for col in "EF":
        if odf.cell(orders, f"{col}1").display != odf.cell(original["Orders"], f"{col}1").display:
            changed.append(f"{col}1")
    checks.add("orders_data_unchanged", not changed, ", ".join(changed[:10]), kind="preservation")

    extra = [f"row {i + 1}" for i, row in enumerate(orders[LAST:], start=LAST)
             if any(c.display or c.value is not None for c in row)]
    extra += [f"G{r}" for r in range(1, LAST + 1) if any(c.display or c.value is not None for c in orders[r - 1][6:])] if len(orders) >= LAST else []
    checks.add("nothing_else_added_to_orders", not extra, ", ".join(extra[:10]), kind="preservation")

    rates = doc.get("Rates", [])
    same_rates = len(rates) == len(original["Rates"]) and all(
        len(rates[i]) == len(original["Rates"][i]) and all(_same(a, b) for a, b in zip(rates[i], original["Rates"][i]))
        for i in range(len(rates))
    )
    checks.add("rates_sheet_unchanged", same_rates, f"{len(rates)} rows", kind="preservation")
    checks.add("sheets_kept", list(doc) == list(original), f"sheets {list(doc)}", kind="preservation")


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
