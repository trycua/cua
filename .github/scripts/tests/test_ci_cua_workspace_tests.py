"""Every libs/cua crate that has tests runs them in GitHub CI, or says why not.

The Linux Rust lane (ci-cua-sdk.yml, `rust-linux-build`) lists its crates by hand
with `-p`. A crate missing from the list is never tested there, and a path
filter cannot see that: cua-auth's tests were skipped this way, so three
broken flows tests merged green (CUA-3363). This test makes the list and the
workspace agree.
"""

import functools
import re
import sys
from pathlib import Path

import pytest
import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import ci_gate_scope as scope  # noqa: E402
import ci_graph  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]
CUA = ROOT / "libs/cua"
WORKFLOW = ROOT / ".github/workflows/ci-cua-sdk.yml"

# Crates with tests that the Linux lane does not run, and where they run (or
# why they do not). Adding a crate here is a decision; leaving it out of both
# places fails this test.
ELSEWHERE = {
    "cua-keyvault": "macOS Keychain and SwiftUI identity tests: ci-cua-spaces-macos.yml",
    "cua-image": "needs registry and platform helpers; run by bin/ci and the image workflows",
    "cua-host": "launchd service tests; run by bin/ci on macOS, not yet checked on Linux",
    "cua-ci": "experiment (`cua ci`); no CI lane yet",
    "cua-replica-lab": "chaos rig, not shipped; run by hand",
    "cua-chaos": "chaos tooling, not shipped; its only lane (chaos-cua-compat.yml) was removed with the dead compat lanes (CUA-3571); run by hand with `cargo test -p cua-chaos`",
}


def members():
    text = (CUA / "Cargo.toml").read_text()
    block = re.search(r"members\s*=\s*\[(.*?)\]", text, re.S).group(1)
    block = re.sub(r"#[^\n]*", "", block)
    return re.findall(r'"([^"]+)"', block)


def package_name(crate_dir):
    text = (CUA / crate_dir / "Cargo.toml").read_text()
    return re.search(r'\[package\].*?\bname\s*=\s*"([^"]+)"', text, re.S).group(1)


def has_tests(crate_dir):
    d = CUA / crate_dir
    if list(d.glob("tests/*.rs")):
        return True
    return any(re.search(r"#\[(tokio::)?test\b", p.read_text(errors="ignore")) for p in d.glob("src/**/*.rs"))


@functools.lru_cache(maxsize=None)
def tested_packages():
    """Packages named with `-p` on a `cargo test` / `cargo nextest` command in any workflow."""
    found = set()
    for wf in sorted((ROOT / ".github/workflows").glob("*.yml")):
        doc = yaml.safe_load(wf.read_text()) or {}
        for job in (doc.get("jobs") or {}).values():
            for step in job.get("steps") or []:
                run = (step.get("run") or "").replace("\\\n", " ")
                for cmd in re.split(r"\n|&&|;", run):
                    if re.search(r"cargo (nextest (run|archive)|test)\b", cmd):
                        found |= set(re.findall(r"(?:^|\s)(?:-p|--package)[ =](\S+)", cmd))
    return frozenset(found)


def nextest_packages():
    jobs = yaml.safe_load(WORKFLOW.read_text())["jobs"]
    # The test binaries are built once, by `cargo nextest archive`, and the shards run the archive.
    for step in jobs["rust-linux-build"]["steps"]:
        run = step.get("run", "").replace("\\\n", " ")
        if "cargo nextest archive" in run:
            return set(re.findall(r"(?:^|\s)-p\s+(\S+)", run))
    raise AssertionError("rust-linux-build has no cargo nextest archive step")


TESTED = sorted(package_name(m) for m in members() if has_tests(m))


@pytest.mark.parametrize("name", TESTED)
def test_a_crate_with_tests_runs_them_in_the_linux_lane_or_says_why_not(name):
    assert name in tested_packages() or name in ELSEWHERE, (
        f"{name} has tests but is not named with `-p` on any `cargo test` or `cargo nextest` step in "
        f".github/workflows: its tests never run on GitHub. Add it to the rust-linux-build nextest archive list in ci-cua-sdk.yml, or to ELSEWHERE with the reason."
    )


def test_the_exceptions_are_not_stale():
    listed = tested_packages()
    assert not (set(ELSEWHERE) & listed), "in the lane now: drop it from ELSEWHERE"
    assert set(ELSEWHERE) <= set(TESTED), "no longer a crate with tests: drop it from ELSEWHERE"


def test_the_lane_names_only_workspace_crates():
    names = {package_name(m) for m in members()}
    assert nextest_packages() <= names


def test_a_cua_auth_change_always_reaches_the_lane_that_tests_it():
    cfg = scope.load_map()
    graph = ci_graph.load()
    for f in ("libs/cua/crates/cua-auth/tests/flows.rs", "libs/cua/crates/cua-auth/src/lib.rs"):
        areas, skip = scope.scope([f], cfg, graph)
        assert "cua" in areas
        assert "ci-cua-sdk.yml" not in skip
    on = yaml.safe_load(WORKFLOW.read_text())[True]
    assert "libs/cua/**" in on["pull_request"]["paths"]
