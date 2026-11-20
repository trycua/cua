"""The Cua Spaces nightly channel stays apart from stable (CUA-3503).

Run: python3 -m pytest .github/scripts/tests/test_spaces_nightly_wiring.py
"""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github/workflows"


def source(name: str) -> str:
    return (WORKFLOWS / name).read_text()


def test_nightly_publishes_only_to_its_own_rolling_prerelease():
    nightly = source("nightly-cua-spaces.yml")
    assert "  TAG: cua-spaces-nightly\n" in nightly
    # Every release write goes to $TAG; the stable release is only read.
    for line in re.findall(r"gh release (?:upload|create|edit|delete-asset)[^\n]*", nightly):
        assert '"$TAG"' in line, line
    assert "cua-spaces-latest" not in "".join(re.findall(r"gh release (?:upload|create|edit|delete-asset)[^\n]*", nightly))
    # Never Latest, always a prerelease.
    assert "--prerelease --latest=false" in nightly
    assert nightly.count("--latest=false") == 3
    assert "--latest=true" not in nightly and "make_latest" not in nightly


def test_nightly_checks_its_feeds_and_the_stable_ones():
    nightly = source("nightly-cua-spaces.yml")
    assert "sparkle_feed.py nightly" in nightly
    assert "sparkle_feed.py check \"$stable/cua-spaces-appcast.xml\"" in nightly
    assert "scripts/nightly-feeds.mjs" in nightly


def test_nightly_is_gated_on_main_ci_and_on_changes():
    nightly = source("nightly-cua-spaces.yml")
    assert "spaces_nightly_gate.py" in nightly
    assert "spaces_inputs_tree.sh HEAD" in nightly
    assert "if: github.ref == 'refs/heads/main'" in nightly


def test_nightly_builds_every_platform():
    build = source("cd-cua-spaces-build.yml")
    native = build.split("\n  electron-native:\n", 1)[1].split("\n  electron-windows-pkg:\n", 1)[0]
    assert "inputs.channel != 'nightly'" not in native
    assert "dist/nightly.yml" in build and "dist/nightly-linux*.yml" in build
    assert build.count("needs.plan.outputs.electron_version") >= 3


def test_release_workflow_refuses_betas_before_anything_else():
    import yaml

    release = yaml.safe_load(source("cd-cua-spaces.yml"))
    jobs = release["jobs"]
    guard = jobs["refuse-beta"]
    assert "needs" not in guard
    env = guard["steps"][0]["env"]
    assert env["ALLOW_SPACES_BETA"] == "${{ vars.ALLOW_SPACES_BETA }}"
    assert "*-beta*" in guard["steps"][0]["run"]

    def deps(job, seen):
        needs = jobs[job].get("needs", [])
        for d in [needs] if isinstance(needs, str) else needs:
            if d not in seen:
                seen.add(d)
                deps(d, seen)
        return seen

    # Every job waits for the guard: no build, staging smoke or draft before it.
    assert [j for j in jobs if j != "refuse-beta" and "refuse-beta" not in deps(j, set())] == []
    # A job that runs on !cancelled() still needs a real plan.
    assert "needs.electron-plan.result == 'success'" in jobs["verify-release"]["if"]
