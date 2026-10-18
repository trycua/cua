#!/usr/bin/env python3
"""Build the Cua Spaces auto-updater feed (`latest.json`) for the Tauri app.

The Tauri app ships on Linux and Windows. On macOS the product is the SwiftUI
app (apps/cua-spaces-macos), which does not read this feed, so the feed lists
no darwin platform: a Tauri build installed on a Mac earlier finds no update
here and is never moved onto another Tauri build.

The Linux and Windows jobs upload their updater artifacts next to the
installers under stable names, each with the minisign `.sig` the Tauri bundler
wrote. Once every platform has finished, the release workflow calls:

    updater_feed.py --version 0.2.0 --dir assets \
        --base-url https://github.com/trycua/cua/releases/download/cua-spaces-v0.2.0 \
        --out latest.json

Artifacts it reads from --dir (each needs `<name>.sig`):

    cua-spaces-<version>-linux-x64.AppImage     -> linux-x86_64(-appimage)
    cua-spaces-<version>-linux-arm64.AppImage   -> linux-aarch64(-appimage)
    cua-spaces-<version>-windows-x64-setup.exe  -> windows-x86_64(-nsis)
    cua-spaces-<version>-windows-x64.msi        -> windows-x86_64-msi

The feed must end up with linux-x86_64, linux-aarch64 and windows-x86_64, or
the tool fails: a feed that silently drops a platform strands those installs
on their current version. A --merge feed (notes, pub_date) must not list a
darwin platform.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import sys
from pathlib import Path

REQUIRED = ("linux-x86_64", "linux-aarch64", "windows-x86_64")

# (file suffix after "cua-spaces-<version>-", feed keys)
ARTIFACTS = (
    ("linux-x64.AppImage", ("linux-x86_64", "linux-x86_64-appimage")),
    ("linux-arm64.AppImage", ("linux-aarch64", "linux-aarch64-appimage")),
    ("windows-x64-setup.exe", ("windows-x86_64", "windows-x86_64-nsis")),
    ("windows-x64.msi", ("windows-x86_64-msi",)),
)


def build(version: str, directory: Path, base_url: str, merge: Path | None, notes: str) -> dict:
    feed: dict = {}
    if merge is not None and merge.is_file():
        feed = json.loads(merge.read_text())
        if feed.get("version", version).lstrip("v") != version:
            raise SystemExit(f"{merge}: version {feed.get('version')} does not match --version {version}")
    platforms: dict = dict(feed.get("platforms", {}))
    darwin = sorted(key for key in platforms if key.startswith("darwin"))
    if darwin:
        raise SystemExit(
            f"{merge}: lists {', '.join(darwin)}; the macOS app is the SwiftUI app, which does not "
            "read this feed, so it must not point Macs at a Tauri build"
        )
    for suffix, keys in ARTIFACTS:
        name = f"cua-spaces-{version}-{suffix}"
        artifact = directory / name
        signature = directory / f"{name}.sig"
        if not artifact.is_file():
            continue
        if not signature.is_file():
            raise SystemExit(f"{name}: updater signature {signature.name} is missing")
        entry = {"signature": signature.read_text().strip(), "url": f"{base_url.rstrip('/')}/{name}"}
        for key in keys:
            platforms[key] = entry
    missing = [key for key in REQUIRED if key not in platforms]
    if missing:
        raise SystemExit(f"updater feed is missing platforms: {', '.join(missing)}")
    for key, entry in platforms.items():
        if not entry.get("signature") or not entry.get("url"):
            raise SystemExit(f"{key}: feed entry needs both a signature and a url")
    pub_date = feed.get("pub_date") or dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    return {
        "version": version,
        "notes": feed.get("notes") or notes,
        "pub_date": pub_date,
        "platforms": dict(sorted(platforms.items())),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--version", required=True)
    parser.add_argument("--dir", required=True, help="directory holding the Linux/Windows updater artifacts")
    parser.add_argument("--base-url", required=True, help="download URL prefix of the versioned release")
    parser.add_argument("--merge", help="an earlier feed for this version (notes, pub_date; no darwin entries)")
    parser.add_argument("--notes", default="See the release notes on GitHub.")
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)
    feed = build(args.version, Path(args.dir), args.base_url, Path(args.merge) if args.merge else None, args.notes)
    Path(args.out).write_text(json.dumps(feed, indent=2) + "\n")
    print(f"wrote {args.out}: {', '.join(feed['platforms'])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
