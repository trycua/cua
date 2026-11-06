"""Nothing in trycua/cua may depend on GitHub's repo-wide "Latest" release.

trycua/cua releases many products (cua-spaces, cua-spacesd, cua-driver-rs,
cua-sdk, lume, ...) from one repository, so "Latest" is whichever shipped last.
Consumers read an explicit per-product source instead: a baked version
constant, a release list filtered by tag prefix, or a fixed-tag rolling release
(`cua-install-latest`, `cua-spaces-latest`) that holds a feed or manifest.
"Latest" is cosmetic (see test_release_latest_policy for who pins it)."""

import re
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
THIS = Path(__file__).resolve()
# The guard reads Latest only to put it back on Cua Spaces; it is not a consumer.
GUARD = ".github/scripts/pin_latest_release.py"

# A lookup of the repo-wide Latest on trycua/cua, however it is spelled.
CONSUMERS = [
    re.compile(r"(trycua/cua|GITHUB_REPO\w*|\{repository\}|\{owner\}/\{repo\})[^\n]*releases/latest"),
    re.compile(r"releases/latest[^\n]*trycua/cua"),
    re.compile(r"github\.com/trycua/cua/releases/latest"),
    re.compile(r"gh release view\s*(--|$)"),  # no tag: resolves to Latest
    re.compile(r"gh release download\s*(--|$)"),  # no tag: resolves to Latest
    re.compile(r"\bisLatest\b"),
]


def _files():
    out = subprocess.run(
        ["git", "ls-files", "-z"], cwd=REPO_ROOT, check=True, capture_output=True, text=True
    ).stdout
    for rel in filter(None, out.split("\0")):
        path = REPO_ROOT / rel
        if (
            path == THIS
            or rel == GUARD
            or rel.startswith((".github/release-backfill/", "blog/", "libs/spaces-sdk-swift/"))
            or rel.endswith(("CHANGELOG.md", ".lock", ".lockb"))
            or "/tests/" in rel
            or not path.is_file()
        ):
            continue
        try:
            yield rel, path.read_text()
        except (UnicodeDecodeError, OSError):
            continue


def test_nothing_reads_the_repo_wide_latest_release():
    bad = []
    for rel, text in _files():
        for n, line in enumerate(text.splitlines(), 1):
            if any(p.search(line) for p in CONSUMERS):
                bad.append(f"{rel}:{n}: {line.strip()[:120]}")
    assert not bad, (
        "these read GitHub's repo-wide Latest release; read a per-product source instead "
        "(baked version, tag-prefix filtered list, or a fixed-tag feed):\n" + "\n".join(bad)
    )
