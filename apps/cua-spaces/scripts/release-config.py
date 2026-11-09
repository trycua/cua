#!/usr/bin/env python3
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

"""Write the Tauri config overlay a Cua Spaces release build uses.

    python3 scripts/release-config.py --version "$VERSION" --repo "$GITHUB_REPOSITORY"

writes src-tauri/tauri.release.conf.json, which the release workflow passes
as `tauri build --config`. It holds:

  - the cua CLI sidecar (everything in src-tauri/tauri.sidecar.conf.json);
  - the updater endpoint on the repository that builds the release, so a
    staging build updates from its own `cua-spaces-latest` feed and a
    trycua/cua build from trycua/cua's;
  - the release version. A stable tag must equal src-tauri/tauri.conf.json's
    version (Release Please bumps both); a prerelease tag (X.Y.Z-suffix) is
    stamped in, with the numeric X.Y.Z as the MSI version (WiX takes numbers
    only).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

APP = Path(__file__).resolve().parent.parent
TAURI = APP / "src-tauri"
SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$")
REPO_RE = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})/[A-Za-z0-9._-]{1,100}$")
FEED = "https://github.com/{repo}/releases/download/cua-spaces-latest/latest.json"


def overlay(version: str, repo: str, base: dict, sidecar: dict) -> dict:
    m = SEMVER.match(version)
    if not m:
        raise ValueError(f"invalid version {version!r} (want X.Y.Z or X.Y.Z-suffix)")
    if not REPO_RE.match(repo):
        raise ValueError(f"invalid repository {repo!r} (want owner/name)")
    prerelease = m.group(4) is not None
    if not prerelease and version != base.get("version"):
        raise ValueError(
            f"tag version {version} does not match src-tauri/tauri.conf.json version {base.get('version')}"
        )
    conf = json.loads(json.dumps(sidecar))
    conf.pop("$schema", None)
    conf["version"] = version
    if prerelease:
        wix = conf.setdefault("bundle", {}).setdefault("windows", {}).setdefault("wix", {})
        wix["version"] = ".".join(m.group(i) for i in (1, 2, 3))
    updater = dict(base.get("plugins", {}).get("updater", {}))
    updater["endpoints"] = [FEED.format(repo=repo)]
    conf.setdefault("plugins", {})["updater"] = updater
    return conf


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--version", required=True)
    parser.add_argument("--repo", required=True, help="owner/name, e.g. $GITHUB_REPOSITORY")
    parser.add_argument("--out", type=Path, default=TAURI / "tauri.release.conf.json")
    args = parser.parse_args(argv)
    base = json.loads((TAURI / "tauri.conf.json").read_text())
    sidecar = json.loads((TAURI / "tauri.sidecar.conf.json").read_text())
    try:
        conf = overlay(args.version, args.repo, base, sidecar)
    except ValueError as err:
        print(err, file=sys.stderr)
        return 1
    args.out.write_text(json.dumps(conf, indent=2) + "\n")
    print(f"{args.out}: version {conf['version']}, updater {conf['plugins']['updater']['endpoints'][0]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
