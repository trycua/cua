"""The Tauri installer bundles must not build on plain pull requests.

The Tauri app (apps/cua-spaces) is not what ships, and its bundles take 60 to
100 minutes cold. Pull requests keep the cheap frontend and Rust command-layer
checks; the bundles build on main, on manual runs and on `ci:full` PRs.
"""

from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = yaml.safe_load((ROOT / ".github/workflows/ci-cua-spaces.yml").read_text())
JOBS = WORKFLOW["jobs"]


def test_bundle_is_skipped_on_plain_pull_requests():
    condition = JOBS["bundle"]["if"]
    assert "github.event_name != 'pull_request'" in condition
    assert "contains(github.event.pull_request.labels.*.name, 'ci:full')" in condition
    assert JOBS["bundle"]["needs"] == "changes"


def test_bundle_still_builds_on_main_and_manual_runs():
    triggers = WORKFLOW[True]
    assert triggers["push"]["branches"] == ["main"]
    assert "workflow_dispatch" in triggers


def test_pull_requests_keep_the_cheap_tauri_checks():
    # `rust` compiles and tests src-tauri; `frontend` runs vitest + typecheck.
    for name in ("rust", "frontend"):
        condition = JOBS[name].get("if", "")
        assert "pull_request" not in condition and "ci:full" not in condition
    assert "src-tauri/Cargo.toml" in str(JOBS["rust"]["steps"])
