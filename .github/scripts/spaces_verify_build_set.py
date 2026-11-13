#!/usr/bin/env python3
"""Check a main-built Cua Spaces build set before a tag run promotes it (CUA-2067).

    spaces_verify_build_set.py DIR --tag-version V --tag-sha SHA \
        [--repository trycua/cua] [--skip-attestations]

Fails (exit 1, one reason per line) unless DIR holds a complete set:
- build-manifest.json exists and its source SHA is the tag's commit;
- its version is the tag version;
- every listed file exists, matches its sha256, and no other file is present;
- every file (the manifest included) has a GitHub artifact attestation made by
  cd-cua-spaces-build.yml on refs/heads/main (`gh attestation verify`).
Prints the sha256 of build-manifest.json (the digest the staging smoke status
must carry) as the last line, "manifest-digest=<hex>", when the set passes.
Nothing here publishes or changes anything.
"""
import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

SIGNER = "{repo}/.github/workflows/cd-cua-spaces-build.yml"


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def check_manifest(root: Path, tag_version: str, tag_sha: str):
    """Return (problems, manifest_digest). Pure file checks, no network."""
    problems = []
    mpath = root / "build-manifest.json"
    if not mpath.is_file():
        return ["build-manifest.json is missing"], None
    manifest = json.loads(mpath.read_text())
    if manifest.get("schema") != 1:
        problems.append(f"unsupported manifest schema {manifest.get('schema')!r}")
    if manifest.get("source_sha") != tag_sha:
        problems.append(f"manifest source SHA {manifest.get('source_sha')} is not the tag commit {tag_sha}")
    if manifest.get("version") != tag_version:
        problems.append(f"manifest version {manifest.get('version')} is not the tag version {tag_version}")
    listed = {}
    for entry in manifest.get("files", []):
        listed[entry["file"]] = entry
        if entry.get("version") != tag_version:
            problems.append(f"{entry['file']}: version {entry.get('version')} is not {tag_version}")
        if entry.get("source_sha") != tag_sha:
            problems.append(f"{entry['file']}: source SHA is not the tag commit")
        path = root / entry["file"]
        if not path.is_file():
            problems.append(f"{entry['file']}: listed but missing")
        elif sha256(path) != entry["sha256"]:
            problems.append(f"{entry['file']}: sha256 differs from the manifest")
    if not listed:
        problems.append("manifest lists no files")
    present = {p.name for p in root.iterdir() if p.is_file()} - {"build-manifest.json"}
    for extra in sorted(present - set(listed)):
        problems.append(f"{extra}: present but not in the manifest")
    return problems, sha256(mpath)


def check_attestations(root: Path, repository: str):
    problems = []
    for path in sorted(p for p in root.iterdir() if p.is_file()):
        r = subprocess.run(
            [
                "gh", "attestation", "verify", str(path),
                "--repo", repository,
                "--signer-workflow", SIGNER.format(repo=repository),
                "--source-ref", "refs/heads/main",
            ],
            capture_output=True, text=True,
        )
        if r.returncode != 0:
            problems.append(f"{path.name}: attestation not verified: {(r.stderr or r.stdout).strip().splitlines()[-1:]}")
    return problems


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("dir")
    p.add_argument("--tag-version", required=True)
    p.add_argument("--tag-sha", required=True)
    p.add_argument("--repository", default="trycua/cua")
    p.add_argument("--skip-attestations", action="store_true", help="tests only")
    a = p.parse_args()
    root = Path(a.dir)
    problems, digest = check_manifest(root, a.tag_version, a.tag_sha)
    if not problems and not a.skip_attestations:
        problems += check_attestations(root, a.repository)
    for line in problems:
        print(line)
    if problems:
        return 1
    print(f"manifest-digest={digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
