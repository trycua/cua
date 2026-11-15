#!/usr/bin/env python3
"""Close older post-publish installer bookkeeping PRs once a newer one opens."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from typing import Sequence

BRANCH_PREFIX = "post-publish-installer-"
TITLE = re.compile(
    r"^chore\(cua-driver\): advance published installer version to "
    r"(\d+)\.(\d+)\.(\d+)$"
)
STABLE_VERSION = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")


def parse_version(text: str) -> tuple[int, int, int] | None:
    match = STABLE_VERSION.fullmatch(text)
    return tuple(int(part) for part in match.groups()) if match else None


def select_superseded(
    open_prs: Sequence[dict], new_pr_number: int, new_version: str
) -> list[dict]:
    """Return open bookkeeping PRs that the new PR makes obsolete.

    Only PRs from this workflow's branches, with an exact bookkeeping title for
    a version at or below the new one, are selected. Anything else is left alone.
    """
    new = parse_version(new_version)
    if new is None:
        raise ValueError(f"not an exact stable x.y.z version: {new_version!r}")
    selected = []
    for pr in open_prs:
        if pr["number"] == new_pr_number:
            continue
        if not pr["headRefName"].startswith(BRANCH_PREFIX):
            continue
        match = TITLE.fullmatch(pr["title"])
        if match is None:
            continue
        if tuple(int(part) for part in match.groups()) <= new:
            selected.append(pr)
    return selected


def gh(*args: str) -> str:
    return subprocess.run(
        ["gh", *args], check=True, capture_output=True, text=True
    ).stdout


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--new-pr", type=int, required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args(argv)

    open_prs = json.loads(
        gh(
            "pr", "list", "--repo", args.repo, "--state", "open",
            "--limit", "100", "--json", "number,title,headRefName",
        )
    )
    for pr in select_superseded(open_prs, args.new_pr, args.version):
        number = str(pr["number"])
        try:
            gh(
                "pr", "close", number, "--repo", args.repo, "--delete-branch",
                "--comment", f"Superseded by #{args.new_pr}.",
            )
            print(f"Closed #{number} ({pr['headRefName']}), superseded by #{args.new_pr}")
        except subprocess.CalledProcessError as error:
            print(f"Could not close #{number}: {error.stderr}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
