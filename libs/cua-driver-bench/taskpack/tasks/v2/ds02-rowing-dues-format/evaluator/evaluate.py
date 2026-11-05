"""Evaluate DS-02: header row frozen and bold, Amount as euro currency with two decimals, Notes wrapped;
every value unchanged and the data rows not bold."""

from __future__ import annotations

import sys
import xml.etree.ElementTree as ET
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))

from v2kit import common, odf  # noqa: E402

TASK_ID = "DS-02"
DOCUMENT = "dues.fods"
SHEET = "Dues 2026-27"
COLS = "ABCDEF"
EURO = ("€", "EUR")


def _value(c: odf.Cell) -> tuple:
    if c.number is not None:
        return ("n", c.number)
    if c.value_type in ("date", "time", "boolean"):
        return (c.value_type, c.value)
    return ("s", c.display)


def _currency_ok(doc: odf.Doc, style: str | None) -> tuple[bool, str]:
    """A currency data style with the euro sign and exactly two decimals (following style:map targets)."""
    data = odf.data_style_of(doc, style)
    if data is None:
        return False, "no data style"
    todo, seen = [data], set()
    while todo:
        node = todo.pop()
        name = node.get(odf.q("style:name"))
        if name in seen:
            continue
        seen.add(name)
        if node.tag == odf.q("number:currency-style"):
            symbol = "".join(s.text or "" for s in node.findall("number:currency-symbol", odf.NS)).strip()
            number = node.find("number:number", odf.NS)
            places = number.get(odf.q("number:decimal-places")) if number is not None else None
            if symbol in EURO and places == "2":
                return True, ""
            detail = f"symbol={symbol!r} places={places!r}"
        else:
            detail = f"{node.tag.rsplit('}', 1)[-1]} {name}"  # stdlib ElementTree: strip the namespace by hand
        for m in node.findall("style:map", odf.NS):
            target = doc.style_index.get(m.get(odf.q("style:apply-style-name")) or "")
            if target is not None:
                todo.append(target)
    return False, detail


def evaluate(workspace: Path, checks: common.Checks) -> None:
    original = odf.sheets(odf.load(BUNDLE / "fixture" / DOCUMENT))[SHEET]
    doc = odf.load(workspace / DOCUMENT)
    all_sheets = odf.sheets(doc)
    rows = all_sheets.get(SHEET, [])
    last = len(original)  # header + member rows

    cfg = odf.config_items(doc)
    v_mode, v_pos = cfg.get(f"{SHEET}/VerticalSplitMode"), cfg.get(f"{SHEET}/VerticalSplitPosition")
    h_mode, h_pos = cfg.get(f"{SHEET}/HorizontalSplitMode", "0"), cfg.get(f"{SHEET}/HorizontalSplitPosition", "0")
    frozen_row = v_mode == "2" and v_pos == "1"
    no_frozen_cols = h_mode != "2" or h_pos == "0"
    checks.add("header_row_frozen", frozen_row and no_frozen_cols,
               f"vertical mode={v_mode} pos={v_pos}; horizontal mode={h_mode} pos={h_pos}")

    not_bold = [f"{c}1" for c in COLS if not odf.is_bold(odf.style_props(doc, odf.cell(rows, f"{c}1").style))]
    checks.add("header_row_bold", not not_bold, ", ".join(not_bold))

    bad_money = []
    for r in range(2, last + 1):
        ok, detail = _currency_ok(doc, odf.cell(rows, f"D{r}").style)
        if not ok:
            bad_money.append(f"D{r}: {detail}")
    checks.add("amount_euro_two_decimals", not bad_money, "; ".join(bad_money[:4]))

    unwrapped = [f"F{r}" for r in range(2, last + 1)
                 if odf.cell(original, f"F{r}").display and odf.style_props(doc, odf.cell(rows, f"F{r}").style, "style:table-cell-properties").get(odf.q("fo:wrap-option")) != "wrap"]
    checks.add("notes_wrapped", not unwrapped, ", ".join(unwrapped[:8]))

    changed = [f"{c}{r}" for r in range(1, last + 1) for c in COLS
               if _value(odf.cell(rows, f"{c}{r}")) != _value(odf.cell(original, f"{c}{r}"))]
    extra = [f"row {i + 1}" for i, row in enumerate(rows[last:], start=last) if any(c.display or c.value for c in row)]
    extra += [f"row {i + 1} past F" for i, row in enumerate(rows[:last]) if any(c.display or c.value for c in row[len(COLS):])]
    checks.add("values_unchanged", not changed and not extra and list(all_sheets) == [SHEET],
               ", ".join((changed + extra)[:10]) or f"sheets {list(all_sheets)}", kind="preservation")

    bold_data = [f"{c}{r}" for r in range(2, last + 1) for c in COLS
                 if odf.cell(rows, f"{c}{r}").display and odf.is_bold(odf.style_props(doc, odf.cell(rows, f"{c}{r}").style))]
    checks.add("data_rows_not_bold", not bold_data, ", ".join(bold_data[:8]), kind="preservation")


if __name__ == "__main__":
    common.evaluator_main(TASK_ID, evaluate)
