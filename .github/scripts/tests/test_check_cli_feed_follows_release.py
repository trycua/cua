"""Tests for check_cli_feed_follows_release.py with fake release and manifest data."""

from __future__ import annotations

import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import check_cli_feed_follows_release as feed  # noqa: E402

NOW = datetime(2026, 11, 11, 12, 0, tzinfo=timezone.utc)
GRACE = timedelta(minutes=120)


def rel(version, hours_ago=10, draft=False, pre=False):
    return {
        "tagName": f"cua-sdk-v{version}",
        "isDraft": draft,
        "isPrerelease": pre,
        "publishedAt": (NOW - timedelta(hours=hours_ago)).isoformat().replace("+00:00", "Z"),
    }


def manifest(cli, platforms=("linux-x64", "darwin-arm64")):
    return {
        "versions": {"cli": cli, "app": "0.33.7"},
        "artifacts": [{"component": "cli", "platform": p, "version": cli} for p in platforms]
        + [{"component": "app", "platform": "darwin-arm64", "version": "0.33.7"}],
    }


def test_current_feed_is_clean():
    assert feed.feed_problems([rel("0.24.0"), rel("0.23.0")], manifest("0.24.0"), NOW, GRACE) == []


def test_feed_stuck_on_old_cli_is_named():
    # The #6923 shape: 0.14.0 published, the feed still on 0.10.1.
    problems = feed.feed_problems([rel("0.14.0"), rel("0.10.1", 300)], manifest("0.10.1"), NOW, GRACE)
    assert len(problems) == 1
    assert "0.10.1" in problems[0] and "cua-sdk-v0.14.0" in problems[0]


def test_numeric_not_lexical_version_order():
    problems = feed.feed_problems([rel("0.9.0"), rel("0.10.0")], manifest("0.9.0"), NOW, GRACE)
    assert problems and "cua-sdk-v0.10.0" in problems[0]


def test_release_inside_grace_window_is_not_flagged():
    assert feed.feed_problems([rel("0.15.0", hours_ago=1)], manifest("0.14.0"), NOW, GRACE) == []


def test_drafts_and_prereleases_are_ignored():
    releases = [rel("0.20.0", draft=True), rel("0.19.0", pre=True), rel("0.18.0")]
    assert feed.feed_problems(releases, manifest("0.18.0"), NOW, GRACE) == []


def test_non_cli_tags_and_suffix_versions_are_ignored():
    releases = [
        {"tagName": "cua-spaces-v9.9.9", "isDraft": False, "isPrerelease": False, "publishedAt": "2026-11-01T00:00:00Z"},
        rel("0.30.0-rc1"),
        rel("0.18.0"),
    ]
    assert feed.feed_problems(releases, manifest("0.18.0"), NOW, GRACE) == []


def test_stale_platform_entry_is_named():
    m = manifest("0.24.0")
    m["artifacts"][0]["version"] = "0.10.1"
    problems = feed.feed_problems([rel("0.24.0")], m, NOW, GRACE)
    assert len(problems) == 1 and "linux-x64=0.10.1" in problems[0]


def test_no_releases_is_clean():
    assert feed.feed_problems([], manifest("0.1.0"), NOW, GRACE) == []
