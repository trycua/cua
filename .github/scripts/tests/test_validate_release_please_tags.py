from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess

import pytest

SCRIPT_PATH = Path(__file__).resolve().parents[1] / "validate_release_please_tags.py"
SPEC = importlib.util.spec_from_file_location("validate_release_please_tags", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
TagValidationError = MODULE.TagValidationError
package_tag = MODULE.package_tag
validate_tags = MODULE.validate_tags


CONFIG = {
    "include-component-in-tag": True,
    "include-v-in-tag": True,
    "tag-separator": "-",
    "packages": {
        "libs/cua-driver": {
            "component": "cua-driver-rs",
            "release-type": "simple",
        }
    },
}
MANIFEST = {"libs/cua-driver": "0.18.0"}


def run_git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()


@pytest.fixture
def repo(tmp_path: Path) -> Path:
    run_git(tmp_path, "init", "-b", "main")
    run_git(tmp_path, "config", "user.name", "Release Test")
    run_git(tmp_path, "config", "user.email", "release-test@example.com")
    (tmp_path / "file").write_text("first\n")
    run_git(tmp_path, "add", "file")
    run_git(tmp_path, "commit", "-m", "first")
    return tmp_path


def test_package_tag_uses_manifest_tag_options() -> None:
    assert package_tag(CONFIG, CONFIG["packages"]["libs/cua-driver"], "0.18.0") == (
        "cua-driver-rs-v0.18.0"
    )


def test_allows_missing_tag_for_newly_merged_release(repo: Path) -> None:
    assert validate_tags(repo_root=repo, config=CONFIG, manifest=MANIFEST, target="HEAD") == []


def test_accepts_manifest_tag_on_target_branch(repo: Path) -> None:
    run_git(repo, "tag", "cua-driver-rs-v0.18.0")
    (repo / "file").write_text("second\n")
    run_git(repo, "commit", "-am", "second")

    assert validate_tags(repo_root=repo, config=CONFIG, manifest=MANIFEST, target="HEAD") == [
        "cua-driver-rs-v0.18.0"
    ]


def test_rejects_manifest_tag_outside_target_branch(repo: Path) -> None:
    base = run_git(repo, "rev-parse", "HEAD")
    run_git(repo, "switch", "-c", "release-metadata")
    (repo / "metadata").write_text("notes\n")
    run_git(repo, "add", "metadata")
    run_git(repo, "commit", "-m", "metadata")
    run_git(repo, "tag", "cua-driver-rs-v0.18.0")
    run_git(repo, "switch", "main")
    (repo / "file").write_text("main\n")
    run_git(repo, "commit", "-am", "main")
    assert run_git(repo, "merge-base", "HEAD", "release-metadata") == base

    with pytest.raises(TagValidationError, match="is not an ancestor"):
        validate_tags(repo_root=repo, config=CONFIG, manifest=MANIFEST, target="HEAD")
