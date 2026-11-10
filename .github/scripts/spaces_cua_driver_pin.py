#!/usr/bin/env python3
"""Keep Cua Spaces on the latest Cua Driver release.

apps/cua-spaces-macos/cua-driver.release.json pins the Cua Driver the Mac app
bundles. Rule: Cua Spaces always bundles the latest Cua Driver.

  spaces_cua_driver_pin.py bump  [--tag cua-driver-rs-vX.Y.Z]
      Rewrite the pin for that release (default: the latest stable one).
      Size and SHA-256 come from our own download of the asset and must match
      the release's SHA256SUMS. Never moves the pin backwards. Writes
      `changed=true|false` and `version=X.Y.Z` to GITHUB_OUTPUT.
  spaces_cua_driver_pin.py check [--grace-hours 24]
      Fail when the pin is older than a newer stable release by more than the
      grace period. The safety net for the bump workflow.

Stable means a published, non-draft release tagged cua-driver-rs-vX.Y.Z.
This repo publishes every driver release with the prerelease flag set (see
cd-rust-cua-driver.yml), so the flag does not separate stable from nightly;
the tag does (nightlies carry a -nightly.* suffix).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
from datetime import datetime, timezone
from pathlib import Path

REPO = os.environ.get("GITHUB_REPOSITORY", "trycua/cua")
ROOT = Path(__file__).resolve().parents[2]
PIN = Path(os.environ.get("CUA_DRIVER_PIN_FILE", ROOT / "apps/cua-spaces-macos/cua-driver.release.json"))
TAG_RE = re.compile(r"^cua-driver-rs-v(\d+)\.(\d+)\.(\d+)$")


def gh(*args: str) -> str:
    return subprocess.run(["gh", *args], check=True, capture_output=True, text=True).stdout


def version_of(tag: str) -> tuple[int, int, int]:
    m = TAG_RE.match(tag)
    if not m:
        raise SystemExit(f"{tag} is not a stable cua-driver-rs release tag")
    return tuple(int(p) for p in m.groups())  # type: ignore[return-value]


def when(r: dict) -> datetime:
    return datetime.fromisoformat(r["published_at"].replace("Z", "+00:00"))


def stable_releases() -> list[dict]:
    pages = json.loads(gh("api", "--paginate", "--slurp", f"repos/{REPO}/releases?per_page=100"))
    rels = [r for page in pages for r in page]
    ok = [r for r in rels if not r["draft"] and r.get("published_at") and TAG_RE.match(r["tag_name"])]
    return sorted(ok, key=lambda r: version_of(r["tag_name"]), reverse=True)


def emit(**kv: str) -> None:
    lines = "".join(f"{k}={v}\n" for k, v in kv.items())
    out = os.environ.get("GITHUB_OUTPUT")
    if out:
        with open(out, "a") as f:
            f.write(lines)
    print(lines, end="")


def bump(tag: str | None) -> int:
    pin = json.loads(PIN.read_text())
    if tag is None:
        rels = stable_releases()
        if not rels:
            raise SystemExit("no stable cua-driver-rs release found")
        tag = rels[0]["tag_name"]
    new = version_of(tag)
    if new < version_of(pin["tag"]):
        print(f"{tag} is older than the pin {pin['tag']}; leaving the pin alone")
        emit(changed="false", version=pin["version"])
        return 0
    rel = json.loads(gh("api", f"repos/{REPO}/releases/tags/{tag}"))
    if rel["draft"] or not rel.get("published_at"):
        raise SystemExit(f"{tag} is not published")
    version = ".".join(map(str, new))
    asset = f"cua-driver-rs-{version}-darwin-universal-binary.tar.gz"
    names = {a["name"] for a in rel["assets"]}
    for need in (asset, "SHA256SUMS"):
        if need not in names:
            raise SystemExit(f"{tag} has no {need} asset")
    with tempfile.TemporaryDirectory() as tmp:
        gh("release", "download", tag, "--repo", REPO, "-p", asset, "-p", "SHA256SUMS", "-D", tmp)
        data = Path(tmp, asset).read_bytes()
        sums = {
            line.split()[1].lstrip("*"): line.split()[0]
            for line in Path(tmp, "SHA256SUMS").read_text().splitlines()
            if line.strip()
        }
        sha = hashlib.sha256(data).hexdigest()
        if sums.get(asset) != sha:
            raise SystemExit(f"{asset}: downloaded SHA-256 {sha} != SHA256SUMS {sums.get(asset)}")
        with tarfile.open(Path(tmp, asset)) as tar:
            members = {Path(m.name).name for m in tar.getmembers() if m.isfile()}
        missing = [f for f in pin["files"] if f not in members]
        if missing:
            raise SystemExit(f"{asset} lacks {missing}; the pin's files list needs a human")
    pin.update(version=version, tag=tag, asset=asset, bytes=len(data), sha256=sha)
    files = ", ".join(json.dumps(f) for f in pin.pop("files"))
    body = json.dumps(pin, indent=2)[:-2]  # drop the closing brace
    text = f'{body},\n  "files": [{files}]\n}}\n'
    changed = PIN.read_text() != text
    PIN.write_text(text)
    print(f"pin -> {version} ({len(data)} bytes, sha256 {sha})")
    emit(changed=str(changed).lower(), version=version)
    return 0


def check(grace_hours: float) -> int:
    pin = json.loads(PIN.read_text())
    rels = stable_releases()
    if not rels:
        raise SystemExit("no stable cua-driver-rs release found")
    latest = rels[0]
    if version_of(pin["tag"]) >= version_of(latest["tag_name"]):
        print(f"ok: Cua Spaces bundles {pin['version']}, the latest stable release")
        return 0
    # The pin is behind from the moment the first newer release was published.
    behind_since = min(when(r) for r in rels if version_of(r["tag_name"]) > version_of(pin["tag"]))
    waited = (datetime.now(timezone.utc) - behind_since).total_seconds() / 3600
    msg = (f"Cua Spaces bundles Cua Driver {pin['version']} but {latest['tag_name']} is out; "
           f"the pin has been behind for {waited:.1f}h")
    if waited > grace_hours:
        print(f"::error::{msg}. Run the 'Release: Bundle Cua Driver in Cua Spaces' workflow, "
              "or bump apps/cua-spaces-macos/cua-driver.release.json by hand.")
        return 1
    print(f"ok (within the {grace_hours:g}h grace period): {msg}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("bump").add_argument("--tag")
    sub.add_parser("check").add_argument("--grace-hours", type=float, default=24)
    a = ap.parse_args()
    return bump(a.tag) if a.cmd == "bump" else check(a.grace_hours)


if __name__ == "__main__":
    sys.exit(main())
