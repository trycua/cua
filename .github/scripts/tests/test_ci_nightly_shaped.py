"""Nightly-shaped jobs stay off ordinary pull requests, and are heard when they fail at night."""

from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]

# workflow -> (paths that still start it on a pull request, a path that must not, a path main runs it for)
CASES = {
    "e2b-bindings.yml": ("libs/cua/swift/Package.swift", "libs/cua/crates/cua-sdk/src/lib.rs", "libs/cua/crates/cua-sdk/src/lib.rs"),
    "ci-cua-driver-preflight.yml": (
        "scripts/ci/linux/preflight-rust-e2e.sh",
        "libs/cua-driver/rust/crates/cua-driver/src/main.rs",
        "libs/cua-driver/rust/crates/cua-driver/src/main.rs",
    ),
    "ci-nix-linux.yml": ("flake.lock", "libs/cua-driver/rust/crates/cua-driver/src/main.rs", "libs/cua-driver/rust/crates/cua-driver/src/main.rs"),
}


def load(name):
    doc = yaml.safe_load((ROOT / ".github/workflows" / name).read_text())
    return doc, doc.get("on", doc.get(True))


def covered(patterns, path):
    hit = False
    for raw in patterns:
        neg, pat = raw.startswith("!"), raw.lstrip("!")
        if pat.endswith("/**"):
            ok = path.startswith(pat[:-3] + "/")
        elif "*" in pat:
            continue
        else:
            ok = path == pat
        if ok:
            hit = not neg
    return hit


@pytest.mark.parametrize("name", CASES)
def test_pull_requests_run_it_only_for_the_paths_that_matter(name):
    _, on = load(name)
    keeps, skips, main_runs = CASES[name]
    assert covered(on["pull_request"]["paths"], keeps)
    assert not covered(on["pull_request"]["paths"], skips)
    assert covered(on["push"]["paths"], main_runs)
    assert on["push"]["branches"] == ["main"]


@pytest.mark.parametrize("name", CASES)
def test_it_runs_every_night_and_a_failure_opens_an_issue(name):
    doc, on = load(name)
    assert on["schedule"]
    report = doc["jobs"]["report"]
    assert "failure()" in report["if"] and "pull_request" in report["if"]
    assert report["permissions"] == {"issues": "write"}
    assert any(s.get("uses") == "./.github/actions/nightly-issue" for s in report["steps"])
    needs = report["needs"] if isinstance(report["needs"], list) else [report["needs"]]
    assert set(needs) <= set(doc["jobs"]) - {"report"}


def test_the_nightly_failures_also_reach_alertmanager():
    text = (ROOT / ".github/workflows/workflow-failure-alerts.yml").read_text()
    for name in ("e2b-bindings", "CI: Cua Driver desktop readiness (non-certifying)", "CI: Nix Linux Rust source"):
        assert f'- "{name}"' in text and f"{name}=ci" in text
