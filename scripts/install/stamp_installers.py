#!/usr/bin/env python3
"""Copy install.sh / install.ps1 with the release repository as their default.

The installers default to trycua/cua: their download base and the Sigstore
identity they verify (the release workflow at its tag) both derive from it.
A release workflow publishes the installers from the repository that built
the release, so it stamps that repository in (a staging repository's feed
then installs, and verifies, its own releases):

    python3 scripts/install/stamp_installers.py --repo "$GITHUB_REPOSITORY" --out stamped

writes stamped/install.sh and stamped/install.ps1. CUA_INSTALL_REPO still
overrides the default at install time. Fails if a default is not found, so a
changed installer cannot silently keep pointing somewhere else.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})/[A-Za-z0-9._-]{1,100}$")

# (file, the exact default line fragment, its replacement template)
DEFAULTS = {
    "install.sh": ('REPO="${CUA_INSTALL_REPO:-trycua/cua}"', 'REPO="${{CUA_INSTALL_REPO:-{repo}}}"'),
    "install.ps1": (
        "$Repo = if ($env:CUA_INSTALL_REPO) { $env:CUA_INSTALL_REPO } else { 'trycua/cua' }",
        "$Repo = if ($env:CUA_INSTALL_REPO) {{ $env:CUA_INSTALL_REPO }} else {{ '{repo}' }}",
    ),
}


def stamp(text: str, name: str, repo: str) -> str:
    old, template = DEFAULTS[name]
    if text.count(old) != 1:
        raise ValueError(f"{name}: expected exactly one default repository line {old!r}")
    return text.replace(old, template.format(repo=repo))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--repo", required=True, help="owner/name, e.g. $GITHUB_REPOSITORY")
    parser.add_argument("--out", required=True, type=Path, help="directory for the stamped copies")
    parser.add_argument("--src", type=Path, default=HERE, help="directory holding install.sh / install.ps1")
    args = parser.parse_args(argv)
    if not REPO_RE.match(args.repo):
        print(f"invalid repository {args.repo!r} (want owner/name)", file=sys.stderr)
        return 2
    args.out.mkdir(parents=True, exist_ok=True)
    for name in DEFAULTS:
        text = (args.src / name).read_text(encoding="utf-8")
        try:
            stamped = stamp(text, name, args.repo)
        except ValueError as err:
            print(err, file=sys.stderr)
            return 1
        dest = args.out / name
        # newline="" keeps each file's line endings byte for byte.
        with open(dest, "w", encoding="utf-8", newline="") as fh:
            fh.write(stamped)
        dest.chmod((args.src / name).stat().st_mode)
        print(f"{dest}: default repository {args.repo}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
