#!/usr/bin/env python3
"""Write build-manifest.json for a Cua Spaces shadow build (CUA-2066).

Lists every file in DIR (except the manifest itself) with its sha256, size,
platform and kind, plus the version, build number, source SHA and input tree
hash the build ran from. Phase 2 of CUA-1991 (the promote path) reads it.

    spaces_build_manifest.py DIR --version V --build-number N --source-sha SHA \
        --tree-hash HASH --repository trycua/cua --run-id ID
"""
import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

# (pattern on the file name, platform, kind). First match wins.
RULES = [
    (r"-darwin-universal\.dmg$", "darwin-universal", "dmg"),
    (r"-darwin-universal\.pkg$", "darwin-universal", "pkg"),
    (r"^Cua-Spaces-Setup-.*-(x64|arm64)\.exe(\.blockmap)?$", None, "windows-installer"),
    (r"^Cua-Spaces-.*-(x86_64|x64)\.AppImage$", "linux-x64", "appimage"),
    (r"^Cua-Spaces-.*-arm64\.AppImage$", "linux-arm64", "appimage"),
    (r"_amd64\.deb$", "linux-x64", "deb"),
    (r"_arm64\.deb$", "linux-arm64", "deb"),
    (r"^(latest|beta)\.yml$", "windows", "feed"),
    (r"^(latest|beta)-linux\.yml$", "linux-x64", "feed"),
    (r"^(latest|beta)-linux-arm64\.yml$", "linux-arm64", "feed"),
    (r"^signing-.*\.txt$", "all", "signing-note"),
]


def classify(name: str):
    for pattern, platform, kind in RULES:
        m = re.search(pattern, name)
        if not m:
            continue
        if kind == "windows-installer":
            platform = "windows-" + m.group(1)
            if name.endswith(".blockmap"):
                kind = "windows-installer-blockmap"
            else:
                kind = "windows-installer"
        return platform, kind
    return "unknown", "other"


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("dir")
    p.add_argument("--version", required=True)
    p.add_argument("--build-number", required=True)
    p.add_argument("--source-sha", required=True)
    p.add_argument("--tree-hash", required=True)
    p.add_argument("--repository", required=True)
    p.add_argument("--run-id", required=True)
    a = p.parse_args()

    root = Path(a.dir)
    files = []
    for path in sorted(root.iterdir()):
        if not path.is_file() or path.name == "build-manifest.json":
            continue
        platform, kind = classify(path.name)
        files.append(
            {
                "file": path.name,
                "sha256": sha256(path),
                "size": path.stat().st_size,
                "platform": platform,
                "kind": kind,
                "version": a.version,
                "build_number": a.build_number,
                "source_sha": a.source_sha,
                "input_tree_hash": a.tree_hash,
            }
        )
    unknown = [f["file"] for f in files if f["platform"] == "unknown"]
    if unknown:
        print(f"::error::files the manifest cannot classify: {unknown}", file=sys.stderr)
        return 1
    if not files:
        print("::error::no files to list", file=sys.stderr)
        return 1
    manifest = {
        "schema": 1,
        "repository": a.repository,
        "run_id": a.run_id,
        "version": a.version,
        "build_number": a.build_number,
        "source_sha": a.source_sha,
        "input_tree_hash": a.tree_hash,
        "files": files,
    }
    (root / "build-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"build-manifest.json: {len(files)} files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
