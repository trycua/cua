# SPDX-License-Identifier: FSL-1.1-Apache-2.0
import importlib.util
from datetime import datetime, timezone
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "cache_hygiene", Path(__file__).resolve().parents[1] / "cache_hygiene.py")
ch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ch)

NOW = datetime(2026, 11, 10, 12, tzinfo=timezone.utc)


def cache(i, key, ref, created):
    return {"id": i, "key": key, "ref": ref, "created_at": created, "size_in_bytes": 1}


def ids(caches, open_prs=frozenset()):
    return sorted(c["id"] for c, _ in ch.plan(caches, set(open_prs), NOW))


def test_family_strips_date_and_hash():
    assert ch.family("sccache-v2-Linux-X64-libs-cua_20261110") == "sccache-v2-Linux-X64-libs-cua"
    assert ch.family("nix-Linux-Rust unit tests-" + "a" * 64) == "nix-Linux-Rust unit tests"
    # a family is never a prefix of its sibling
    assert ch.family("sccache-v2-Linux-X64-libs-cua_20261110") != ch.family(
        "sccache-v2-Linux-X64-libs-cua-spacesd_20261110")


def test_main_keeps_only_newest_per_family():
    cs = [cache(1, "sccache-v2-Linux-X64-a_20261108", "refs/heads/main", "2026-11-08T01:00:00Z"),
          cache(2, "sccache-v2-Linux-X64-a_20261110", "refs/heads/main", "2026-11-10T01:00:00Z"),
          cache(3, "sccache-v2-Linux-X64-b_20261108", "refs/heads/main", "2026-11-08T01:00:00Z")]
    assert ids(cs) == [1]


def test_pull_requests_closed_deleted_open_kept():
    cs = [cache(1, "k", "refs/pull/5/merge", "2026-11-10T01:00:00Z"),
          cache(2, "k", "refs/pull/6/merge", "2026-11-10T01:00:00Z")]
    assert ids(cs, {6}) == [1]


def test_tags_and_branches_age_out():
    cs = [cache(1, "k", "refs/tags/v1", "2026-11-01T00:00:00Z"),
          cache(2, "k", "refs/tags/v2", "2026-11-10T00:00:00Z"),
          cache(5, "k", "refs/heads/refs/tags/v3", "2026-11-01T00:00:00Z"),
          cache(3, "k", "refs/heads/x", "2026-11-01T00:00:00Z"),
          cache(4, "k", "refs/heads/x", "2026-11-10T00:00:00Z")]
    assert ids(cs) == [1, 3, 5]


def test_never_deletes_sole_main_entry():
    cs = [cache(1, "sccache-v2-Linux-X64-a_20260101", "refs/heads/main", "2026-01-01T00:00:00Z")]
    assert ids(cs) == []


def test_other_main_caches_are_not_families():
    cs = [cache(1, "node-cache-Linux-x64-npm-" + "a" * 40, "refs/heads/main", "2026-11-01T00:00:00Z"),
          cache(2, "node-cache-Linux-x64-npm-" + "b" * 40, "refs/heads/main", "2026-11-10T00:00:00Z")]
    assert ids(cs) == []
