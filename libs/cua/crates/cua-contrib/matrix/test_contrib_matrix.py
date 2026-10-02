"""The support-matrix generator against doctor reports recorded from real
runs (fixtures/, trimmed; see each file's ``_recorded``): a Fleet gVisor
desktop that passed, the same image with a broken display, and a sandbox
whose cua-spacesd never answered. Stdlib + pytest; no network."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

import contrib_matrix as m

FIXTURES = Path(__file__).resolve().parent / "fixtures"


def report(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text())


def test_recorded_reports_get_measured_verdicts():
    full = m.summarize(report("doctor-full.json"))
    assert full["verdict"] == "full", full
    assert full["groups"]["display"] == "pass" and full["groups"]["input"] == "pass"

    broken = m.summarize(report("doctor-display-broken.json"))
    assert broken["verdict"] == "container-only", broken
    assert broken["groups"]["display"] == "fail"
    assert broken["groups"]["process"] == "pass"

    none = m.summarize(report("doctor-no-guest.json"))
    assert none["verdict"] == "headless-only", none
    assert none["guest"] is False


@pytest.fixture
def ledger(tmp_path, monkeypatch):
    monkeypatch.setattr(m, "LEDGER", tmp_path / "ledger")
    return tmp_path / "ledger"


def test_ingest_render_and_regressions(ledger):
    table = m.render()
    assert "| e2b | container | not measured |" in table
    assert "| daytona | container | not measured |" in table

    m.ingest("e2b", "container", "ghcr.io/trycua/linux:24.04", report("doctor-full.json"),
             sha="abc", url="https://example.invalid/run/1", at="2026-09-24T01:00:00Z")
    m.fail("daytona", "container", "ghcr.io/trycua/linux:24.04", "snapshot build failed",
           at="2026-09-24T01:00:00Z")
    table = m.render()
    assert "| e2b | container | full | pass |" in table
    assert "[2026-09-24](https://example.invalid/run/1)" in table
    assert "| daytona | container | failed |" in table

    # A later run with a broken display is a regression the table shows.
    m.ingest("e2b", "container", "ghcr.io/trycua/linux:24.04",
             report("doctor-display-broken.json"), at="2026-09-25T01:00:00Z")
    table = m.render()
    assert "| e2b | container | container-only | FAIL |" in table
    saved = json.loads((ledger / "e2b-container.json").read_text())
    assert saved["verdict"] == "container-only" and saved["groups"]["display"] == "fail"


def test_the_docs_region_is_generated_and_checked(ledger, tmp_path):
    page = tmp_path / "page.mdx"
    page.write_text(
        "# Page\n\n{/* GENERATED:contrib-matrix:start */}\nstale\n"
        "{/* GENERATED:contrib-matrix:end */}\n\nAfter.\n"
    )
    assert m.main(["check", "--page", str(page)]) == 1
    assert m.main(["write", "--page", str(page)]) == 0
    assert m.main(["check", "--page", str(page)]) == 0
    text = page.read_text()
    assert "stale" not in text and "| e2b | container | not measured |" in text
    assert text.endswith("After.\n")


def test_the_published_page_matches_the_ledger():
    """The docs page in the repo is up to date with the committed ledger."""
    assert m.main(["check"]) == 0
