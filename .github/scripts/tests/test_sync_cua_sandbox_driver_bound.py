from pathlib import Path
import subprocess

import pytest

import sync_cua_sandbox_driver_bound as sync
from sync_cua_sandbox_driver_bound import FILES, PYPROJECT, main

REPO = Path(__file__).resolve().parents[3]


def make_repo(root: Path, cap: int = 55) -> None:
    for rel in FILES:
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            f'dependencies = ["cua-driver>=0.37.0,<0.{cap}"]\n'
            f"The extra needs `>=0.37.0,<0.{cap}`.\n"
        )


def test_raises_the_bound_everywhere(tmp_path: Path, capsys) -> None:
    make_repo(tmp_path)
    assert main(["--repo-root", str(tmp_path), "--driver-version", "0.55.2"]) == 0
    assert capsys.readouterr().out.split() == list(FILES)
    for rel in FILES:
        text = (tmp_path / rel).read_text()
        # Built from parts so this file never holds a literal copy of the live bound.
        assert f">=0.37.0,<0.{55 + 1}" in text and f"<0.{55}" not in text


def test_in_range_version_changes_nothing(tmp_path: Path, capsys) -> None:
    make_repo(tmp_path)
    assert main(["--repo-root", str(tmp_path), "--driver-version", "0.54.9"]) == 0
    assert capsys.readouterr().out.strip() == ""
    assert "<0.55" in (tmp_path / PYPROJECT).read_text()


def test_bound_never_moves_down(tmp_path: Path) -> None:
    make_repo(tmp_path, cap=60)
    assert sync.sync(tmp_path, "0.50.0") == []


@pytest.mark.parametrize("version", ["1.0.0", "0.55", "0.55.0-rc1", "0.55.0; rm -rf /", "", "0.36.0"])
def test_refuses_versions_it_should_not_trust(tmp_path: Path, version: str) -> None:
    make_repo(tmp_path)
    assert main(["--repo-root", str(tmp_path), "--driver-version", version]) == 2
    assert "<0.55" in (tmp_path / PYPROJECT).read_text()


def test_check_mode_reports_without_editing(tmp_path: Path) -> None:
    make_repo(tmp_path)
    assert main(["--repo-root", str(tmp_path), "--driver-version", "0.54.0", "--check"]) == 0
    assert main(["--repo-root", str(tmp_path), "--driver-version", "0.55.0", "--check"]) == 1
    assert "<0.55" in (tmp_path / PYPROJECT).read_text()


def test_a_file_missing_the_bound_fails_before_partial_state_is_trusted(tmp_path: Path) -> None:
    make_repo(tmp_path)
    (tmp_path / FILES[-1]).write_text("nothing here\n")
    assert main(["--repo-root", str(tmp_path), "--driver-version", "0.55.0"]) == 2


def test_file_list_matches_the_real_repository() -> None:
    """Every tracked file that records the bound is rewritten, and no listed file lags."""
    low, cap = sync.current_spec(REPO)
    spec = f">={low},<0.{cap}"
    tracked = subprocess.run(
        ["git", "grep", "-l", "-F", spec], cwd=REPO, capture_output=True, text=True
    ).stdout.split()
    # This test and the generator describe the bound; they are not files it rewrites.
    ignored = {
        "libs/python/cua-sandbox/tests/test_driver_dependency.py",
        Path(__file__).resolve().relative_to(REPO).as_posix(),
        Path(sync.__file__).resolve().relative_to(REPO).as_posix(),
    }
    unlisted = set(tracked) - set(FILES) - ignored
    assert not unlisted, f"add to FILES in sync_cua_sandbox_driver_bound.py: {sorted(unlisted)}"
    for rel in FILES:
        assert spec in (REPO / rel).read_text(), f"{rel} lags the pyproject bound"


def test_release_workflow_runs_the_script_from_main_in_its_own_job() -> None:
    workflow = (REPO / ".github/workflows/release-please.yml").read_text()
    job = workflow.split("\n  driver-bound:\n", 1)[1]
    assert "needs: release-please" in job
    assert "persist-credentials: false" in job and "ref: main" in job
    assert "GH_TOKEN: ${{ steps.app-token.outputs.token }}" in job
    assert "github.token" not in job and "GITHUB_TOKEN" not in job
    # No expression is interpolated into shell; values arrive through env.
    assert "${{ " not in job.split("run: |", 1)[1]
    assert job.index("--detach origin/main") < job.index("sync_cua_sandbox_driver_bound.py")
    assert "--auto --squash --match-head-commit" in job
    assert 'gh pr merge "$PR" --auto --match-head-commit' in job
    # The release job must not run it (it executes release-branch code).
    assert "sync_cua_sandbox_driver_bound" not in workflow.split("\n  driver-bound:\n", 1)[0]


def test_driver_release_branch_refreshes_when_the_bound_changes() -> None:
    import release_branch_needs_rebase as rebase

    assert "libs/python/cua-sandbox/pyproject.toml" in rebase.EXTRA_PATHS["cua-driver-rs"]
