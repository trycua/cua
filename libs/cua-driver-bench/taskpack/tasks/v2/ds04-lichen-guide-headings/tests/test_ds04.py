"""Checker tests for DS-04 on untouched, solved and near-miss documents (and LibreOffice round-trips)."""

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

DOC = "guide.fodt"
HEADING_STYLES = """
  <style:style style:name="Heading" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Text_20_body" style:class="text"><style:paragraph-properties fo:margin-top="0.423cm" fo:margin-bottom="0.212cm" fo:keep-with-next="always"/><style:text-properties fo:font-size="14pt"/></style:style>
  <style:style style:name="Heading_20_1" style:display-name="Heading 1" style:family="paragraph" style:parent-style-name="Heading" style:next-style-name="Text_20_body" style:default-outline-level="1" style:class="text"><style:text-properties fo:font-size="130%" fo:font-weight="bold"/></style:style>
  <style:style style:name="Heading_20_2" style:display-name="Heading 2" style:family="paragraph" style:parent-style-name="Heading" style:next-style-name="Text_20_body" style:default-outline-level="2" style:class="text"><style:text-properties fo:font-size="115%" fo:font-weight="bold"/></style:style>
  <style:style style:name="Heading_20_3" style:display-name="Heading 3" style:family="paragraph" style:parent-style-name="Heading" style:next-style-name="Text_20_body" style:default-outline-level="3" style:class="text"><style:text-properties fo:font-size="101%" fo:font-weight="bold"/></style:style>
  <style:style style:name="Contents_20_Heading" style:display-name="Contents Heading" style:family="paragraph" style:parent-style-name="Heading" style:class="index"/>
  <style:style style:name="Contents_20_1" style:display-name="Contents 1" style:family="paragraph" style:parent-style-name="Standard" style:class="index"/>
  <style:style style:name="Contents_20_2" style:display-name="Contents 2" style:family="paragraph" style:parent-style-name="Standard" style:class="index"><style:paragraph-properties fo:margin-left="0.499cm"/></style:style>
"""


def toc(headings: list[tuple[int, str]]) -> str:
    templates = "".join(
        f'<text:table-of-content-entry-template text:outline-level="{lvl}" text:style-name="Contents_20_{lvl}">'
        '<text:index-entry-link-start/><text:index-entry-chapter/><text:index-entry-text/>'
        '<text:index-entry-tab-stop style:type="right" style:leader-char="."/><text:index-entry-page-number/><text:index-entry-link-end/>'
        "</text:table-of-content-entry-template>" for lvl in (1, 2))
    entries = "".join(
        f'<text:p text:style-name="Contents_20_{lvl}"><text:a xlink:type="simple" xlink:href="#__RefHeading___Toc{i}">'
        f"{escape(t)}<text:tab/>{1 + i // 5}</text:a></text:p>" for i, (lvl, t) in enumerate(headings))
    return (
        '<text:table-of-content text:protected="true" text:name="Table of Contents1">'
        '<text:table-of-content-source text:outline-level="10">'
        '<text:index-title-template text:style-name="Contents_20_Heading">Table of Contents</text:index-title-template>'
        + templates + "</text:table-of-content-source><text:index-body>"
        '<text:index-title text:name="Table of Contents1_Head"><text:p text:style-name="Contents_20_Heading">Table of Contents</text:p></text:index-title>'
        + entries + "</text:index-body></text:table-of-content>"
    )


def build(levels: dict[str, int] | None = None, toc_where: str | None = "after_title", toc_headings=None,
          change: tuple[str, str] | None = None) -> str:
    """levels: heading text -> outline level (default: the wanted ones; 0 leaves the paragraph plain)."""
    text = (BUNDLE / "fixture" / DOC).read_text("utf-8")
    text = text.replace(" </office:styles>", HEADING_STYLES + " </office:styles>", 1)
    wanted = {t: lvl for lvl, t in ev.HEADINGS}
    wanted.update(levels or {})
    for heading, lvl in wanted.items():
        old = f'<text:p text:style-name="Text_20_body">{escape(heading)}</text:p>'
        assert old in text, heading
        if lvl:
            text = text.replace(old, f'<text:h text:style-name="Heading_20_{lvl}" text:outline-level="{lvl}">{escape(heading)}</text:h>', 1)
    block = toc(ev.HEADINGS if toc_headings is None else toc_headings)
    title = f'<text:p text:style-name="Title">{escape(ev.TITLE)}</text:p>'
    if toc_where == "after_title":
        text = text.replace(title, title + "\n   <text:p text:style-name=\"Standard\"/>" + block, 1)
    elif toc_where == "end":
        text = text.replace("\n  </office:text>", block + "\n  </office:text>", 1)
    if change:
        assert change[0] in text
        text = text.replace(*change, 1)
    return text


class DS04(unittest.TestCase):
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

    def test_subsection_as_heading_1_fails(self) -> None:
        self.put(build({"Reading the Thallus": 1}))
        self.assert_fails("heading_2_applied")

    def test_subsection_as_heading_3_fails(self) -> None:
        self.put(build({"Glen Netleaf (Pulmolobia hiberna)": 3}))
        self.assert_fails("heading_2_applied")

    def test_missing_heading_fails(self) -> None:
        self.put(build({"Recording Your Finds": 0}))
        self.assert_fails("heading_1_applied")

    def test_stale_toc_fails(self) -> None:
        # contents inserted before the headings were applied and never updated
        self.put(build(toc_headings=[]))
        self.assert_fails("toc_lists_headings")

    def test_toc_at_end_fails(self) -> None:
        self.put(build(toc_where="end"))
        self.assert_fails("toc_after_title")

    def test_no_toc_fails(self) -> None:
        self.put(build(toc_where=None))
        self.assert_fails("toc_after_title")

    def test_body_paragraph_made_heading_fails(self) -> None:
        line = "Thank you for walking gently and for adding to what we know about the island."
        self.put(build(change=(f'<text:p text:style-name="Text_20_body">{line}</text:p>',
                               f'<text:h text:style-name="Heading_20_2" text:outline-level="2">{line}</text:h>')))
        self.assert_fails("no_other_headings")

    def test_edited_text_fails(self) -> None:
        self.put(build(change=("hand lens and a little patience", "hand lens")))
        self.assert_fails("text_unchanged")

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
