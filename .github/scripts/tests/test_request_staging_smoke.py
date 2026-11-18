"""The staging smoke request fails loudly when it cannot reach trycua/cloud.

v0.17.0-beta.6 waited two hours at its staging gate while this script only
warned: the release app could not mint a token for trycua/cloud, so the
cua-release-candidate dispatch was never sent and the job showed green.
"""

from __future__ import annotations

import os
import pathlib
import subprocess

import pytest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
SCRIPT = REPO_ROOT / ".github/scripts/request_staging_smoke.sh"


def run(tmp_path: pathlib.Path, token: str, gh_ok: bool) -> subprocess.CompletedProcess:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=True)
    gh = bin_dir / "gh"
    gh.write_text(f"#!/bin/sh\necho \"$@\" >> {tmp_path}/gh.log\nexit {0 if gh_ok else 1}\n")
    gh.chmod(0o755)
    env = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}", "GH_TOKEN": token, "GITHUB_STEP_SUMMARY": str(tmp_path / "summary")}
    return subprocess.run(["bash", str(SCRIPT), "cua-spaces", "abc123"], env=env, capture_output=True, text=True)


def test_a_sent_dispatch_passes(tmp_path):
    r = run(tmp_path, "t", gh_ok=True)
    assert r.returncode == 0, r.stdout + r.stderr
    assert "event_type=cua-release-candidate" in (tmp_path / "gh.log").read_text()
    assert "Requested the staging smoke for cua-spaces at abc123" in r.stdout


@pytest.mark.parametrize("token,gh_ok", [("", True), ("t", False)])
def test_no_dispatch_fails_with_an_error(tmp_path, token, gh_ok):
    r = run(tmp_path, token, gh_ok)
    assert r.returncode == 1
    assert "::error title=Staging smoke::" in r.stdout
    assert "gh workflow run spaces-staging-smoke.yml -R trycua/cloud -f component=cua-spaces -f sha=abc123" in r.stdout
