#!/usr/bin/env python3
"""Check a release's `checksums.txt` and `release-artifacts.json` against its files.

    check_release_manifest.py --dir release/ --component app --version 0.8.0 \
        [--base-url https://github.com/trycua/cua/releases/download/cua-spaces-v0.8.0]

DIR holds every asset of the release (as `gh release download` writes them).
It fails unless:

- every asset (all but checksums.txt, release-artifacts.json and latest.json;
  signatures included) has a line in checksums.txt whose sha256 is the
  file's, and every line names an asset that is here;
- every installable asset of COMPONENT at VERSION (each name release_manifest.py
  recognises: the SwiftUI, Electron, Tauri and CLI artifacts) is listed in
  release-artifacts.json under that component and version, with the file's
  sha256 and size and, with --base-url, its download URL; and every listed
  artifact of that component names an asset that is here. With
  --allow-carried-forward (a partial release), an entry of another version is
  a platform's earlier build: it must carry a url and sha256 but need not name
  an asset of this release.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from release_manifest import classify, sha256  # noqa: E402

META = {"checksums.txt", "release-artifacts.json", "latest.json"}


def check(directory: Path, component: str, version: str, base_url: str | None, carried: bool = False) -> list[str]:
    errors: list[str] = []
    assets = {
        p.name: p
        for p in sorted(directory.iterdir())
        if p.is_file() and p.name not in META
    }
    digests = {name: sha256(path) for name, path in assets.items()}

    checksums_path = directory / "checksums.txt"
    if not checksums_path.is_file():
        errors.append("no checksums.txt")
    else:
        listed: dict[str, str] = {}
        for line in checksums_path.read_text().splitlines():
            if not line.strip():
                continue
            digest, _, name = line.partition("  ")
            listed[name.lstrip("*").strip()] = digest.strip()
        for name, digest in digests.items():
            if name not in listed:
                errors.append(f"checksums.txt does not list {name}")
            elif listed[name] != digest:
                errors.append(f"checksums.txt: {name} sha256 {listed[name]} is not the file's {digest}")
        for name in listed:
            if name not in assets:
                errors.append(f"checksums.txt lists {name}, which is not on the release")

    manifest_path = directory / "release-artifacts.json"
    if not manifest_path.is_file():
        errors.append("no release-artifacts.json")
        return errors
    manifest = json.loads(manifest_path.read_text())
    if manifest.get("versions", {}).get(component) != version:
        errors.append(f"release-artifacts.json: {component} version {manifest.get('versions', {}).get(component)!r}, want {version!r}")
    entries = [a for a in manifest.get("artifacts", []) if a.get("component") == component]
    by_name: dict[str, list[dict]] = {}
    for entry in entries:
        by_name.setdefault(entry.get("name", ""), []).append(entry)
    for name, path in assets.items():
        found = classify(name)
        if not found or found[0] != component:
            continue
        if found[3] != version:
            errors.append(f"{name} is not a {version} artifact")
            continue
        rows = by_name.get(name)
        if not rows:
            errors.append(f"release-artifacts.json does not list {name}")
            continue
        size = path.stat().st_size
        for row in rows:
            where = f"release-artifacts.json: {name} ({row.get('platform')})"
            if row.get("sha256") != digests[name]:
                errors.append(f"{where} sha256 {row.get('sha256')} is not the file's {digests[name]}")
            if row.get("size") != size:
                errors.append(f"{where} size {row.get('size')} is not the file's {size}")
            if row.get("version") != version:
                errors.append(f"{where} version {row.get('version')!r}, want {version!r}")
            if base_url and row.get("url") != f"{base_url.rstrip('/')}/{name}":
                errors.append(f"{where} url {row.get('url')} is not {base_url.rstrip('/')}/{name}")
    for name, rows in by_name.items():
        # A partial release keeps other platforms' entries from their own
        # (older) release; they name assets of that release, not this one.
        if carried and all(row.get("version") != version for row in rows):
            for row in rows:
                if not row.get("url") or not row.get("sha256"):
                    errors.append(f"release-artifacts.json: carried-forward {name} ({row.get('platform')}) lacks a url or sha256")
            continue
        if name not in assets:
            errors.append(f"release-artifacts.json lists {name}, which is not on the release")
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--dir", required=True)
    parser.add_argument("--component", choices=("cli", "app"), required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--base-url")
    parser.add_argument("--allow-carried-forward", action="store_true",
                        help="partial release: entries of another version are other platforms' earlier builds")
    args = parser.parse_args(argv)
    errors = check(Path(args.dir), args.component, args.version, args.base_url, args.allow_carried_forward)
    for error in errors:
        print(f"FAIL {error}")
    if errors:
        return 1
    print(f"checksums.txt and release-artifacts.json list every {args.component} {args.version} asset with its sha256")
    return 0


if __name__ == "__main__":
    sys.exit(main())
