"""Checker tests for WF-06 on an untouched, a solved and near-miss states."""

from __future__ import annotations

import copy
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

BUNDLE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BUNDLE.parent / "_kit"))
sys.path.insert(0, str(BUNDLE / "evaluator"))

from v2kit import common  # noqa: E402
import evaluate as ev  # noqa: E402

BODY = (
    "<p>Join us on <strong>Saturday 24 October</strong> at 10:00 to plant spring bulbs along the edge of Quarry Green.</p>"
    "<ul><li>Gardening gloves</li><li>A trowel or hand fork</li><li>A flask of something warm</li></ul>"
    '<p>Add your name on the <a href="https://quarrylane.example/rota">volunteer rota</a>.</p>'
)
SOLVED = {
    "id": "QL-0416", "title": "Autumn bulb planting on Quarry Green", "category": "volunteering",
    "status": "draft", "published_on": "", "body_html": BODY,
}


class WF06(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.ws = Path(self.tmp.name) / "ws"
        subprocess.run([sys.executable, str(BUNDLE / "reset/setup.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        subprocess.run([sys.executable, str(BUNDLE / "reset/verify.py"), "--workspace", str(self.ws)], check=True, capture_output=True)
        self.initial = common.read_json(BUNDLE / "app/initial-state.json")["posts"]

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write(self, posts: list, events: list[dict]) -> None:
        common.write_json(self.ws / "state/state.json", {"posts": posts})
        lines = [json.dumps({**e, "seq": i + 1}) for i, e in enumerate(events)]
        (self.ws / "state/events.jsonl").write_text("\n".join(lines) + ("\n" if lines else ""))

    def solved(self, **change) -> None:
        post = {**SOLVED, **change}
        self.write(copy.deepcopy(self.initial) + [post], [{"type": "draft_saved", "id": post["id"]}])

    def run_eval(self) -> dict:
        return common.evaluate_dir("WF-06", ev.evaluate, self.ws)

    def test_untouched_fails_with_zero(self) -> None:
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_untouched_after_server_start_fails_with_zero(self) -> None:
        self.write(copy.deepcopy(self.initial), [{"type": "server_started"}])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertEqual(r["score"], 0.0)

    def test_solved_passes(self) -> None:
        self.solved()
        r = self.run_eval()
        self.assertTrue(r["passed"], r)
        self.assertEqual(r["score"], 1.0)

    def test_browser_markup_variants_pass(self) -> None:
        body = (
            "<div>Join us on <b>Saturday 24 October</b>&nbsp;at 10:00 to plant spring bulbs along the edge of Quarry Green</div>"
            "<ul><li>Gardening gloves</li><li>A trowel or hand fork</li><li>A flask of something warm</li></ul>"
            '<p>Add your name on the <a href="https://quarrylane.example/rota/">volunteer rota.</a></p><p> </p>'
        )
        self.solved(body_html=body)
        r = self.run_eval()
        self.assertTrue(r["passed"], r)

    def test_published_instead_of_draft_fails(self) -> None:
        post = {**SOLVED, "status": "published", "published_on": "2026-10-11"}
        self.write(copy.deepcopy(self.initial) + [post], [{"type": "post_published", "id": post["id"]}])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["saved_as_draft_not_published"]["passed"])

    def test_published_then_saved_back_to_draft_fails(self) -> None:
        self.write(copy.deepcopy(self.initial) + [SOLVED], [
            {"type": "post_published", "id": "QL-0416"}, {"type": "draft_saved", "id": "QL-0416"}])
        self.assertFalse(self.run_eval()["passed"])

    def test_whole_sentence_bold_fails(self) -> None:
        self.solved(body_html=BODY.replace("<p>Join us on <strong>Saturday 24 October</strong> at 10:00",
                                           "<p><strong>Join us on Saturday 24 October</strong> at 10:00"))
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["bold_ok"]["passed"])

    def test_list_as_paragraphs_fails(self) -> None:
        self.solved(body_html=BODY.replace("<ul><li>Gardening gloves</li><li>A trowel or hand fork</li><li>A flask of something warm</li></ul>",
                                           "<p>Gardening gloves</p><p>A trowel or hand fork</p><p>A flask of something warm</p>"))
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["list_ok"]["passed"])

    def test_wrong_link_address_fails(self) -> None:
        self.solved(body_html=BODY.replace("https://quarrylane.example/rota", "https://quarrylane.example/rotas"))
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertFalse(r["checks"]["link_ok"]["passed"])

    def test_whole_paragraph_linked_fails(self) -> None:
        self.solved(body_html=BODY.replace('<p>Add your name on the <a href="https://quarrylane.example/rota">volunteer rota</a>.</p>',
                                           '<p><a href="https://quarrylane.example/rota">Add your name on the volunteer rota.</a></p>'))
        self.assertFalse(self.run_eval()["passed"])

    def test_wrong_category_fails(self) -> None:
        self.solved(category="events")
        self.assertFalse(self.run_eval()["passed"])

    def test_edited_published_post_fails(self) -> None:
        posts = copy.deepcopy(self.initial)
        posts[0]["status"] = "draft"
        posts[0]["published_on"] = ""
        self.write(posts + [SOLVED], [{"type": "draft_saved", "id": "QL-0416"}])
        r = self.run_eval()
        self.assertFalse(r["passed"])
        self.assertLess(r["score"], 1.0)

    def test_two_drafts_fail(self) -> None:
        self.write(copy.deepcopy(self.initial) + [SOLVED, {**SOLVED, "id": "QL-0417"}], [])
        self.assertFalse(self.run_eval()["passed"])


if __name__ == "__main__":
    unittest.main()
