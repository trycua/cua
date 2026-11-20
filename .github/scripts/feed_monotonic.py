#!/usr/bin/env python3
"""Never let an older release overwrite a newer Electron updater feed.

Two Cua Spaces CD runs can finish out of order: 0.38.12's run ended after
0.38.13's and its latest.yml replaced 0.38.13's (18 Nov 2026), so Windows and
Linux apps were offered 0.38.12. The Sparkle appcast does not have this race
because make-appcast.sh merges into the published file.

  python3 feed_monotonic.py select FEED_DIR OUT_DIR --repo R [--tag T]

Copies each FEED_DIR/*.yml to OUT_DIR unless the feed of the same name already
published on the rolling release names a higher semver version; the skip is
logged. An equal version is kept (a re-run republishes the same feed). The
caller uploads OUT_DIR/*.yml.
"""
import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SEMVER = re.compile(r"^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$")


def feed_version(text):
    m = re.search(r"^version:\s*['\"]?([^'\"\s]+)['\"]?\s*$", text, re.M)
    if not m:
        raise ValueError("no top-level version: in feed")
    return m.group(1)


def semver_key(version):
    """Sortable key following semver 2.0 precedence (build metadata ignored)."""
    m = SEMVER.match(version)
    if not m:
        raise ValueError("not a semver version: %r" % version)
    core = tuple(int(m.group(i)) for i in (1, 2, 3))
    if m.group(4) is None:
        return core, (1,)  # a release outranks any of its prereleases
    ids = tuple((0, int(p), "") if p.isdigit() else (1, 0, p) for p in m.group(4).split("."))
    return core, (0,) + ids


def is_older(candidate, published):
    return semver_key(candidate) < semver_key(published)


def select(feed_dir, out_dir, published_text):
    """published_text(name) -> text of the published feed, or None if absent."""
    out_dir.mkdir(parents=True, exist_ok=True)
    kept = []
    for path in sorted(feed_dir.glob("*.yml")):
        mine = feed_version(path.read_text())
        theirs_text = published_text(path.name)
        if theirs_text is not None:
            theirs = feed_version(theirs_text)
            if is_older(mine, theirs):
                print("::warning::skipping %s: this run's %s is older than the published %s" % (path.name, mine, theirs))
                continue
        print("%s: publishing %s" % (path.name, mine))
        shutil.copy(path, out_dir / path.name)
        kept.append(path.name)
    return kept


def gh_published(repo, tag):
    assets = json.loads(subprocess.run(
        ["gh", "release", "view", tag, "--repo", repo, "--json", "assets"],
        check=True, capture_output=True, text=True).stdout)["assets"]
    names = {a["name"] for a in assets}

    def fetch(name):
        if name not in names:
            return None
        with tempfile.TemporaryDirectory() as d:
            subprocess.run(["gh", "release", "download", tag, "--pattern", name, "--dir", d, "--repo", repo, "--clobber"],
                           check=True, capture_output=True)
            return (Path(d) / name).read_text()
    return fetch


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["select"])
    ap.add_argument("feed_dir", type=Path)
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("--repo", required=True)
    ap.add_argument("--tag", default="cua-spaces-latest")
    a = ap.parse_args(argv)
    select(a.feed_dir, a.out_dir, gh_published(a.repo, a.tag))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
