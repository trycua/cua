"""Behavior of push_watch.classify with a fake compare API."""

from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("push_watch", ROOT / ".github/scripts/push_watch.py")
push_watch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(push_watch)

HEAD = "a" * 40
BEFORE = "b" * 40
ZERO = "0" * 40


def event(ref="refs/heads/feat/x", actor="f-trycua", before=ZERO, head=HEAD):
    return {"actor": {"login": actor}, "payload": {"ref": ref, "before": before, "head": head}}


def fake_compare(results):
    def compare(base, head):
        return results[(base, head)]

    return compare


def test_normal_branch_push_is_fine():
    compare = fake_compare({("main", HEAD): (200, {"ahead_by": 3})})
    assert push_watch.classify(event(), "trycua/cua", 300, compare) == []


def test_foreign_history_is_flagged():
    compare = fake_compare({("main", HEAD): (404, None)})
    reasons = push_watch.classify(event(), "trycua/cua", 300, compare)
    assert len(reasons) == 1
    assert "no common ancestor" in reasons[0]
    assert "f-trycua" in reasons[0]


def test_large_new_branch_is_flagged():
    compare = fake_compare({("main", HEAD): (200, {"ahead_by": 10555})})
    reasons = push_watch.classify(event(), "trycua/cua", 300, compare)
    assert len(reasons) == 1 and "10555 commits" in reasons[0]


def test_large_update_uses_the_incremental_count():
    compare = fake_compare(
        {
            ("main", HEAD): (200, {"ahead_by": 900}),
            (BEFORE, HEAD): (200, {"total_commits": 2}),
        }
    )
    assert push_watch.classify(event(before=BEFORE), "trycua/cua", 300, compare) == []


def test_bots_may_push_many_commits_but_not_foreign_history():
    big = fake_compare({("main", HEAD): (200, {"ahead_by": 900})})
    assert push_watch.classify(event(actor="cua-release-bot[bot]"), "trycua/cua", 300, big) == []
    foreign = fake_compare({("main", HEAD): (404, None)})
    assert push_watch.classify(event(actor="cua-release-bot[bot]"), "trycua/cua", 300, foreign)


def test_main_tags_and_deletes_are_ignored():
    never = fake_compare({})
    assert push_watch.classify(event(ref="refs/heads/main"), "trycua/cua", 300, never) == []
    assert push_watch.classify(event(ref="refs/tags/v1"), "trycua/cua", 300, never) == []
    assert push_watch.classify(event(head=""), "trycua/cua", 300, never) == []


def test_allowlisted_orphan_branch_is_not_flagged():
    foreign = fake_compare({("main", HEAD): (404, None)})
    ev = event(ref="refs/heads/image-doctor-ledger", actor="cua-image-doctor")
    assert push_watch.classify(ev, "trycua/cua", 300, foreign, frozenset({"image-doctor-ledger"})) == []
    assert push_watch.classify(ev, "trycua/cua", 300, foreign)
