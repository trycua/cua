"""pypi_retention_guard refuses delete lists that break docs/pypi-retention.md."""

import datetime as dt
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import pypi_retention_guard as guard  # noqa: E402

TODAY = dt.date(2026, 11, 12)
OLD = "2026-06-01T00:00:00Z"  # > 90 days before TODAY
NEW = "2026-10-01T00:00:00Z"  # < 90 days before TODAY


def files(when=OLD, yanked=True):
    return [{"upload_time_iso_8601": when, "yanked": yanked, "size": 1}]


def fake_pypi(driver_releases, others=None):
    """fetch() over a tiny fake index: cua-driver plus `others` {project: {version: requires_dist}}."""
    others = others or {}
    docs = {"cua-driver": {"releases": driver_releases}}
    for p, rels in others.items():
        docs[p] = {"releases": {v: files() for v in rels}}
        for v, reqs in rels.items():
            docs[f"{p}/{v}"] = {"info": {"requires_dist": reqs}}
    for v in driver_releases:
        docs.setdefault(f"cua-driver/{v}", {"info": {"requires_dist": None}})
    return docs.get


def run(versions, releases, others=None, roots=(), announced=dt.date(2026, 10, 1), text=None):
    if text is None:
        text = "Removing from PyPI: " + ", ".join(versions)
    return guard.check(
        "cua-driver", versions, today=TODAY, announced_on=announced, announcement_text=text,
        fetch=fake_pypi(releases, others), repo_roots=roots, projects=["cua-driver", *(others or {})],
    )


BASE = {
    "0.12.2": files(), "0.12.3": files(), "0.12.5": files(),
    "0.45.2": files(), "0.45.4": files(),
    "0.47.1": files(NEW), "0.47.2": files(NEW),
}


def test_a_compliant_list_passes():
    assert run(["0.12.2", "0.12.3"], BASE) == []


def test_latest_patch_of_a_minor_is_kept():
    problems = run(["0.12.5"], BASE)
    assert any("newest patch of 0.12" in p for p in problems)


def test_younger_than_90_days_is_refused():
    problems = run(["0.47.1"], BASE)
    assert any("minimum 90" in p for p in problems)


def test_must_be_yanked_first():
    rel = dict(BASE, **{"0.12.2": files(yanked=False)})
    assert any("not yanked" in p for p in run(["0.12.2"], rel))


def test_announcement_needs_30_days_and_must_name_the_version():
    assert any("only 11 days" in p for p in run(["0.12.2"], BASE, announced=dt.date(2026, 11, 1)))
    assert any("no announcement" in p for p in run(["0.12.2"], BASE, announced=None))
    assert any("not named" in p for p in run(["0.12.2"], BASE, text="Removing 0.12.20"))


def test_missing_version_is_refused():
    assert any("not on PyPI" in p for p in run(["0.9.9"], BASE))


def test_a_published_exact_pin_blocks_deletion():
    # cua-sandbox 0.9.1 requires cua-driver==0.28.2 (the 12 Nov breakage).
    others = {"cua-sandbox": {"0.9.1": ['cua-driver==0.45.2; extra == "driver"']}}
    problems = run(["0.45.2"], BASE, others)
    assert any("cua-sandbox==0.9.1" in p for p in problems)


def test_a_published_range_blocks_only_when_nothing_is_left():
    others = {"cua-agent": {"1.0": ["cua-driver<0.13,>=0.12"]}}
    assert run(["0.12.2", "0.12.3"], BASE, others) == []
    narrow = {"cua-agent": {"1.0": ["cua-driver<0.12.4,>=0.12"]}}
    assert any("cua-agent==1.0" in p for p in run(["0.12.2", "0.12.3"], BASE, narrow))


def test_a_repo_pin_blocks_deletion(tmp_path):
    (tmp_path / "requirements.txt").write_text("cua-driver==0.12.3\n")
    problems = run(["0.12.2", "0.12.3"], BASE, roots=[tmp_path])
    assert any("requirements.txt:1" in p for p in problems)
    (tmp_path / "requirements.txt").write_text("cua-driver[mcp]~=0.12.2\n")
    assert run(["0.12.2", "0.12.3"], BASE, roots=[tmp_path]) == []


def test_a_uv_lock_entry_blocks_deletion(tmp_path):
    (tmp_path / "uv.lock").write_text('[[package]]\nname = "cua-driver"\nversion = "0.12.2"\n')
    assert any("uv.lock" in p for p in run(["0.12.2"], BASE, roots=[tmp_path]))


def test_other_package_names_are_not_pins(tmp_path):
    (tmp_path / "x.md").write_text("cua-driver-rs==0.12.2 and @trycua/cua-driver@0.12.2\n")
    assert run(["0.12.2"], BASE, roots=[tmp_path]) == []


def test_read_delete_list(tmp_path):
    p = tmp_path / "list.txt"
    p.write_text("# announced: 2026-10-01 https://example.test/notes\n0.12.2\n\n0.12.3  # dup minor\n")
    assert guard.read_delete_list(p) == (["0.12.2", "0.12.3"], dt.date(2026, 10, 1), "https://example.test/notes")


def test_the_12_nov_list_would_have_been_refused():
    # 0.45.2 was the partner pin: it was 0.45.x's middle patch but under 90 days old.
    rel = dict(BASE, **{"0.45.2": files("2026-10-20T00:00:00Z")})
    assert any("0.45.2" in p and "minimum 90" in p for p in run(["0.45.2"], rel))
