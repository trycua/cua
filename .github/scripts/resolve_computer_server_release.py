"""Resolve an existing computer-server tag to reviewed, version-matching source."""

import os
import re
import subprocess
import tomllib
from pathlib import Path

VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")
PREFIX = "refs/tags/computer-server-v"


def git(*arguments):
    return subprocess.check_output(["git", *arguments], text=True, timeout=60).strip()


def resolve(version, ref):
    selected = version or (ref.removeprefix(PREFIX) if ref.startswith(PREFIX) else "")
    if not VERSION.fullmatch(selected):
        raise ValueError("a stable computer-server version is required")
    if ref.startswith(PREFIX) and ref != PREFIX + selected:
        raise ValueError("release input and tag disagree")
    sha = git("rev-parse", "--verify", f"{PREFIX}{selected}^{{commit}}")
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("invalid release commit")
    # A release is from landed source, not an arbitrary unmerged tag.
    git("merge-base", "--is-ancestor", sha, "refs/remotes/origin/main")
    project = tomllib.loads(git("show", f"{sha}:libs/python/computer-server/pyproject.toml"))
    if (
        project["project"]["name"] != "cua-computer-server"
        or project["project"]["version"] != selected
    ):
        raise ValueError("release tag and package metadata disagree")
    return {"version": selected, "sha": sha}


def main():
    result = resolve(os.environ.get("RELEASE_VERSION", ""), os.environ.get("RELEASE_REF", ""))
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
        for key, value in result.items():
            output.write(f"{key}={value}\n")
    print(f"computer-server {result['version']} source {result['sha']}")


if __name__ == "__main__":
    main()
