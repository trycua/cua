"""Unit tests for the docs code-block linter (stdlib only).

python3 -m unittest discover -s tests/e2e/cua-sdk/docs -p 'test_*.py'
"""

from __future__ import annotations

import datetime as dt
import tempfile
import textwrap
import unittest
from pathlib import Path

import extract

POLICY = {
    "lanes": {"docs": "", "fleet": ""},
    "skip": {
        "host-install": {"owner": "docs"},
        "placeholder": {"owner": "docs", "requires_placeholder": True},
        "old": {"owner": "docs", "expires": "2020-01-01"},
    },
}


def page(tmp: Path, name: str, body: str) -> Path:
    path = tmp / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(textwrap.dedent(body))
    return path


class LintTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())

    def run_lint(self, baseline=frozenset()):
        return extract.lint(
            extract.all_fences(self.tmp), POLICY, set(baseline), today=dt.date(2026, 9, 23)
        )

    def test_dispositions_pass(self):
        page(
            self.tmp,
            "a.mdx",
            """
            ```python test="docs" id="one"
            print(1)
            ```

            ```text output
            1
            ```

            ```bash skip="host-install"
            pip install cua
            ```

            ```python skip="placeholder"
            connect("<name>")
            ```
            """,
        )
        problems, pending = self.run_lint()
        self.assertEqual(problems, [])
        self.assertEqual(pending, [])

    def test_undecided_block_fails_unless_baselined(self):
        page(self.tmp, "a.mdx", "```bash\ncua sb ls\n```\n")
        problems, _ = self.run_lint()
        self.assertEqual(len(problems), 1)
        self.assertIn("no disposition", problems[0])
        (fence,) = extract.all_fences(self.tmp)
        problems, pending = self.run_lint({("a.mdx", fence.fingerprint)})
        self.assertEqual(problems, [])
        self.assertEqual(pending, [fence])

    def test_edited_block_leaves_baseline(self):
        page(self.tmp, "a.mdx", "```bash\ncua sb ls --json\n```\n")
        problems, _ = self.run_lint({("a.mdx", "0000000000000000")})
        self.assertTrue(any("no disposition" in p for p in problems))
        self.assertTrue(any("--update-baseline" in p for p in problems))

    def test_rule_violations(self):
        page(
            self.tmp,
            "a.mdx",
            """
            ```python test="docs"
            x = 1
            ```

            ```python test="nightly" id="Bad_Id"
            x = 1
            ```

            ```python test="docs" id="dup"
            x = 1
            ```

            ```python test="docs" id="dup"
            x = 2
            ```

            ```bash skip="because"
            rm -rf /
            ```

            ```bash skip="old"
            ls
            ```

            ```python skip="placeholder"
            connect("sandbox")
            ```

            ```text output skip="host-install"
            x
            ```

            ```text output output-of="missing"
            x
            ```
            """,
        )
        problems, _ = self.run_lint()
        text = "\n".join(problems)
        for needle in [
            "needs a stable id",
            "unknown lane(s) ['nightly']",
            "must match",
            "duplicate id 'dup'",
            "'because' is not in",
            "expired on 2020-01-01",
            "visible placeholder",
            "more than one disposition",
            "output-of='missing'",
        ]:
            self.assertIn(needle, text)

    def test_generated_pages_and_regions_are_exempt(self):
        page(
            self.tmp, "gen.mdx", "{/*\n  AUTO-GENERATED FILE - DO NOT EDIT\n*/}\n```bash\nx\n```\n"
        )
        page(
            self.tmp,
            "region.mdx",
            "{/* GENERATED:t:start */}\n```bash\nx\n```\n{/* GENERATED:t:end */}\n",
        )
        problems, _ = self.run_lint()
        self.assertEqual(problems, [])

    def test_shrink_only(self):
        self.assertEqual(extract.shrink_problems({("a", "1")}, {("a", "1"), ("b", "2")}), [])
        self.assertEqual(len(extract.shrink_problems({("c", "3")}, {("a", "1")})), 1)
        # A moved block (same code, new page) is not growth.
        self.assertEqual(extract.shrink_problems({("new", "1")}, {("old", "1")}), [])
        self.assertEqual(len(extract.shrink_problems({("a", "1"), ("b", "1")}, {("a", "1")})), 1)

    def test_update_baseline_carries_moved_blocks(self):
        page(self.tmp, "new.mdx", "```bash\ncua sb ls\n```\n```bash\ncua sb rm x\n```\n")
        moved, fresh = extract.all_fences(self.tmp)
        kept = extract.carry_baseline([moved, fresh], {("old.mdx", moved.fingerprint)})
        self.assertEqual(kept, [moved])

    def test_multi_lane_and_stable_ids(self):
        page(
            self.tmp,
            "g/p.mdx",
            '```bash\nx\n```\n```python test="docs,fleet" id="run" session="s"\nprint(1)\n```\n',
        )
        (block,) = extract.all_blocks(self.tmp)
        self.assertEqual(block.lanes, ["docs", "fleet"])
        self.assertEqual(block.id, "g/p#run")
        self.assertEqual(block.session, "s")


class ExcerptTest(unittest.TestCase):
    def test_excerpt_must_match_its_region(self):
        repo = Path(tempfile.mkdtemp())
        src = repo / "samples" / "app.ts"
        src.parent.mkdir(parents=True)
        src.write_text(
            "function f() {\n"
            "  // #region docs:open\n"
            "  const x = 1\n"
            "    .toString()\n"
            "  // #endregion docs:open\n"
            "}\n"
            "# #region docs:py\n"
            "x = 2\n"
            "# #endregion docs:py\n"
        )
        docs = repo / "docs"
        page(
            docs,
            "t.mdx",
            """
            ```ts test="excerpt" id="open" source="samples/app.ts#open"
            const x = 1
              .toString()
            ```

            <Tab>
              ```python test="excerpt" id="py" source="samples/app.ts#py"
              x = 2
              ```
            </Tab>

            ```ts test="excerpt" id="stale" source="samples/app.ts#open"
            const x = 2
            ```

            ```ts test="excerpt" id="gone" source="samples/app.ts#nope"
            x
            ```

            ```ts test="excerpt" id="bare"
            x
            ```
            """,
        )
        policy = {"lanes": {"excerpt": {}}, "skip": {}}
        problems, _ = extract.lint(
            extract.all_fences(docs), policy, set(), today=dt.date(2026, 9, 23), repo=repo
        )
        self.assertEqual(len(problems), 3, problems)
        self.assertIn("excerpt differs from samples/app.ts#open", problems[0])
        self.assertIn("no region docs:nope in samples/app.ts", problems[1])
        self.assertIn('needs source="<repo path>#<region id>"', problems[2])
        self.assertEqual(extract.sync_excerpts(docs, repo), 1)
        problems, _ = extract.lint(
            extract.all_fences(docs), policy, set(), today=dt.date(2026, 9, 23), repo=repo
        )
        self.assertEqual(len(problems), 2, problems)
        stale = (docs / "t.mdx").read_text().split('id="stale"')[1]
        self.assertTrue(
            stale.startswith(' source="samples/app.ts#open"\nconst x = 1\n  .toString()\n```')
        )


class AnnotateTest(unittest.TestCase):
    def test_mechanical_dispositions(self):
        tmp = Path(tempfile.mkdtemp())
        path = page(
            tmp,
            "a.mdx",
            """
            ```
            plain
            ```

            ```console title="x"
            $ ls
            ```

            ```mermaid
            graph TD
            ```

            ```bash
            pip install cua  # the SDK
            brew install ffmpeg
            ```

            ```bash
            npm run build
            ```

              ```bash tab="Linux"
              curl -fsSL https://cua.ai/install.sh | sh
              ```
            """,
        )
        self.assertEqual(extract.annotate(tmp), 5)
        text = path.read_text()
        self.assertIn("```text output\nplain", text)
        self.assertIn('```console title="x" output', text)
        self.assertIn('```mermaid skip="diagram"', text)
        self.assertIn('```bash skip="host-install"\npip install', text)
        self.assertIn("```bash\nnpm run build", text)
        self.assertIn('  ```bash tab="Linux" skip="host-install"', text)
        self.assertEqual(extract.annotate(tmp), 0)


if __name__ == "__main__":
    unittest.main()
