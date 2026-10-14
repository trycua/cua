"""Behavioral tests for mark_perception_release_prs_tagged.py with a fake gh."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / ".github/scripts/mark_perception_release_prs_tagged.py"
WORKFLOW = ROOT / ".github/workflows/release-please.yml"
VERSION_PATH = "libs/cua-driver/rust/crates/cua-perception/VERSION"
BRANCH = "release-please--branches--main--components--cua-perception"

# The fake gh serves pull requests and tags from a JSON state file and logs
# every call. Like the real gh, a missing ref prints the 404 body on stdout.
FAKE_GH = textwrap.dedent(
    """\
    #!{python}
    import json, os, pathlib, sys

    state = json.loads(pathlib.Path(os.environ["FAKE_GH_STATE"]).read_text())
    args = sys.argv[1:]
    with open(os.environ["FAKE_GH_LOG"], "a") as log:
        log.write(json.dumps(args) + "\\n")
    if args[:2] == ["pr", "list"]:
        print(json.dumps(state["pulls"]))
        raise SystemExit(0)
    if args[:2] == ["pr", "edit"]:
        raise SystemExit(0)
    if args[0] == "api":
        if state.get("mode") == "server-error":
            print(json.dumps({{"message": "Server Error", "status": "500"}}))
            raise SystemExit(1)
        name = args[1].split("/git/ref/tags/", 1)[1]
        if name not in state["tags"]:
            print(json.dumps({{"message": "Not Found", "status": "404"}}))
            raise SystemExit(1)
        print(json.dumps({{"ref": "refs/tags/" + name, "object": state["tags"][name]}}))
        raise SystemExit(0)
    raise SystemExit(2)
    """
)


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()


@pytest.fixture
def env(tmp_path: Path) -> dict[str, object]:
    repo = tmp_path / "repo"
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)
    git(repo, "config", "user.email", "test@example.com")
    git(repo, "config", "user.name", "Test")
    version = repo / VERSION_PATH
    version.parent.mkdir(parents=True)
    version.write_text("0.2.1\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "chore(main): release cua-perception 0.2.1")
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    fake = bin_dir / "gh"
    fake.write_text(FAKE_GH.format(python=sys.executable))
    fake.chmod(0o755)
    return {
        "repo": repo,
        "sha": git(repo, "rev-parse", "HEAD"),
        "bin": bin_dir,
        "state": tmp_path / "state.json",
        "log": tmp_path / "gh.log",
    }


def run(env: dict[str, object], pulls: list[dict], tags: dict, **extra) -> tuple[subprocess.CompletedProcess[str], list[list[str]]]:
    env["state"].write_text(json.dumps({"pulls": pulls, "tags": tags, **extra}))
    environment = dict(os.environ)
    environment.update({
        "PATH": f"{env['bin']}{os.pathsep}{environment['PATH']}",
        "GITHUB_REPOSITORY": "trycua/cua",
        "FAKE_GH_STATE": str(env["state"]),
        "FAKE_GH_LOG": str(env["log"]),
    })
    result = subprocess.run(
        [sys.executable, str(SCRIPT)], cwd=env["repo"], env=environment,
        capture_output=True, text=True,
    )
    log = env["log"]
    calls = [json.loads(line) for line in log.read_text().splitlines()] if log.exists() else []
    return result, calls


def edits(calls: list[list[str]]) -> list[list[str]]:
    return [call for call in calls if call[:2] == ["pr", "edit"]]


def pull(env, number: int = 4075, branch: str = BRANCH) -> dict:
    return {"number": number, "headRefName": branch, "mergeCommit": {"oid": env["sha"]}}


def test_pending_pull_request_with_anchored_tag_is_marked_tagged(env) -> None:
    tags = {"cua-perception-v0.2.1": {"type": "commit", "sha": env["sha"]}}
    result, calls = run(env, [pull(env)], tags)
    assert result.returncode == 0, result.stdout + result.stderr
    assert edits(calls) == [[
        "pr", "edit", "4075", "--repo", "trycua/cua",
        "--remove-label", "autorelease: pending",
        "--add-label", "autorelease: tagged",
    ]]


def test_missing_tag_leaves_the_pull_request_pending(env) -> None:
    result, calls = run(env, [pull(env)], {})
    assert result.returncode == 0, result.stdout + result.stderr
    assert edits(calls) == []
    assert "leaving it pending" in result.stdout


@pytest.mark.parametrize("record", [
    {"type": "commit", "sha": "0" * 40},
    {"type": "tag", "sha": "SHA"},
])
def test_tag_elsewhere_or_annotated_leaves_the_pull_request_pending(env, record) -> None:
    record = {**record, "sha": env["sha"] if record["sha"] == "SHA" else record["sha"]}
    result, calls = run(env, [pull(env)], {"cua-perception-v0.2.1": record})
    assert result.returncode == 0, result.stdout + result.stderr
    assert edits(calls) == []


def test_other_branches_are_ignored(env) -> None:
    tags = {"cua-perception-v0.2.1": {"type": "commit", "sha": env["sha"]}}
    other = pull(env, 1, "release-please--branches--main--components--cua-perception-extra")
    result, calls = run(env, [other], tags)
    assert result.returncode == 0, result.stdout + result.stderr
    assert edits(calls) == []


def test_tag_lookup_errors_fail_closed(env) -> None:
    result, calls = run(env, [pull(env)], {}, mode="server-error")
    assert result.returncode != 0
    assert edits(calls) == []


def test_runs_after_the_anchor_and_before_release_please() -> None:
    steps = yaml.safe_load(WORKFLOW.read_text())["jobs"]["release-please"]["steps"]
    names = [step.get("name") for step in steps]
    mark = names.index("Mark tagged Perception release pull requests")
    assert names.index("Anchor a merged Perception version with a lightweight tag") < mark
    assert mark < names.index("Open or update automatic release pull requests")
    assert mark < names.index("Open or update the targeted release pull request")
    step = steps[mark]
    assert "if" not in step
    assert step["env"]["GH_TOKEN"] == "${{ steps.app-token.outputs.token }}"
    assert step["run"] == "python3 .github/scripts/mark_perception_release_prs_tagged.py"
