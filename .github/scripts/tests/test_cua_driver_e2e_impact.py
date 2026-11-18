"""Fail-closed tests for advisory Cua Driver desktop change impact."""

import importlib.util
import subprocess
from pathlib import Path

import pytest


MODULE_PATH = Path(__file__).resolve().parents[1] / "cua_driver_e2e_impact.py"
SPEC = importlib.util.spec_from_file_location("cua_driver_e2e_impact", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
impact = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(impact)


def affected(*paths: str) -> list[str]:
    return impact.classify(list(paths))["affected_platforms"]


def test_explicit_prose_and_diagnostics_do_not_request_behavioral_reruns() -> None:
    assert affected("docs/guide.md", "libs/cua-driver/docs/test-harnesses-guide.md") == []
    assert affected("scripts/ci/README.md", "scripts/ci/linux/preflight-rust-e2e.sh") == []
    assert impact.classify(["README.md"])["advisory_only"] is True


@pytest.mark.parametrize(
    ("path", "expected"),
    [
        ("libs/cua-driver/rust/crates/platform-macos/src/input/mouse.rs", ["macos"]),
        ("scripts/ci/windows/run-rust-e2e.ps1", ["windows"]),
        (".github/workflows/e2e-rust-linux.yml", ["linux"]),
        ("libs/cua-driver/rust/crates/cua-driver/src/lib.rs", list(impact.PLATFORMS)),
        ("libs/cua-driver/tests/fixtures/shared/web/journal.cjs", list(impact.PLATFORMS)),
        (".github/workflows/cd-cua-perception-candidate.yml", list(impact.PLATFORMS)),
        ("AGENTS.md", list(impact.PLATFORMS)),
        ("scripts/ci/linux/README.md", ["linux"]),
        ("docs/run.py", list(impact.PLATFORMS)),
        ("unknown.md", list(impact.PLATFORMS)),
        ("../docs/guide.md", list(impact.PLATFORMS)),
    ],
)
def test_platform_and_ambiguous_changes(path: str, expected: list[str]) -> None:
    assert affected(path) == expected


def test_combined_paths_take_union_and_empty_is_conservative() -> None:
    assert affected("docs/guide.md", "scripts/ci/linux/run-rust-e2e.sh", "scripts/ci/macos/run-rust-e2e.sh") == [
        "linux", "macos"
    ]
    assert affected() == list(impact.PLATFORMS)


def test_platform_workflow_and_diagnostic_edits_do_not_overstate_or_waive() -> None:
    result = impact.classify(
        [".github/workflows/e2e-rust-linux.yml", ".github/workflows/ci-cua-driver-quick.yml"]
    )
    assert result["affected_platforms"] == ["linux"]
    assert result["advisory_only"] is True
    assert "does not skip or certify tests" in result["note"]


def test_two_exact_commits_include_both_sides_of_a_rename(tmp_path: Path) -> None:
    repo = tmp_path / "repo"
    repo.mkdir()

    def git(*args: str) -> str:
        return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()

    git("init", "-q")
    git("config", "user.email", "tester@example.invalid")
    git("config", "user.name", "Test")
    source = repo / "libs/cua-driver/rust/crates/cua-driver/src"
    source.mkdir(parents=True)
    (source / "lib.rs").write_text("example\n")
    git("add", ".")
    git("commit", "-qm", "initial")
    tested = git("rev-parse", "HEAD")
    docs = repo / "docs"
    docs.mkdir()
    (source / "lib.rs").rename(docs / "guide.md")
    git("add", "-A")
    git("commit", "-qm", "move")
    candidate = git("rev-parse", "HEAD")

    assert impact.changed_paths(repo, tested, candidate) == [
        "docs/guide.md", "libs/cua-driver/rust/crates/cua-driver/src/lib.rs"
    ]
    assert affected(*impact.changed_paths(repo, tested, candidate)) == list(impact.PLATFORMS)
    with pytest.raises(ValueError, match="full 40-character"):
        impact.changed_paths(repo, "HEAD", candidate)
