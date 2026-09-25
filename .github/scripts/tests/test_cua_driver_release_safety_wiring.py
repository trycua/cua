"""Release-safety wiring for Cua Driver (#4109).

0.28.3 was published with an unsigned macOS app and then baked into the
canonical installers, breaking every fresh macOS install. These tests pin the
gates that make that sequence impossible:

1. a run that can publish must notarize, and says so before building;
2. the release job cannot start until the exact candidate archives pass the
   macOS and Windows signature verification jobs;
3. the installer bake runs only after the published assets pass signature
   verification and the canonical installers install them on every platform;
4. no path skips those gates with always() or a similar status override.
"""

from __future__ import annotations

from pathlib import Path
import re

import pytest
import yaml


REPO_ROOT = Path(__file__).resolve().parents[3]
CD = REPO_ROOT / ".github/workflows/cd-rust-cua-driver.yml"
SIGNATURES = REPO_ROOT / ".github/workflows/cua-driver-release-signatures.yml"
SELF_TEST = REPO_ROOT / ".github/workflows/ci-cua-driver-release-signatures.yml"
DO_NOTARIZE = "${{ startsWith(github.ref, 'refs/tags/cua-driver-rs-v') || inputs.notarize == true }}"
CAN_PUBLISH = (
    "${{ (github.event_name == 'push' && startsWith(github.ref, 'refs/tags/cua-driver-rs-v')) "
    "|| (github.event_name == 'workflow_dispatch' && inputs.publish == true) }}"
)
STATUS_OVERRIDES = ("always()", "failure()", "cancelled()", "!cancelled()")


def load(path: Path) -> dict:
    return yaml.safe_load(path.read_text(encoding="utf-8"))


@pytest.fixture(scope="module")
def cd() -> dict:
    return load(CD)


def needs(job: dict) -> set[str]:
    value = job.get("needs", [])
    return {value} if isinstance(value, str) else set(value)


def step(job: dict, name: str) -> dict:
    matches = [entry for entry in job["steps"] if entry.get("name") == name]
    assert len(matches) == 1, f"expected one step named {name!r}"
    return matches[0]


def publishing_jobs(workflow: dict) -> list[str]:
    """Jobs that upload a release or move the public installers."""
    names = []
    for name, job in workflow["jobs"].items():
        text = yaml.safe_dump(job)
        if "github_release.py" in text or "update_cua_driver_installer_version.py" in text:
            names.append(name)
    return names


def ancestors(workflow: dict, name: str) -> set[str]:
    seen: set[str] = set()
    pending = list(needs(workflow["jobs"][name]))
    while pending:
        current = pending.pop()
        if current not in seen:
            seen.add(current)
            pending.extend(needs(workflow["jobs"][current]))
    return seen


def test_macos_build_notarizes_every_tag_build(cd: dict) -> None:
    build = cd["jobs"]["build-macos-universal"]
    assert build["env"]["DO_NOTARIZE"] == DO_NOTARIZE
    first = build["steps"][0]
    assert first["name"] == "Assert notarization for publishable builds"
    assert first["env"]["CAN_PUBLISH"] == CAN_PUBLISH
    assert '"$GITHUB_REF" == refs/tags/cua-driver-rs-v* && "$DO_NOTARIZE" != "true"' in first["run"]
    assert '"$CAN_PUBLISH" == "true" && "$DO_NOTARIZE" != "true"' in first["run"]
    # Signing and notarization steps are all keyed on the same switch.
    for name in (
        "Import code-signing certificate",
        "Codesign bare universal binary (hardened runtime)",
        "Codesign + notarize + staple CuaDriver.app",
    ):
        assert step(build, name)["if"] == "env.DO_NOTARIZE == 'true'"


def test_publishing_without_notarization_fails_before_any_build(cd: dict) -> None:
    preflight = cd["jobs"]["release-attribution-preflight"]
    guard = preflight["steps"][0]
    assert guard["name"] == "Refuse to publish without macOS notarization"
    assert guard["env"] == {"CAN_PUBLISH": CAN_PUBLISH, "DO_NOTARIZE": DO_NOTARIZE}
    assert "exit 1" in guard["run"]
    for name in ("build-linux", "build-windows", "build-macos-universal"):
        assert "release-attribution-preflight" in needs(cd["jobs"][name])


def test_every_publishing_path_is_a_notarized_path(cd: dict) -> None:
    """The release job's trigger must imply CAN_PUBLISH, which implies DO_NOTARIZE via the guard."""
    release = cd["jobs"]["release"]
    condition = release["if"]
    assert condition in (
        "github.event_name == 'workflow_dispatch' && inputs.publish == true",
        "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/cua-driver-rs-v')",
    )
    guard = release["steps"][0]
    assert guard["name"] == "Refuse to publish unverified or unnotarized artifacts"
    assert guard["env"]["DO_NOTARIZE"] == DO_NOTARIZE
    assert guard["env"]["MACOS_SIGNATURES"] == "${{ needs.verify-macos-release-signatures.result }}"
    assert guard["env"]["WINDOWS_SIGNATURES"] == "${{ needs.verify-windows-release-signatures.result }}"
    assert '"$DO_NOTARIZE" != "true"' in guard["run"]


def test_signature_jobs_verify_exact_candidate_archives(cd: dict) -> None:
    macos = cd["jobs"]["verify-macos-release-signatures"]
    windows = cd["jobs"]["verify-windows-release-signatures"]
    for job in (macos, windows):
        assert job["uses"] == "./.github/workflows/cua-driver-release-signatures.yml"
        assert job["with"]["source"] == "artifacts"
        assert job["with"]["version"] == "${{ needs.release-attribution-preflight.outputs.version }}"
        assert "expect_macos" not in job["with"] and "expect_windows" not in job["with"]
    assert needs(macos) == {"release-attribution-preflight", "build-macos-universal"}
    assert needs(windows) == {"release-attribution-preflight", "build-windows"}
    assert macos["with"]["verify_macos"] is True and macos["with"]["verify_windows"] is False
    assert windows["with"]["verify_macos"] is False and windows["with"]["verify_windows"] is True
    # The macOS gate runs for exactly the builds that notarize.
    assert "${{ " + macos["if"] + " }}" == DO_NOTARIZE
    assert "if" not in windows


def test_release_job_needs_signature_verification(cd: dict) -> None:
    release = cd["jobs"]["release"]
    assert {
        "verify-release-artifacts",
        "verify-macos-release-signatures",
        "verify-windows-release-signatures",
    } <= needs(release)
    assert not any(override in release["if"] for override in STATUS_OVERRIDES)
    publish = step(release, "Publish the verified Release Please draft")
    assert release["steps"].index(step(release, "Refuse to publish unverified or unnotarized artifacts")) < release["steps"].index(publish)
    assert release["outputs"]["version"] == "${{ steps.version.outputs.version }}"


def test_installer_bake_needs_post_publication_verification(cd: dict) -> None:
    jobs = cd["jobs"]
    signatures = jobs["verify-published-signatures"]
    installers = jobs["verify-published-installers"]
    bake = jobs["advance-installer-version"]

    assert signatures["uses"] == "./.github/workflows/cua-driver-release-signatures.yml"
    assert signatures["with"] == {"version": "${{ needs.release.outputs.version }}", "source": "release"}
    assert installers["uses"] == "./.github/workflows/ci-cua-driver-installer-compat.yml"
    assert installers["with"] == {"versions": "${{ needs.release.outputs.version }}"}
    for job in (signatures, installers):
        assert needs(job) == {"release"}
        assert "if" not in job

    assert needs(bake) == {"release", "verify-published-signatures", "verify-published-installers"}
    assert "if" not in bake
    advance = step(bake, "Advance public installer version on main")
    assert advance["env"]["VERSION"] == "${{ needs.release.outputs.version }}"
    assert "--withdrawn-path \"$UPDATE_ROOT/.github/release-state/cua-driver-rs-withdrawn-versions\"" in advance["run"]
    assert "validate_release_versions.py" in advance["run"]


def test_only_the_gated_bake_job_moves_the_public_installers(cd: dict) -> None:
    assert sorted(publishing_jobs(cd)) == ["advance-installer-version", "release"]
    release_text = yaml.safe_dump(cd["jobs"]["release"])
    assert "--state-path" not in release_text
    assert "refs/heads/main" not in release_text
    staged = step(cd["jobs"]["release"], "Stage release files")["run"]
    assert "--withdrawn-path release-control/.github/release-state/cua-driver-rs-withdrawn-versions" in staged
    assert {
        "verify-macos-release-signatures",
        "verify-windows-release-signatures",
        "verify-published-signatures",
        "verify-published-installers",
    } <= ancestors(cd, "advance-installer-version")


def test_no_gate_is_bypassed_by_a_status_override(cd: dict) -> None:
    for name in (
        "release",
        "verify-macos-release-signatures",
        "verify-windows-release-signatures",
        "verify-published-signatures",
        "verify-published-installers",
        "advance-installer-version",
    ):
        condition = str(cd["jobs"][name].get("if", ""))
        assert not any(override in condition for override in STATUS_OVERRIDES), name


def test_signature_workflow_checks_what_the_operating_systems_check() -> None:
    workflow = load(SIGNATURES)
    trigger = workflow[True]["workflow_call"]["inputs"]  # PyYAML parses `on` as True
    assert set(trigger) == {"version", "source", "verify_macos", "verify_windows", "expect_macos", "expect_windows"}
    assert trigger["expect_macos"]["default"] == "pass"
    assert trigger["expect_windows"]["default"] == "pass"
    assert workflow["permissions"] == {"contents": "read"}

    macos = workflow["jobs"]["macos"]
    windows = workflow["jobs"]["windows"]
    assert macos["runs-on"].startswith("macos-")
    assert windows["runs-on"].startswith("windows-")
    macos_verify = step(macos, "Verify macOS signatures, notarization, and stapling")["run"]
    assert "verify_cua_driver_release_signatures.py macos" in macos_verify
    assert "--team-id YCK386LBJ7" in macos_verify
    windows_verify = step(windows, "Verify Windows Authenticode signatures")["run"]
    assert "verify_cua_driver_release_signatures.py windows" in windows_verify
    assert '--signer "Cua AI, Inc."' in windows_verify
    for run in (macos_verify, windows_verify):
        # GitHub runs `bash -e`; the status must be captured, not aborted on.
        assert '${ANNOTATE[@]+"${ANNOTATE[@]}"} || STATUS=$?' in run
        assert run.index("STATUS=0") < run.index("|| STATUS=$?")
        assert "expect_verification_result.sh \"$EXPECT\" \"$STATUS\"" in run
    assert step(macos, "Download candidate darwin archives")["with"]["name"] == "cua-driver-rs-darwin"
    assert step(windows, "Download candidate Windows archives")["with"]["pattern"] == "cua-driver-rs-windows-*"


def test_signature_team_matches_the_pinned_signing_team() -> None:
    cd_text = CD.read_text(encoding="utf-8")
    pinned = re.search(r"test \"\$TEAM_ID\" = '([A-Z0-9]{10})'", cd_text).group(1)
    assert pinned == "YCK386LBJ7"
    assert f"--team-id {pinned}" in SIGNATURES.read_text(encoding="utf-8")
    script = (REPO_ROOT / ".github/scripts/verify_cua_driver_release_signatures.py").read_text()
    assert f'RELEASE_TEAM_ID = "{pinned}"' in script


def test_self_test_proves_the_gate_in_both_directions() -> None:
    workflow = load(SELF_TEST)
    jobs = workflow["jobs"]
    assert jobs["signed-release"]["with"] == {"version": "0.28.2", "source": "release"}
    assert jobs["unsigned-release"]["with"] == {
        "version": "0.28.3",
        "source": "release",
        "expect_macos": "fail",
        "expect_windows": "pass",
    }
    withdrawn = (REPO_ROOT / ".github/release-state/cua-driver-rs-withdrawn-versions").read_text()
    assert re.search(r"^0\.28\.3 #", withdrawn, re.MULTILINE)
