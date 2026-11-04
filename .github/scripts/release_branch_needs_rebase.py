#!/usr/bin/env python3
"""Decide whether a Release Please branch must be rebased onto main.

Every rebase force-pushes the branch, and every force-push restarts the
release PR's whole check suite. Rebase only when it is needed:

* the branch no longer merges cleanly into main, or
* commits that touch this component's paths (or the shared release tooling)
  landed on main after the branch's merge base, so the changelog and
  generated files could be stale.

Prints ``rebase <reason>`` or ``keep <reason>`` and always exits 0 on a
decision. Release Please itself still regenerates the branch whenever a
relevant commit lands, so the changelog stays complete.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

COMPONENT_PREFIX = "release-please--branches--main--components--"
# Shared inputs that change how every release branch is generated or checked.
SHARED_PATHS = (
    ".github/scripts",
    "release-please-config.json",
)


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args], check=check, capture_output=True, text=True
    )


def component_paths(config: dict, branch: str) -> list[str]:
    if not branch.startswith(COMPONENT_PREFIX):
        return []
    component = branch[len(COMPONENT_PREFIX) :]
    return [
        path
        for path, package in config.get("packages", {}).items()
        if package.get("component") == component
    ]


def decide(config: dict, branch: str, base: str, head: str) -> tuple[str, str]:
    paths = component_paths(config, branch)
    if not paths:
        return "rebase", "unknown component; keep the previous behavior"

    merge_tree = git("merge-tree", "--write-tree", base, head, check=False)
    if merge_tree.returncode != 0:
        return "rebase", "branch conflicts with main"

    merge_base = git("merge-base", base, head).stdout.strip()
    changed = git(
        "diff", "--name-only", merge_base, base, "--", *paths, *SHARED_PATHS
    ).stdout.split()
    if changed:
        return "rebase", f"main changed {len(changed)} relevant file(s), e.g. {changed[0]}"
    return "keep", "no relevant commits landed on main since the branch base"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--branch", required=True)
    parser.add_argument("--base", default="origin/main")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--config", default="release-please-config.json")
    args = parser.parse_args()
    with open(args.config, encoding="utf-8") as handle:
        config = json.load(handle)
    action, reason = decide(config, args.branch, args.base, args.head)
    print(f"{action} {reason}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
