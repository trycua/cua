"""Raise the upper bound of cua-sandbox's `driver` extra to cover a Cua Driver release.

The extra pins `cua-driver>=LOW,<0.NN`. Cua Driver is 0.x, so the bound stays below
the next minor, and every new minor used to need the same hand edit across the
pyproject, the README, the docs, two tests and five uv.lock files. This is that edit,
done by one reviewed script (called from release-please.yml, run from main, never from
a pull request branch).

    sync_cua_sandbox_driver_bound.py --driver-version 0.55.0 [--repo-root DIR] [--check]

- The new bound is the next minor after the driver version (`0.55.0` -> `<0.56`).
- The bound only moves up. A driver version already inside the range changes nothing.
- A non-0.x driver (1.0 and later) is refused: raising a major is a human decision.
- The version is validated before use, and the result is asserted to cover it.

Prints the changed files, one per line; staging is the caller's. `--check` changes
nothing and exits 1 when the driver version is outside the range.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import sys

PYPROJECT = "libs/python/cua-sandbox/pyproject.toml"

# Every file that records the bound. A test asserts this list is complete.
FILES = (
    PYPROJECT,
    "libs/python/cua-sandbox/README.md",
    "docs/content/docs/cua-sdk/reference/runtime-support.mdx",
    "libs/python/cua-sandbox/uv.lock",
    "libs/python/cua-sandbox-apps/uv.lock",
    "libs/cua-bench/uv.lock",
    "libs/cua-bench-rl/uv.lock",
    "uv.lock",
)

SPEC = re.compile(r'"cua-driver(?P<spec>>=(?P<low>\d+\.\d+\.\d+),<0\.(?P<cap>\d+))"')
VERSION = re.compile(r"^0\.(?P<minor>\d+)\.(?P<patch>\d+)$")


class BoundError(Exception):
    pass


def current_spec(root: Path) -> tuple[str, int]:
    """The (lower bound, upper-bound minor) now in the pyproject."""
    found = SPEC.findall((root / PYPROJECT).read_text())
    if len(found) != 1:
        raise BoundError(f"expected one cua-driver range in {PYPROJECT}, found {len(found)}")
    _, low, cap = found[0]
    return low, int(cap)


def needed_cap(version: str) -> int:
    match = VERSION.fullmatch(version)
    if match is None:
        raise BoundError(
            f"{version!r} is not a stable 0.x.y Cua Driver version; "
            "raising the bound past 0.x needs a human decision"
        )
    return int(match["minor"]) + 1


def sync(root: Path, version: str) -> list[str]:
    low, cap = current_spec(root)
    parts = [int(p) for p in version.split(".")] if VERSION.fullmatch(version) else None
    new_cap = needed_cap(version)
    if parts is not None and tuple(parts) < tuple(int(p) for p in low.split(".")):
        raise BoundError(f"{version} is below the lower bound {low}")
    if new_cap <= cap:
        return []  # already covered; the bound never moves down
    old, new = f">={low},<0.{cap}", f">={low},<0.{new_cap}"
    changed = []
    for rel in FILES:
        path = root / rel
        text = path.read_text()
        if old not in text:
            raise BoundError(f"{rel} does not record {old}")
        path.write_text(text.replace(old, new))
        changed.append(rel)
    if current_spec(root)[1] != new_cap:
        raise BoundError("the pyproject bound did not change as expected")
    return changed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--driver-version", required=True)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    root = args.repo_root.resolve()
    try:
        if args.check:
            _, cap = current_spec(root)
            if needed_cap(args.driver_version) <= cap:
                return 0
            print(f"cua-driver {args.driver_version} is outside the bound <0.{cap}", file=sys.stderr)
            return 1
        print("\n".join(sync(root, args.driver_version)))
    except BoundError as err:
        print(f"::error::{err}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
