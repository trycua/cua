#!/usr/bin/env python3
"""Build `release-artifacts.json`, the manifest install.sh / install.ps1 read.

The release workflows call this after collecting their artifacts:

    release_manifest.py --component cli --version 0.3.0 --dir release/ \
        --base-url https://github.com/trycua/cua/releases/download/cua-sdk-v0.3.0 \
        --merge previous.json --out release/release-artifacts.json

Artifact names it recognises (anything else in --dir is ignored):

    cli: cua-cli-<version>-<platform>.tar.gz   (macOS, Linux)
         cua-cli-<version>-<platform>.zip      (Windows)
    app: cua-spaces-<version>-darwin-universal.dmg / .pkg
         cua-spaces-<version>-linux-<arch>.AppImage / .deb
         cua-spaces-<version>-windows-<arch>.msi / -setup.exe

<platform> is darwin-arm64, darwin-x64, linux-x64, linux-arm64, windows-x64 or
windows-arm64; a darwin-universal app is listed under both macOS platforms.
Optional signatures next to an artifact: `<name>.minisig` (minisign) and
`<name>.sigstore.json` (cosign bundle).

Format contract (install.sh parses it with grep/sed, no jq): the file is JSON
with `"schema": 1`, and every artifact object sits on ONE line with string
values that contain no double quotes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

SCHEMA = 1
PLATFORMS = ("darwin-arm64", "darwin-x64", "linux-x64", "linux-arm64", "windows-x64", "windows-arm64")
VERSION = r"(?P<version>\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)"

PATTERNS = [
    ("cli", "tar.gz", re.compile(rf"^cua-cli-{VERSION}-(?P<platform>(?:darwin|linux)-(?:x64|arm64))\.tar\.gz$")),
    ("cli", "zip", re.compile(rf"^cua-cli-{VERSION}-(?P<platform>windows-(?:x64|arm64))\.zip$")),
    ("app", "dmg", re.compile(rf"^cua-spaces-{VERSION}-(?P<platform>darwin-(?:universal|x64|arm64))\.dmg$")),
    ("app", "pkg", re.compile(rf"^cua-spaces-{VERSION}-(?P<platform>darwin-(?:universal|x64|arm64))\.pkg$")),
    ("app", "appimage", re.compile(rf"^cua-spaces-{VERSION}-(?P<platform>linux-(?:x64|arm64))\.AppImage$")),
    ("app", "deb", re.compile(rf"^cua-spaces-{VERSION}-(?P<platform>linux-(?:x64|arm64))\.deb$")),
    ("app", "msi", re.compile(rf"^cua-spaces-{VERSION}-(?P<platform>windows-(?:x64|arm64))\.msi$")),
    ("app", "nsis", re.compile(rf"^cua-spaces-{VERSION}-(?P<platform>windows-(?:x64|arm64))-setup\.exe$")),
]

SAFE = re.compile(r'^[^"\\\x00-\x1f]*$')


def classify(name: str):
    for component, kind, pattern in PATTERNS:
        match = pattern.match(name)
        if match:
            return component, kind, match.group("platform"), match.group("version")
    return None


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def url_for(base_url: str | None, name: str) -> str:
    return f"{base_url.rstrip('/')}/{name}" if base_url else name


def scan(component: str, version: str, directory: Path, base_url: str | None) -> list[dict]:
    entries = []
    for path in sorted(directory.iterdir()):
        if not path.is_file():
            continue
        found = classify(path.name)
        if not found or found[0] != component:
            continue
        _, kind, platform, file_version = found
        if file_version != version:
            raise SystemExit(f"{path.name}: version {file_version} does not match --version {version}")
        platforms = ["darwin-arm64", "darwin-x64"] if platform == "darwin-universal" else [platform]
        common = {
            "name": path.name,
            "url": url_for(base_url, path.name),
            "sha256": sha256(path),
            "size": path.stat().st_size,
        }
        for suffix, key in ((".minisig", "minisig"), (".sigstore.json", "cosign_bundle")):
            if (directory / f"{path.name}{suffix}").is_file():
                common[key] = url_for(base_url, f"{path.name}{suffix}")
        for p in platforms:
            entries.append({"component": component, "platform": p, "kind": kind, "version": version, **common})
    if not entries:
        raise SystemExit(f"no {component} artifacts for {version} in {directory}")
    return entries


def render(manifest: dict) -> str:
    """JSON with one artifact object per line (the install.sh contract)."""
    for artifact in manifest["artifacts"]:
        for key, value in artifact.items():
            if isinstance(value, str) and not SAFE.match(value):
                raise SystemExit(f"unsafe characters in {key}: {value!r}")
    head = {k: v for k, v in manifest.items() if k != "artifacts"}
    lines = ["{"]
    for key, value in head.items():
        lines.append(f"  {json.dumps(key)}: {json.dumps(value, separators=(', ', ': '))},")
    lines.append('  "artifacts": [')
    rows = [json.dumps(a, separators=(",", ":")) for a in manifest["artifacts"]]
    lines.extend(f"    {row}{',' if i < len(rows) - 1 else ''}" for i, row in enumerate(rows))
    lines.append("  ]")
    lines.append("}")
    return "\n".join(lines) + "\n"


def build(args: argparse.Namespace) -> dict:
    previous: dict = {}
    if args.merge and Path(args.merge).is_file():
        previous = json.loads(Path(args.merge).read_text())
        if previous.get("schema") != SCHEMA:
            raise SystemExit(f"{args.merge}: unsupported schema {previous.get('schema')!r}")
    ours = scan(args.component, args.version, Path(args.dir), args.base_url)
    kept = [a for a in previous.get("artifacts", []) if a.get("component") != args.component]
    versions = dict(previous.get("versions", {}))
    versions[args.component] = args.version
    artifacts = sorted(kept + ours, key=lambda a: (a["component"], a["platform"], a["kind"]))
    return {"schema": SCHEMA, "repo": args.repo, "versions": versions, "artifacts": artifacts}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--component", choices=("cli", "app"), required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--dir", required=True, help="directory holding the release artifacts")
    parser.add_argument("--base-url", help="download URL prefix (default: names relative to the manifest)")
    parser.add_argument("--merge", help="existing manifest; its other components are kept")
    parser.add_argument("--repo", default="trycua/cua")
    parser.add_argument("--out", help="output path (default stdout)")
    args = parser.parse_args(argv)
    text = render(build(args))
    if args.out:
        Path(args.out).write_text(text)
    else:
        sys.stdout.write(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
