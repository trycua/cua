"""Dry-run checks for the release branch rebase decision."""

from __future__ import annotations

import importlib.util
import subprocess
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "release_branch_needs_rebase.py"
spec = importlib.util.spec_from_file_location("release_branch_needs_rebase", SCRIPT)
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)

BRANCH = mod.COMPONENT_PREFIX + "lume"
CONFIG = {
    "packages": {
        "libs/lume": {"component": "lume"},
        "libs/cua-driver": {"component": "cua-driver-rs"},
    }
}


def run(repo: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True)


def commit(repo: Path, path: str, text: str) -> None:
    target = repo / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text)
    run(repo, "add", path)
    run(repo, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-m", f"edit {path}")


def make_repo(tmp_path: Path) -> Path:
    run(tmp_path, "init", "-q", "-b", "main")
    commit(tmp_path, "libs/lume/a.txt", "base\n")
    run(tmp_path, "checkout", "-q", "-b", "release")
    commit(tmp_path, "libs/lume/VERSION", "1.1.0\n")
    run(tmp_path, "checkout", "-q", "main")
    return tmp_path


def decide(repo: Path, monkeypatch) -> str:
    monkeypatch.chdir(repo)
    return mod.decide(CONFIG, BRANCH, "main", "release")[0]


def test_unrelated_main_push_keeps_branch(tmp_path, monkeypatch):
    repo = make_repo(tmp_path)
    commit(repo, "docs/readme.md", "unrelated\n")
    assert decide(repo, monkeypatch) == "keep"


def test_other_component_push_keeps_branch(tmp_path, monkeypatch):
    repo = make_repo(tmp_path)
    commit(repo, "libs/cua-driver/x.rs", "other\n")
    assert decide(repo, monkeypatch) == "keep"


def test_component_commit_triggers_rebase(tmp_path, monkeypatch):
    repo = make_repo(tmp_path)
    commit(repo, "libs/lume/b.txt", "new feature\n")
    assert decide(repo, monkeypatch) == "rebase"


def test_shared_release_tooling_triggers_rebase(tmp_path, monkeypatch):
    repo = make_repo(tmp_path)
    commit(repo, ".github/scripts/validate_release_versions.py", "x\n")
    assert decide(repo, monkeypatch) == "rebase"


def test_conflict_triggers_rebase(tmp_path, monkeypatch):
    repo = make_repo(tmp_path)
    commit(repo, "libs/lume/VERSION", "9.9.9\n")
    assert decide(repo, monkeypatch) == "rebase"


def test_unknown_branch_keeps_old_behavior():
    assert mod.decide(CONFIG, "feature/x", "main", "HEAD")[0] == "rebase"
