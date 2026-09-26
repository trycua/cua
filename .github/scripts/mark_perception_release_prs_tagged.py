#!/usr/bin/env python3
"""Mark merged Cua Perception release pull requests as tagged.

Release Please refuses to open or update any release pull request while a
merged release pull request still carries ``autorelease: pending``. It swaps
that label for ``autorelease: tagged`` only when it creates a GitHub release
itself. Cua Perception sets ``skip-github-release`` and is tagged by
``anchor_perception_release_tag.sh`` instead, so its merged release pull
request keeps the pending label forever and blocks every other component.

This script closes that gap. For each merged Perception release pull request
that is still pending, it reads the version the merge commit set and relabels
the pull request only when ``cua-perception-v<version>`` is a lightweight tag
at that exact merge commit. Anything else is left pending, so Release Please
keeps failing closed until the tag is correct.

Inputs (environment): GITHUB_REPOSITORY, GH_TOKEN (used by gh).
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys

BRANCH = "release-please--branches--main--components--cua-perception"
VERSION_PATH = "libs/cua-driver/rust/crates/cua-perception/VERSION"
PENDING = "autorelease: pending"
TAGGED = "autorelease: tagged"
SEMVER = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")


def run(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(list(args), check=check, capture_output=True, text=True)


def pending_pull_requests(repository: str) -> list[dict]:
    result = run(
        "gh", "pr", "list",
        "--repo", repository,
        "--state", "merged",
        "--base", "main",
        "--label", PENDING,
        "--search", f"head:{BRANCH}",
        "--limit", "100",
        "--json", "number,headRefName,mergeCommit",
    )
    pulls = json.loads(result.stdout or "[]")
    # Search matches head-branch prefixes, so require the exact bot branch.
    return [pull for pull in pulls if pull.get("headRefName") == BRANCH]


def merged_version(sha: str) -> str | None:
    result = run("git", "show", f"{sha}:{VERSION_PATH}", check=False)
    if result.returncode != 0:
        return None
    version = result.stdout.strip()
    return version if SEMVER.fullmatch(version) else None


def remote_tag(repository: str, tag: str) -> tuple[str, str] | None:
    result = run("gh", "api", f"repos/{repository}/git/ref/tags/{tag}", check=False)
    try:
        body = json.loads(result.stdout or "{}")
    except json.JSONDecodeError:
        body = {}
    if result.returncode == 0:
        return body["object"]["type"], body["object"]["sha"]
    if body.get("status") == "404" or body.get("message") == "Not Found":
        return None
    raise SystemExit(f"::error::Could not read {tag} (gh exit {result.returncode})")


def main() -> int:
    repository = os.environ["GITHUB_REPOSITORY"]
    pulls = pending_pull_requests(repository)
    if not pulls:
        print("No merged Perception release pull request is pending")
        return 0

    for pull in pulls:
        number = pull["number"]
        sha = (pull.get("mergeCommit") or {}).get("oid")
        if not sha:
            print(f"::warning::#{number} has no merge commit; leaving it pending")
            continue
        version = merged_version(sha)
        if version is None:
            print(f"::warning::#{number} merge {sha} sets no valid {VERSION_PATH}; leaving it pending")
            continue
        tag = f"cua-perception-v{version}"
        record = remote_tag(repository, tag)
        if record != ("commit", sha):
            print(
                f"::warning::{tag} is not a lightweight tag at #{number}'s merge "
                f"commit {sha} (found {record}); leaving it pending"
            )
            continue
        run(
            "gh", "pr", "edit", str(number),
            "--repo", repository,
            "--remove-label", PENDING,
            "--add-label", TAGGED,
        )
        print(f"Marked #{number} as tagged at {tag}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
