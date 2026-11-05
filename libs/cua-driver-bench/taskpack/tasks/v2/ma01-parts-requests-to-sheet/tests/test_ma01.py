"""Checker tests for MA-01 on untouched, solved and near-miss states (sheet and web state)."""

from __future__ import annotations

import copy
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from xml.sax.saxutils import escape

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))
sys.path.insert(0, str(BUNDLE.parent / "_kit" / "devtools"))
sys.path.insert(0, str(BUNDLE / "evaluator"))

from v2kit import common  # noqa: E402
import evaluate as ev  # noqa: E402
import lo_roundtrip  # noqa: E402

SOLVED_ROWS = list(ev.NEW)
NEW_IDS = {w[0] for w in ev.NEW}


def row_xml(rid: str, part: str, qty, who: str) -> str:
    if isinstance(qty, int):
        q = f'<table:table-cell office:value-type="float" office:value="{qty}"><text:p>{qty}</text:p></table:table-cell>'
    else:
        q = f'<table:table-cell office:value-type="string"><text:p>{escape(qty)}</text:p></table:table-cell>'
    cells = "".join(f'<table:table-cell office:value-type="string"><text:p>{escape(v)}</text:p></table:table-cell>' for v in (rid, part))
    last = f'<table:table-cell office:value-type="string"><text:p>{escape(who)}</text:p></table:table-cell>'
    return f"    <table:table-row>{cells}{q}{last}</table:table-row>\n"


def sheet_with(rows: list, keep_existing: bool = True) -> str:
    text = (BUNDLE / "fixture" / "purchase-log.fods").read_text("utf-8")
    if not keep_existing:  # drop every data row below the header
        start = text.index("<table:table-row>", text.index("</table:table-row>"))
        end = text.index("   </table:table>")
        text = text[:start] + text[end:]
    extra = "".join(row_xml(*r) for r in rows)
    return text.replace("   </table:table>", extra + "   </table:table>")


class MA01(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        self.initial = common.read_json(BUNDLE / "app/initial-state.json")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def web(self, logged: set[str]) -> None:
        state = copy.deepcopy(self.initial)
        for r in state["requests"]:
            if r["id"] in logged:
                r["logged"] = True
        common.write_json(self.ws / "state/state.json", state)

    def sheet(self, rows: list, keep_existing: bool = True) -> None:
        (self.ws / "purchase-log.fods").write_text(sheet_with(rows, keep_existing), "utf-8")

    def run_eval(self) -> dict:
        return common.evaluate_dir("MA-01", ev.evaluate, self.ws)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)
        self.web(set())
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        self.sheet(SOLVED_ROWS)
        self.web(NEW_IDS)
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_quantity_as_text_and_other_case_still_passes(self) -> None:
        self.sheet([(rid, part.upper(), str(qty), who.lower()) for rid, part, qty, who in SOLVED_ROWS])
        self.web(NEW_IDS)
        self.assertTrue(self.run_eval()["passed"])

    def test_requested_instead_of_approved_quantity_fails(self) -> None:
        self.sheet([(rid, part, 6 if rid == "PR-2039" else qty, who) for rid, part, qty, who in SOLVED_ROWS])
        self.web(NEW_IDS)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["log_row_PR-2039"]["passed"])

    def test_page_order_instead_of_id_order_fails(self) -> None:
        self.sheet(list(reversed(SOLVED_ROWS)))
        self.web(NEW_IDS)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["log_rows_in_order"]["passed"])

    def test_pending_request_logged_too_fails(self) -> None:
        self.sheet(SOLVED_ROWS + [("PR-2042", "Acrylic sheet 3 mm, clear, 600 x 400 mm", 6, "Bastien Quarrie")])
        self.web(NEW_IDS)
        self.assertFalse(self.run_eval()["passed"])

    def test_sheet_done_but_one_not_marked_fails(self) -> None:
        self.sheet(SOLVED_ROWS)
        self.web(NEW_IDS - {"PR-2041"})
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["marked_PR-2041"]["passed"])

    def test_only_web_marked_fails(self) -> None:
        self.web(NEW_IDS)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertLess(r["score"], 0.5)

    def test_other_request_edited_fails(self) -> None:
        self.sheet(SOLVED_ROWS)
        self.web(NEW_IDS | {"PR-2032"})
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["other_requests_unchanged"]["passed"])

    def test_existing_rows_overwritten_fails(self) -> None:
        self.sheet(SOLVED_ROWS, keep_existing=False)
        self.web(NEW_IDS)
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["existing_log_rows_kept"]["passed"])

    @unittest.skipUnless(lo_roundtrip.available(), "LibreOffice is not installed")
    def test_libreoffice_roundtrip(self) -> None:
        out = Path(self.tmp.name) / "rt"
        src = Path(self.tmp.name) / "src" / "purchase-log.fods"
        src.parent.mkdir()
        src.write_text(sheet_with(SOLVED_ROWS), "utf-8")
        shutil.copyfile(lo_roundtrip.roundtrip(src, out / "solved"), self.ws / "purchase-log.fods")
        self.web(NEW_IDS)
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        shutil.copyfile(lo_roundtrip.roundtrip(BUNDLE / "fixture" / "purchase-log.fods", out / "untouched"), self.ws / "purchase-log.fods")
        self.web(set())
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)


if __name__ == "__main__":
    unittest.main()
