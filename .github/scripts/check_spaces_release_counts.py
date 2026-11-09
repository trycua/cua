#!/usr/bin/env python3
"""Fail when a merged fix/feat PR under the Cua Spaces paths is missing from
both the open cua-spaces release PR and apps/cua-spaces-macos/CHANGELOG.md.

Release Please only counts conventional-commit titles, and only under the
package path and additional-paths. A miss means Spaces silently stops shipping.
"""
import argparse
import json
import re
import subprocess
import sys

SPACES_PATHS = ["apps/cua-spaces-macos", "apps/cua-spaces-desktop", "apps/cua-spaces-web", "libs/cua"]
RELEASABLE = re.compile(r"^(feat|fix|perf)(\([^)]*\))?!?: ")
PR_NUMBER = re.compile(r"\(#(\d+)\)\s*$")


def run(*cmd: str) -> str:
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def merged_spaces_commits(since: str, until: str):
    out = run("git", "log", "--first-parent", f"--since={since}", f"{until}", "--format=%H%x09%s", "--", *SPACES_PATHS)
    for line in out.splitlines():
        sha, _, subject = line.partition("\t")
        m = PR_NUMBER.search(subject)
        if m and RELEASABLE.match(subject):
            yield int(m.group(1)), subject


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--since", default="3 days ago")
    ap.add_argument("--ref", default="HEAD")
    ap.add_argument("--repo", required=True)
    args = ap.parse_args()

    prs = json.loads(run("gh", "api", f"repos/{args.repo}/pulls?state=open&per_page=100"))
    release_text = "\n".join(
        p.get("body") or "" for p in prs if "components--cua-spaces" in p["head"]["ref"]
    )
    changelog = run("git", "show", f"{args.ref}:apps/cua-spaces-macos/CHANGELOG.md")
    seen = release_text + "\n" + changelog

    missing = [(n, s) for n, s in merged_spaces_commits(args.since, args.ref) if f"/{n})" not in seen and f"#{n}" not in seen]
    for n, s in missing:
        print(f"::error::#{n} is a releasable Spaces change but is in neither the cua-spaces release PR nor its CHANGELOG: {s}")
    if missing:
        return 1
    print("every releasable Spaces change is counted")
    return 0


if __name__ == "__main__":
    sys.exit(main())
