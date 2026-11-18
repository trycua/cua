import datetime as dt
import importlib.util
import pathlib

spec = importlib.util.spec_from_file_location(
    "sweep", pathlib.Path(__file__).resolve().parents[1] / "sweep_draft_releases.py"
)
sweep = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sweep)

NOW = dt.datetime(2026, 11, 10, 16, 0, tzinfo=dt.timezone.utc)


def rel(tag, name, draft, hours_old=48, prerelease=False):
    created = NOW - dt.timedelta(hours=hours_old)
    return {
        "id": hash((tag, name)) & 0xFFFF,
        "tag_name": tag,
        "name": name,
        "draft": draft,
        "prerelease": prerelease,
        "created_at": created.isoformat().replace("+00:00", "Z"),
    }


def names(out):
    return sorted(r["name"] for r, _ in out)


def test_superseded_by_published_untagged_and_tagged():
    rels = [
        rel("cua-spaces-v0.31.1", "Cua Spaces v0.31.1", False, 1),
        rel("untagged-a", "cua-spaces: v0.25.2", True),
        rel("cua-spaces-v0.25.2", "Cua Spaces v0.25.2", True),
    ]
    assert names(sweep.pick_deletions(rels, [], NOW)) == ["Cua Spaces v0.25.2", "cua-spaces: v0.25.2"]


def test_newest_draft_and_recent_drafts_are_kept():
    rels = [
        rel("cua-driver-rs-v0.46.1", "cua-driver-rs: v0.46.1", False),
        rel("cua-driver-rs-v0.47.0", "cua-driver-rs: v0.47.0", True),
        rel("cua-driver-rs-v0.47.1", "cua-driver-rs: v0.47.1", True),
        rel("cua-driver-rs-v0.47.2", "cua-driver-rs: v0.47.2", True, 2),
    ]
    assert names(sweep.pick_deletions(rels, [], NOW)) == ["cua-driver-rs: v0.47.0", "cua-driver-rs: v0.47.1"]


def test_newest_old_draft_is_kept():
    rels = [rel("cua-driver-rs-v0.47.2", "cua-driver-rs: v0.47.2", True, 30)]
    assert sweep.pick_deletions(rels, [], NOW) == []


def test_active_run_keeps_draft():
    rels = [
        rel("cua-spaces-v0.31.1", "Cua Spaces v0.31.1", False, 1),
        rel("cua-spaces-v0.25.2", "Cua Spaces v0.25.2", True),
    ]
    runs = [{"head_branch": "cua-spaces-v0.25.2", "status": "in_progress", "created_at": "2026-11-09T00:00:00Z"}]
    assert sweep.pick_deletions(rels, runs, NOW) == []
    runs = [{"head_branch": "cua-spaces-v0.25.2", "status": "completed", "created_at": "2026-11-10T14:00:00Z"}]
    assert sweep.pick_deletions(rels, runs, NOW) == []


def test_unclassified_and_other_components_untouched():
    rels = [
        rel("cua-spaces-v0.31.1", "Cua Spaces v0.31.1", False, 1),
        rel("cua-perception-review-input-20260917", "Cua Perception review input (unpublished)", True),
        rel("lume-v0.19.0", "lume: v0.19.0", True),
        rel("cua-spaces-v0.40.0-beta.1", "Cua Spaces v0.40.0-beta.1", True, prerelease=True),
    ]
    assert sweep.pick_deletions(rels, [], NOW) == []
