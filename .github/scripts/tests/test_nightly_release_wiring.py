from __future__ import annotations

import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github/workflows"


def source(name: str) -> str:
    return (WORKFLOWS / name).read_text()


def test_driver_stable_publish_gate_and_workflow_name_are_frozen():
    driver = source("cd-rust-cua-driver.yml")
    assert driver.startswith('name: "CD: Cua Driver (cross-platform)"')
    assert "if: github.event_name == 'workflow_dispatch' && inputs.publish == true" in driver
    sdk = source("cd-py-cua-driver.yml")
    assert 'workflows: ["CD: Cua Driver (cross-platform)"]' in sdk


def test_lume_stable_publish_gate_remains_tag_only():
    lume = source("cd-swift-lume.yml")
    assert "if: startsWith(github.ref, 'refs/tags/lume-v')" in lume
    assert "BUNDLE_VERSION: ${{ inputs.bundle_version || steps.set_version.outputs.version }}" in lume


def test_driver_nightly_reuses_builder_without_stable_state_mutation():
    nightly = source("nightly-cua-driver.yml")
    assert "uses: ./.github/workflows/cd-rust-cua-driver.yml" in nightly
    assert "channel: nightly" in nightly
    assert "--create-if-missing" in nightly
    assert "--prerelease" in nightly
    assert "--make-latest" not in nightly
    assert "update_cua_driver_installer_version.py" not in nightly
    assert ".github/release-state" not in nightly
    assert "release_attribution.py collect" not in nightly


def test_lume_nightly_reuses_notarized_builder_and_never_becomes_latest():
    nightly = source("nightly-lume.yml")
    assert "uses: ./.github/workflows/cd-swift-lume.yml" in nightly
    assert "bundle_version: ${{ needs.plan.outputs.bundle_version }}" in nightly
    assert "--create-if-missing" in nightly
    assert "--prerelease" in nightly
    assert "--make-latest" not in nightly


def test_planner_requires_main_ancestry_and_preserves_immutable_evidence():
    planner = source("nightly-component-plan.yml")
    assert 'SOURCE_SHA: ${{ inputs.source_sha }}' in planner
    assert 'git merge-base --is-ancestor "$SOURCE_SHA" origin/main' in planner
    assert "fetch-depth: 0" in planner
    assert "nightly-plan-${{ inputs.component }}" in planner
    assert "cancel-in-progress: false" in source("nightly-cua-driver.yml")
    assert "cancel-in-progress: false" in source("nightly-lume.yml")


def test_registered_builder_workflows_match_the_orchestrators():
    components = json.loads((ROOT / ".github/releases/components.json").read_text())[
        "components"
    ]
    assert components["cua-driver-rs"]["builderWorkflow"] == (
        ".github/workflows/cd-rust-cua-driver.yml"
    )
    assert components["lume"]["builderWorkflow"] == ".github/workflows/cd-swift-lume.yml"
    for component in components.values():
        assert (ROOT / component["builderWorkflow"]).is_file()


def test_driver_nightly_version_is_staged_in_each_platform_builder():
    driver = source("cd-rust-cua-driver.yml")
    command = (
        "release_channels.py apply-version --component cua-driver-rs --version"
    )
    assert driver.count(command) == 3
    assert "inputs.source_ref || github.workflow_sha" in driver
    assert 'BUNDLE_VERSION="${VERSION%%-*}"' in driver
    assert 'CFBundleShortVersionString -string "$BUNDLE_VERSION"' in driver
    assert 'CFBundleVersion -string "$BUNDLE_VERSION"' in driver


def test_nightly_workflow_names_cannot_trigger_stable_driver_sdk_publish():
    for name in ("nightly-cua-driver.yml", "nightly-lume.yml"):
        first_line = source(name).splitlines()[0]
        assert "CD: Cua Driver (cross-platform)" not in first_line
