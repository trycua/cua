"""Widen the upper bound other packages put on a package that is being released.

Packages such as cua-bench and cua-agent depend on `cua-sandbox>=0.8.0,<0.10.0`.
A sandbox release that crosses that cap (0.9.x to 0.10.0) makes every install
that pairs the checkout's sandbox with those packages unsatisfiable: the ALE
jobs ("No solution found when resolving dependencies") fail on the release PR
until someone edits each cap by hand.

For one package this raises `<X` to the next minor of the release version in
every tracked pyproject.toml, but only where the previous release satisfied the
requirement. A cap that was already behind (cua-cli's `<0.2.0`) was a
deliberate choice, so it is left alone. Only the specifier text changes, so no
lockfile is affected: uv.lock files record path dependencies without it.
Prints the changed files; staging is the caller's.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess

VERSION = r"\d+\.\d+(?:\.\d+)?"


def parse(version: str) -> tuple[int, int, int]:
    parts = [int(part) for part in version.split(".")]
    return tuple((parts + [0, 0])[:3])  # type: ignore[return-value]


def next_minor(version: str) -> str:
    major, minor, _ = parse(version)
    return f"{major}.{minor + 1}.0"


def satisfied(specifier: str, version: tuple[int, int, int]) -> bool:
    for op, bound in re.findall(rf"(>=|<=|==|<|>)\s*({VERSION})", specifier):
        other = parse(bound)
        if not {">=": version >= other, "<=": version <= other, "==": version == other,
                "<": version < other, ">": version > other}[op]:
            return False
    return True


def widen(text: str, package: str, previous: str, release: str) -> str:
    # A quoted requirement such as "cua-sandbox>=0.8.0,<0.10.0" or with extras/markers.
    pattern = re.compile(
        rf'(?P<head>"{re.escape(package)}(?:\[[^\]]*\])?\s*)(?P<spec>(?:[<>=!~]=?\s*{VERSION}\s*,?\s*)+)(?P<tail>[";])'
    )

    def replace(match: re.Match[str]) -> str:
        spec = match["spec"]
        cap = re.search(rf"<\s*({VERSION})", spec)
        if cap is None or parse(cap[1]) > parse(release):
            return match[0]
        if not satisfied(spec, parse(previous)):
            return match[0]
        return match["head"] + spec.replace(cap[0], f"<{next_minor(release)}") + match["tail"]

    return pattern.sub(replace, text)


def tracked_pyprojects(root: Path) -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", "-z", "--", "pyproject.toml", "**/pyproject.toml"],
        cwd=root, check=True, capture_output=True, text=True,
    ).stdout
    return sorted({path for path in out.split("\0") if path})


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Widen caps on a package being released.")
    parser.add_argument("--repo-root", dest="root", type=Path, default=Path.cwd())
    parser.add_argument("--package", required=True)
    parser.add_argument("--from", dest="previous", required=True, help="previous release version")
    parser.add_argument("--to", dest="release", required=True, help="release version")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    changed = []
    for rel in tracked_pyprojects(root):
        path = root / rel
        text = path.read_text()
        updated = widen(text, args.package, args.previous, args.release)
        if updated != text:
            path.write_text(updated)
            changed.append(rel)
    print("\n".join(changed))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
