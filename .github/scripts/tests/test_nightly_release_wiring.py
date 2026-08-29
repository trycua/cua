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
    assert (
        "BUNDLE_VERSION: ${{ inputs.bundle_version || steps.set_version.outputs.version }}" in lume
    )


def test_driver_nightly_reuses_builder_without_stable_state_mutation():
    nightly = source("nightly-cua-driver.yml")
    assert "uses: ./.github/workflows/cd-rust-cua-driver.yml" in nightly
    assert "channel: nightly" in nightly
    assert "--create-if-missing" in nightly
    assert "--prerelease" in nightly
    assert "--make-latest" not in nightly
    assert "update_cua_driver_installer_version.py" not in nightly
    assert ".github/release-state" not in nightly
    assert "Collect PR-first attribution and render nightly body" in nightly
    assert "GH_TOKEN: ${{ github.token }}" in nightly
    assert "needs.plan.outputs.attribution_base_tag" in nightly
    assert "issues: write" in nightly
    assert "pull-requests: read" in nightly
    assert "release_channels.py apply-version" not in nightly
    assert "release_channels.py stage-versioned-tree" in nightly
    assert nightly.index("stage-versioned-tree") < nightly.index(
        "Collect PR-first attribution and render nightly body"
    )


def test_lume_nightly_reuses_notarized_builder_and_never_becomes_latest():
    nightly = source("nightly-lume.yml")
    builder = source("cd-swift-lume.yml")
    assert "uses: ./.github/workflows/cd-swift-lume.yml" in nightly
    assert "bundle_version: ${{ needs.plan.outputs.bundle_version }}" in nightly
    assert "--create-if-missing" in nightly
    assert "--prerelease" in nightly
    assert "--make-latest" not in nightly
    assert "Collect PR-first attribution and render nightly body" in nightly
    assert "GH_TOKEN: ${{ github.token }}" in nightly
    assert "needs.plan.outputs.attribution_base_tag" in nightly
    assert "issues: write" in nightly
    assert "pull-requests: read" in nightly
    assert builder.index("- name: Set version") < builder.index(
        "- name: Stage nightly artifact version"
    )
    set_version = builder[
        builder.index("- name: Set version") : builder.index(
            "- name: Stage nightly artifact version"
        )
    ]
    assert set_version.index('inputs.channel }}" == "nightly"') < set_version.index(
        "SOURCE_VERSION=$(tr -d"
    )


def test_planner_requires_main_ancestry_and_preserves_immutable_evidence():
    planner = source("nightly-component-plan.yml")
    assert "SOURCE_SHA: ${{ inputs.source_sha }}" in planner
    assert 'git merge-base --is-ancestor "$SOURCE_SHA" origin/main' in planner
    assert "fetch-depth: 0" in planner
    assert "nightly-plan-${{ inputs.component }}" in planner
    assert "attribution_base_tag" in planner
    assert "attribution_issues" in planner
    assert "--attribution-config .github/release-attribution-config.json" in planner
    assert "needs.plan.outputs.reason == 'held-attribution'" in planner
    assert "gh issue create" in planner
    assert "gh issue edit" in planner
    assert "cancel-in-progress: false" in source("nightly-cua-driver.yml")
    assert "cancel-in-progress: false" in source("nightly-lume.yml")


def test_registered_builder_workflows_match_the_orchestrators():
    components = json.loads((ROOT / ".github/releases/components.json").read_text())["components"]
    assert components["cua-driver-rs"]["builderWorkflow"] == (
        ".github/workflows/cd-rust-cua-driver.yml"
    )
    assert components["lume"]["builderWorkflow"] == ".github/workflows/cd-swift-lume.yml"
    for component in components.values():
        assert (ROOT / component["builderWorkflow"]).is_file()


def test_driver_nightly_version_is_staged_in_each_platform_builder():
    driver = source("cd-rust-cua-driver.yml")
    command = "release_channels.py apply-version --component cua-driver-rs --version"
    assert driver.count(command) == 3
    assert "inputs.source_ref || github.workflow_sha" in driver
    assert 'BUNDLE_VERSION="${VERSION%%-*}"' in driver
    assert 'CFBundleShortVersionString -string "$BUNDLE_VERSION"' in driver
    assert 'CFBundleVersion -string "$BUNDLE_VERSION"' in driver


def test_nightly_workflow_names_cannot_trigger_stable_driver_sdk_publish():
    for name in ("nightly-cua-driver.yml", "nightly-lume.yml"):
        first_line = source(name).splitlines()[0]
        assert "CD: Cua Driver (cross-platform)" not in first_line


def test_release_control_ci_runs_for_every_nightly_definition():
    test_workflow = source("ci-test-scripts.yml")
    for path in (
        ".github/releases/**",
        ".github/workflows/nightly-component-plan.yml",
        ".github/workflows/nightly-cua-driver.yml",
        ".github/workflows/nightly-lume.yml",
    ):
        assert f'- "{path}"' in test_workflow
