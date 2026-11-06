# SPDX-License-Identifier: FSL-1.1-Apache-2.0
"""One macOS producer per libs/cua tree (CUA-3562).

The main-branch producers (SwiftUI Rust slice, Electron macOS natives) are
stages of cd-cua-spaces-build.yml, called once per tree, instead of
workflows of their own that build beside it. A tag run restores what they
saved. These checks keep it that way.
"""

from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
WF = ROOT / ".github/workflows"


def load(name):
    return yaml.safe_load((WF / name).read_text())


BUILD = load("cd-cua-spaces-build.yml")
SLICE = load("ci-cua-spaces-rust-slice.yml")
NATIVES = load("cd-cua-spaces-natives-main.yml")


def test_producers_have_no_push_trigger_of_their_own():
    for wf in (SLICE, NATIVES):
        assert "workflow_call" in wf[True]
        assert "push" not in wf[True]
        assert "concurrency" not in wf


def test_the_build_calls_both_producers_once_per_tree():
    jobs = BUILD["jobs"]
    assert jobs["slice"]["uses"] == "./.github/workflows/ci-cua-spaces-rust-slice.yml"
    assert jobs["natives-mac"]["uses"] == "./.github/workflows/cd-cua-spaces-natives-main.yml"
    for name in ("slice", "natives-mac"):
        assert "plan" in jobs[name]["needs"]
        assert "needs.plan.outputs.skip != 'true'" in (jobs[name].get("if") or "needs.plan.outputs.skip != 'true'")
    # The stable build set does not wait for the beta-only natives.
    for name in ("assemble", "verify", "attest"):
        assert "natives-mac" not in jobs[name].get("needs", [])


def test_the_consumer_restores_and_alerts_on_a_miss_instead_of_building():
    macos = BUILD["jobs"]["macos-rust"]
    assert "slice" in macos["needs"]
    steps = macos["steps"]
    use = next(s for s in steps if s.get("uses") == "./.github/actions/spaces-rust-slice")
    assert use["with"]["build"] == "false"
    miss = next(s for s in steps if s.get("name", "").startswith("The producer left the slice"))
    assert "cache-hit != 'true'" in miss["if"] and "exit 1" in miss["run"]


def test_natives_producer_builds_only_macos_the_pipeline_builds_the_rest():
    include = NATIVES["jobs"]["electron-native"]["strategy"]["matrix"]["include"]
    assert {i["label"] for i in include} == {"darwin-arm64", "darwin-x64"}
    assert all(i["runner"] == "macos-26" for i in include)
    build_labels = {i["label"] for i in BUILD["jobs"]["electron-native"]["strategy"]["matrix"]["include"]}
    assert {"win32-x64", "win32-arm64", "linux-x64", "linux-arm64"} <= build_labels


def test_the_build_is_watched_by_the_failure_alerts():
    alerts = load("workflow-failure-alerts.yml")
    assert "CD: Cua Spaces build (shadow)" in alerts[True]["workflow_run"]["workflows"]
    assert "CD: Cua Spaces build (shadow)=" in alerts["jobs"]["alert"]["env"]["WATCH"]


def test_the_tree_hash_covers_every_trigger_path_outside_the_listed_directories():
    tree = (ROOT / ".github/scripts/spaces_inputs_tree.sh").read_text()
    for path in ("libs/images/sandbox-images.json", ".cargo", ".github/scripts/spaces_rust_slice_inputs.py",
                 ".github/workflows/ci-cua-spaces-rust-slice.yml", ".github/workflows/cd-cua-spaces-natives-main.yml"):
        assert path in tree, path
