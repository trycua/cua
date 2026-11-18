"""Contract for the Cua Spaces rebuild dispatch (rebuild_tag, rebuild_sha).

An existing stable tag whose tag-push run never published can be rebuilt and
published from a workflow_dispatch on main, with main's workflow files. Every
release gate must behave exactly as on a tag push, a published release must be
refused, and no tag is ever moved or created.
"""

from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
CD = ROOT / ".github/workflows/cd-cua-spaces.yml"
TAG_RUN = (
    "(startsWith(github.ref, 'refs/tags/cua-spaces-v') || "
    "(github.event_name == 'workflow_dispatch' && inputs.rebuild_tag != ''))"
)
# Jobs that build or verify the tagged source check out the tagged commit.
SOURCE_JOBS = {
    "spacesd-published",
    "macos-rust",
    "build-macos",
    "build-linux-windows",
    "electron-plan",
    "electron-web",
    "electron-native",
    "electron-native-mac",
    "electron-mac",
    "electron-windows-pkg",
    "electron-windows",
    "electron-linux-pkg",
    "electron-publish",
    "verify-release",
}
# Publishing and gate tooling stays on main, so fixed workflow files and the
# current installers apply to the old tag.
MAIN_TOOLING_JOBS = {
    "request-staging-smoke",
    "promote-check",
    "staging-gate",
    "installer-manifest",
    "site-follows-release",
}
CHECKOUT_REF = "${{ inputs.rebuild_sha || github.sha }}"


def load():
    text = CD.read_text()
    return text, yaml.safe_load(text)


def dispatch_inputs(workflow):
    return workflow[True]["workflow_dispatch"]["inputs"]  # PyYAML reads `on` as True


def test_rebuild_inputs_exist_and_default_empty():
    _, workflow = load()
    inputs = dispatch_inputs(workflow)
    for name in ("rebuild_tag", "rebuild_sha"):
        assert inputs[name]["default"] == ""
        assert inputs[name]["required"] is False
    assert "push" in workflow[True]  # the tag push is still the normal path


def test_release_identity_comes_from_the_rebuilt_tag():
    text, workflow = load()
    env = workflow["env"]
    assert env["RELEASE_REF"] == (
        "${{ inputs.rebuild_tag && format('refs/tags/{0}', inputs.rebuild_tag) || github.ref }}"
    )
    assert env["RELEASE_REF_NAME"] == "${{ inputs.rebuild_tag || github.ref_name }}"
    assert env["RELEASE_SHA"] == "${{ inputs.rebuild_sha || github.sha }}"
    assert "inputs.rebuild_tag != ''" in env["REQUIRE_SIGNING"]
    concurrency = workflow["concurrency"]
    # A rebuild shares the tag's group (never two runs of one release) and is
    # never cancelled.
    assert "format('refs/tags/{0}', inputs.rebuild_tag)" in concurrency["group"]
    assert "!inputs.rebuild_tag" in concurrency["cancel-in-progress"]
    # No shell step reads the dispatch ref or SHA as if it were the release.
    for forbidden in ("$GITHUB_REF_NAME", "${GITHUB_REF_NAME", '"$GITHUB_SHA"'):
        if forbidden == '"$GITHUB_SHA"':
            # Only the prerelease notes name the run's own ref and SHA.
            assert text.count(forbidden) == 0, forbidden
        else:
            assert forbidden not in text, forbidden


def test_every_release_gate_keys_on_a_tag_run_including_a_rebuild():
    text, workflow = load()
    jobs = workflow["jobs"]
    for job in ("request-staging-smoke", "staging-gate"):
        assert jobs[job]["if"].strip() == TAG_RUN, job
    assert jobs["promote-check"]["if"].strip() == f"{TAG_RUN} && vars.CUA_SPACES_PROMOTE_CHECK == 'true'"
    assert TAG_RUN in jobs["installer-manifest"]["if"]
    assert TAG_RUN in jobs["site-follows-release"]["if"]
    assert "github.repository == 'trycua/cua'" in jobs["site-follows-release"]["if"]
    # No tag-only condition is left behind.
    bare = "startsWith(github.ref, 'refs/tags/cua-spaces-v')"
    assert text.count(bare) == text.count(TAG_RUN)
    # The release commit, not main's tip, is smoked and gated.
    assert 'request_staging_smoke.sh cua-spaces "$RELEASE_SHA"' in text
    assert 'require_staging_smoke.sh cua-spaces "$RELEASE_SHA"' in text
    # Signing, tag-only publishing and the draft step key on the same switch.
    assert f"TAGGED: ${{{{ {TAG_RUN} }}}}" in text
    assert f"if: {TAG_RUN}\n" in text
    # electron-plan plans a rebuild like a tag push (platform patch diff).
    assert "'workflow_dispatch' && inputs.rebuild_tag == '' && 'workflow_dispatch' || 'push'" in text
    assert text.count('[[ "$RELEASE_REF" == refs/tags/cua-spaces-v* ]]') == 2


def test_source_jobs_build_the_tagged_commit_and_tooling_jobs_keep_main():
    _, workflow = load()
    for name, job in workflow["jobs"].items():
        checkouts = [s for s in job.get("steps", []) if str(s.get("uses", "")).startswith("actions/checkout@")]
        refs = [(c.get("with") or {}).get("ref") for c in checkouts]
        if name in SOURCE_JOBS:
            assert checkouts and all(r == CHECKOUT_REF for r in refs), name
        elif name in MAIN_TOOLING_JOBS:
            assert all(r is None for r in refs), name


def test_version_comes_from_the_rebuilt_tag_not_the_dispatch_input():
    text, _ = load()
    guard = 'if [ "${{ github.event_name }}" = "workflow_dispatch" ] && [ -z "${REBUILD_TAG:-}" ]; then'
    assert text.count(guard) == 3
    assert "${RELEASE_REF_NAME#cua-spaces-v}" in text
    assert text.count("${{ github.event.inputs.version }}") == 3
    assert "REBUILD_TAG" in text.split("refuse-beta:", 1)[1].split("spacesd-published:", 1)[0]


def validate_step(workflow):
    steps = workflow["jobs"]["refuse-beta"]["steps"]
    step = next(s for s in steps if s["name"] == "Validate rebuild request")
    # After the beta refusal, which stays the first step of the first job.
    assert steps.index(step) == 1
    assert step["if"] == "${{ inputs.rebuild_tag != '' }}"
    return step


def test_rebuild_guard_refuses_everything_unsafe():
    _, workflow = load()
    step = validate_step(workflow)
    run = step["run"]
    # Only main's workflow files, only a stable tag, only the tagged commit.
    assert '"$GITHUB_EVENT_NAME" == "workflow_dispatch" && "$WORKFLOW_REF" == "refs/heads/main"' in run
    assert r"^cua-spaces-v[0-9]+\.[0-9]+\.[0-9]+$" in run
    assert "^[0-9a-f]{40}$" in run
    assert 'commits/${REBUILD_TAG}" --jq .sha' in run
    assert '"$TAG_SHA" == "$REBUILD_SHA"' in run
    # Never a duplicate: a published release of the tag is refused.
    assert '.tag_name == \\"${REBUILD_TAG}\\" and .draft == false' in run
    assert "already published" in run
    # No manual-run switch changes a gate for a rebuild.
    for switch in ("DRY_RUN", "SPARKLE_INPUT", "SKIP_STAGING_GATE", "PLATFORMS"):
        assert switch in step["env"]
    assert '"$PLATFORMS" == "all"' in run
    # Nothing moves or creates a tag.
    assert "git " not in run and "gh release create" not in run
    # The guard can see draft releases, and the beta refusal still follows it.
    assert workflow["jobs"]["refuse-beta"]["permissions"] == {"contents": "write"}
    names = [s["name"] for s in workflow["jobs"]["refuse-beta"]["steps"]]
    assert names == ["Refuse beta versions", "Validate rebuild request"]


def test_publish_still_requires_every_gate():
    _, workflow = load()
    job = workflow["jobs"]["installer-manifest"]
    needs = job["needs"]
    for gate in ("verify-release", "staging-gate", "electron-publish", "build-macos"):
        assert gate in needs
    for result in ("verify-release", "staging-gate"):
        assert f"needs.{result}.result == 'success'" in job["if"]
    assert job["permissions"]["id-token"] == "write"


def test_installers_accept_the_main_identity_of_a_rebuild_only_for_the_app():
    sh = (ROOT / "scripts/install/install.sh").read_text()
    ps1 = (ROOT / "scripts/install/install.ps1").read_text()
    assert "cd-cua-spaces.yml@refs/heads/main" in sh
    assert "cd-cua-spaces.yml@refs/heads/main" in ps1
    assert "cd-cua-sdk.yml@refs/heads" not in sh + ps1
    assert "identity-regexp" not in sh + ps1
