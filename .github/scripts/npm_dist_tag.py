#!/usr/bin/env python3
"""Choose the npm dist-tag for publishing a version of one package.

npm refuses to implicitly apply `latest` when a higher version is already
published (E403 "Cannot implicitly apply the latest tag ..."). That happens
legitimately when two releases publish out of order, e.g. 0.63.1 finishing
before 0.63.0. The tag must therefore be derived from the registry state:

  - prerelease version                    -> `next`
  - higher than the current `latest`      -> `latest`
  - lower than the current `latest`       -> `backport-<major>.<minor>`
    (a late release of an older line never moves `latest` backwards)

Usage: npm_dist_tag.py <version> [<current-latest>]
Prints the tag. Exit 2 on an invalid version.
"""

from __future__ import annotations

import re
import sys

SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$")


def parse(version: str) -> tuple[tuple[int, int, int], str | None]:
    match = SEMVER.match(version)
    if not match:
        raise ValueError(f"not a semver version: {version!r}")
    core = (int(match[1]), int(match[2]), int(match[3]))
    return core, match[4]


def dist_tag(version: str, current_latest: str | None) -> str:
    core, prerelease = parse(version)
    if prerelease:
        return "next"
    if not current_latest:
        return "latest"
    latest_core, latest_pre = parse(current_latest)
    if latest_pre or core > latest_core:
        return "latest"
    if core == latest_core:
        return "latest"
    return f"backport-{core[0]}.{core[1]}"


def main(argv: list[str]) -> int:
    if len(argv) not in (2, 3):
        print(__doc__, file=sys.stderr)
        return 2
    try:
        print(dist_tag(argv[1], argv[2] if len(argv) == 3 else None))
    except ValueError as error:
        print(error, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
