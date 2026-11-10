"""The Spaces nightly tag script: immutable, idempotent, loud on a mismatch."""

import os
import re
import stat
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / ".github/scripts/spaces_nightly_tag.sh"
WORKFLOW = (ROOT / ".github/workflows/nightly-cua-spaces.yml").read_text()
SHA = "a" * 40
OTHER = "b" * 40
TAG = "cua-spaces-nightly-20261109.57"

# A fake `gh`: GET answers from $FAKE_REF, POST records itself in $FAKE_LOG.
FAKE_GH = """#!/usr/bin/env bash
echo "$*" >> "$FAKE_LOG"
case "$*" in
  *"-X POST"*) [ -z "${FAKE_POST_FAILS:-}" ] || { echo "422 exists" >&2; exit 1; }; exit 0 ;;
esac
if [ -n "${FAKE_REF:-}" ]; then echo "$FAKE_REF"; exit 0; fi
echo "gh: Not Found (HTTP 404)" >&2; exit 1
"""


def run(tmp_path, *args, ref="", post_fails=False):
    gh = tmp_path / "gh"
    gh.write_text(FAKE_GH)
    gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
    log = tmp_path / "log"
    env = {
        **os.environ,
        "PATH": f"{tmp_path}:{os.environ['PATH']}",
        "GITHUB_REPOSITORY": "trycua/cua",
        "FAKE_LOG": str(log),
        "FAKE_REF": ref,
        "FAKE_POST_FAILS": "1" if post_fails else "",
    }
    p = subprocess.run(["bash", str(SCRIPT), *args], env=env, capture_output=True, text=True)
    calls = log.read_text().splitlines() if log.exists() else []
    return p, calls


def posts(calls):
    return [c for c in calls if "-X POST" in c]


def test_creates_a_lightweight_tag_at_the_built_sha(tmp_path):
    p, calls = run(tmp_path, TAG, SHA)
    assert p.returncode == 0, p.stderr
    assert len(posts(calls)) == 1
    assert f"ref=refs/tags/{TAG}" in posts(calls)[0] and f"sha={SHA}" in posts(calls)[0]
    assert not any("PATCH" in c or "--force" in c for c in calls)


def test_dry_run_writes_nothing(tmp_path):
    p, calls = run(tmp_path, "--dry-run", TAG, SHA)
    assert p.returncode == 0 and "DRY RUN" in p.stdout
    assert posts(calls) == []


def test_same_commit_is_fine_on_a_rerun(tmp_path):
    p, calls = run(tmp_path, TAG, SHA, ref=f"commit {SHA}")
    assert p.returncode == 0 and posts(calls) == []


def test_tag_elsewhere_fails_loudly_and_is_not_moved(tmp_path):
    p, calls = run(tmp_path, TAG, SHA, ref=f"commit {OTHER}")
    assert p.returncode == 1 and "immutable" in p.stdout
    assert posts(calls) == []


def test_annotated_tag_object_is_not_accepted(tmp_path):
    p, _ = run(tmp_path, TAG, SHA, ref=f"tag {SHA}")
    assert p.returncode == 1


def test_failed_create_fails_without_retrying(tmp_path):
    # Create fails and the tag is still not there: fail, never retry with a force.
    p, calls = run(tmp_path, TAG, SHA, post_fails=True)
    assert p.returncode == 1
    assert len(posts(calls)) == 1


@pytest.mark.parametrize("tag", ["cua-spaces-nightly", "cua-spaces-nightly-1.57", "cua-spaces-v0.25.0", "x"])
def test_rejects_names_that_are_not_nightly_tags(tmp_path, tag):
    p, calls = run(tmp_path, tag, SHA)
    assert p.returncode == 1 and calls == []


def test_rejects_a_short_sha(tmp_path):
    p, calls = run(tmp_path, TAG, "abc1234")
    assert p.returncode == 1 and calls == []


def test_workflow_wiring():
    # The tag is made after publish and the channel check, only for a full success,
    # never matches the stable tag trigger, and the rolling release is untouched.
    assert "cua-spaces-nightly-$day.$GITHUB_RUN_NUMBER" in WORKFLOW
    assert "spaces_nightly_tag.sh" in WORKFLOW
    assert re.search(r"Check the channel.*Tag the commit", WORKFLOW, re.S)
    assert re.search(r"name: Tag the commit\n\s+if: needs\.build\.result == 'success'", WORKFLOW)
    assert "TAG: cua-spaces-nightly\n" in WORKFLOW
    assert not re.search(r"cancel-in-progress: true", WORKFLOW)
