"""Behavioral tests for anchor_perception_release_tag.sh with a fake gh."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / ".github/scripts/anchor_perception_release_tag.sh"
VERSION_PATH = "libs/cua-driver/rust/crates/cua-perception/VERSION"
REPOSITORY = "trycua/cua"

# The fake gh keeps remote tags in a JSON file. Like the real gh, a GET for a
# missing ref prints the 404 error body on stdout and exits nonzero.
FAKE_GH = textwrap.dedent(
    """\
    #!{python}
    import json, os, pathlib, sys

    store = pathlib.Path(os.environ["FAKE_GH_TAGS"])
    tags = json.loads(store.read_text()) if store.exists() else {{}}
    args = sys.argv[1:]
    with open(os.environ["FAKE_GH_LOG"], "a") as log:
        log.write(json.dumps(args) + "\\n")
    if os.environ.get("FAKE_GH_MODE") == "server-error" and "--method" not in args:
        print(json.dumps({{"message": "Server Error", "status": "500"}}))
        print("gh: Server Error (HTTP 500)", file=sys.stderr)
        raise SystemExit(1)
    if args[0] != "api":
        raise SystemExit(2)
    if "--method" in args:
        fields = dict(value.split("=", 1) for flag, value in zip(args, args[1:]) if flag == "-f")
        name = fields["ref"].removeprefix("refs/tags/")
        if name in tags:
            print(json.dumps({{"message": "Reference already exists", "status": "422"}}))
            raise SystemExit(1)
        tags[name] = {{"type": "commit", "sha": fields["sha"]}}
        store.write_text(json.dumps(tags))
        print(json.dumps({{"ref": fields["ref"]}}))
        raise SystemExit(0)
    path = args[1]
    name = path.split("/git/ref/tags/", 1)[1]
    if name not in tags:
        print(json.dumps({{"message": "Not Found", "status": "404"}}))
        print("gh: Not Found (HTTP 404)", file=sys.stderr)
        raise SystemExit(1)
    print(json.dumps({{"ref": "refs/tags/" + name, "object": tags[name]}}))
    """
)


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True
    ).stdout.strip()


def commit_version(repo: Path, version: str | None, message: str) -> str:
    path = repo / VERSION_PATH
    path.parent.mkdir(parents=True, exist_ok=True)
    if version is None:
        (repo / "other.txt").write_text(message)
    else:
        path.write_text(version + "\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", message)
    return git(repo, "rev-parse", "HEAD")


@pytest.fixture
def env(tmp_path: Path) -> dict[str, object]:
    if shutil.which("jq") is None:
        pytest.skip("jq is required")
    origin = tmp_path / "origin.git"
    repo = tmp_path / "repo"
    subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(origin)], check=True)
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)
    git(repo, "config", "user.email", "test@example.com")
    git(repo, "config", "user.name", "Test")
    git(repo, "remote", "add", "origin", str(origin))
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    fake = bin_dir / "gh"
    fake.write_text(FAKE_GH.format(python=sys.executable))
    fake.chmod(0o755)
    return {"repo": repo, "bin": bin_dir, "tags": tmp_path / "tags.json", "log": tmp_path / "gh.log"}


def run_anchor(env: dict[str, object], before: str, sha: str, **extra: str) -> subprocess.CompletedProcess[str]:
    repo = env["repo"]
    git(repo, "push", "-q", "origin", f"{sha}:refs/heads/main", "--force")
    environment = dict(os.environ)
    environment.update({
        "PATH": f"{env['bin']}{os.pathsep}{environment['PATH']}",
        "GITHUB_REPOSITORY": REPOSITORY,
        "GITHUB_SHA": sha,
        "BEFORE_SHA": before,
        "VERSION_PATH": VERSION_PATH,
        "FAKE_GH_TAGS": str(env["tags"]),
        "FAKE_GH_LOG": str(env["log"]),
        "RELEASE_VERSION_VALIDATOR": "true",
        **extra,
    })
    return subprocess.run(
        ["bash", str(SCRIPT)], cwd=repo, env=environment, capture_output=True, text=True
    )


def tags(env: dict[str, object]) -> dict[str, dict[str, str]]:
    path = env["tags"]
    return json.loads(path.read_text()) if path.exists() else {}


def test_version_bump_creates_a_lightweight_tag_despite_the_404_body(env) -> None:
    first = commit_version(env["repo"], "0.2.0", "v0.2.0")
    env["tags"].write_text(json.dumps({"cua-perception-v0.2.0": {"type": "commit", "sha": first}}))
    bump = commit_version(env["repo"], "0.2.1", "release 0.2.1")
    result = run_anchor(env, first, bump)
    assert result.returncode == 0, result.stdout + result.stderr
    assert tags(env)["cua-perception-v0.2.1"] == {"type": "commit", "sha": bump}


def test_missing_tag_heals_on_a_later_push_at_the_commit_that_set_version(env) -> None:
    first = commit_version(env["repo"], "0.2.0", "v0.2.0")
    bump = commit_version(env["repo"], "0.2.1", "release 0.2.1")
    later = commit_version(env["repo"], None, "unrelated fix")
    result = run_anchor(env, bump, later)
    assert result.returncode == 0, result.stdout + result.stderr
    assert tags(env)["cua-perception-v0.2.1"] == {"type": "commit", "sha": bump}
    assert "anchoring the commit that set VERSION" in result.stdout
    del first


def test_unchanged_version_with_existing_tag_is_a_no_op(env) -> None:
    bump = commit_version(env["repo"], "0.2.1", "release 0.2.1")
    env["tags"].write_text(json.dumps({"cua-perception-v0.2.1": {"type": "commit", "sha": bump}}))
    later = commit_version(env["repo"], None, "unrelated fix")
    result = run_anchor(env, bump, later)
    assert result.returncode == 0, result.stdout + result.stderr
    posts = [line for line in env["log"].read_text().splitlines() if "--method" in line]
    assert posts == []


def test_existing_tag_elsewhere_is_refused(env) -> None:
    first = commit_version(env["repo"], "0.2.0", "v0.2.0")
    bump = commit_version(env["repo"], "0.2.1", "release 0.2.1")
    env["tags"].write_text(json.dumps({"cua-perception-v0.2.1": {"type": "commit", "sha": first}}))
    result = run_anchor(env, first, bump)
    assert result.returncode == 1
    assert "is not a lightweight tag" in result.stdout


def test_annotated_tag_is_refused(env) -> None:
    first = commit_version(env["repo"], "0.2.0", "v0.2.0")
    bump = commit_version(env["repo"], "0.2.1", "release 0.2.1")
    env["tags"].write_text(json.dumps({"cua-perception-v0.2.1": {"type": "tag", "sha": bump}}))
    assert run_anchor(env, first, bump).returncode == 1


def test_version_decrease_is_refused(env) -> None:
    first = commit_version(env["repo"], "0.2.1", "v0.2.1")
    lower = commit_version(env["repo"], "0.2.0", "downgrade")
    result = run_anchor(env, first, lower)
    assert result.returncode == 1
    assert tags(env) == {}


def test_other_gh_errors_fail_closed(env) -> None:
    first = commit_version(env["repo"], "0.2.0", "v0.2.0")
    bump = commit_version(env["repo"], "0.2.1", "release 0.2.1")
    result = run_anchor(env, first, bump, FAKE_GH_MODE="server-error")
    assert result.returncode != 0
    assert tags(env) == {}


def test_initial_push_is_skipped(env) -> None:
    first = commit_version(env["repo"], "0.2.0", "v0.2.0")
    result = run_anchor(env, "0" * 40, first)
    assert result.returncode == 0
    assert tags(env) == {}
