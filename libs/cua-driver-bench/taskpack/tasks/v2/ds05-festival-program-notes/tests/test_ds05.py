"""Checker tests for DS-05 on untouched, solved and near-miss documents (and LibreOffice round-trips)."""

from __future__ import annotations

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

DOC = "programme.fodt"
AUTO = """ <office:automatic-styles>
  <style:style style:name="T1" style:family="text"><style:text-properties fo:font-weight="bold" style:font-weight-asian="bold" style:font-weight-complex="bold"/></style:style>
 </office:automatic-styles>
 <office:body>"""


def note(body: str, cls: str = "footnote") -> str:
    return (f'<text:note text:id="ftn1" text:note-class="{cls}"><text:note-citation>1</text:note-citation>'
            f'<text:note-body><text:p text:style-name="Footnote">{escape(body)}</text:p></text:note-body></text:note>')


def build(venue: str = ev.VENUE, replace_count: int = -1, note_after: str | None = ev.SENTENCE, note_text: str = ev.NOTE,
          note_class: str = "footnote", bold: str | None = ev.PHRASE) -> str:
    text = (BUNDLE / "fixture" / DOC).read_text("utf-8")
    text = text.replace(ev.PLACEHOLDER, escape(venue), replace_count)
    text = text.replace(" <office:body>", AUTO, 1)
    if note_after:
        anchor = escape(note_after)
        assert anchor in text
        text = text.replace(anchor, anchor + note(note_text, note_class), 1)
    if bold:
        assert escape(bold) in text
        text = text.replace(escape(bold), f'<text:span text:style-name="T1">{escape(bold)}</text:span>', 1)
    return text


class DS05(unittest.TestCase):
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

    def test_note_after_space_passes(self) -> None:
        self.put(build(note_after=ev.SENTENCE + " "))
        self.assertTrue(self.run_eval()["passed"])

    def test_one_placeholder_missed_fails(self) -> None:
        self.put(build(replace_count=3))
        self.assert_fails("placeholders_gone")

    def test_brackets_left_fails(self) -> None:
        self.put(build(venue="[" + ev.VENUE + "]"))
        self.assert_fails("placeholders_gone")

    def test_misspelt_venue_fails(self) -> None:
        self.put(build(venue="Greywether Assembly Room"))
        self.assert_fails("venue_inserted")

    def test_note_after_wrong_sentence_fails(self) -> None:
        self.put(build(note_after="Doors open at 19:00."))
        self.assert_fails("footnote_added")

    def test_endnote_fails(self) -> None:
        self.put(build(note_class="endnote"))
        self.assert_fails("footnote_added")

    def test_note_with_typographic_apostrophe_passes(self) -> None:
        self.put(build(note_text=ev.NOTE.replace("'", "\u2019")))
        self.assertTrue(self.run_eval()["passed"])

    def test_wrong_note_text_fails(self) -> None:
        self.put(build(note_text="Children go free."))
        self.assert_fails("footnote_added")

    def test_whole_sentence_bold_fails(self) -> None:
        self.put(build(bold="Bring your own cup for the free apple juice"))
        self.assert_fails("phrase_bold")

    def test_other_text_bold_fails(self) -> None:
        text = build().replace("Mill Lane, Ottersby", '<text:span text:style-name="T1">Mill Lane, Ottersby</text:span>', 1)
        self.put(text)
        self.assert_fails("nothing_else_bold")

    @unittest.skipUnless(lo_roundtrip.available(), "LibreOffice not installed")
    def test_solved_survives_libreoffice_roundtrip(self) -> None:
        self.put(build())
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
