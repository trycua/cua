#!/usr/bin/env python3
"""The content key of a built macOS tier: what a rebuild must change before
it is worth publishing. A scheduled rebuild compares it with the
`ai.cua.image.content-key` annotation of the tag it would move, and
publishes only when they differ.

    content-key.py DOCTOR_DIR [--image-json libs/images/macos/image.json --tier full]

DOCTOR_DIR is a doctor-gate.sh output (report.json, build-info.json). The
key hashes the guest OS version and kernel, the image manifest, the cua-spacesd build, the claimed apps' and
tools' versions, the simulator runtimes the doctor recorded and the tier's
claims (so a new claim alone also counts as a change). Timestamps, host
facts and check timings are left out. Prints `sha256:<hex>`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys


def load(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, json.JSONDecodeError):
        return {}


def find(obj, key):
    """The first value under `key` anywhere in `obj` (reports nest it)."""
    if isinstance(obj, dict):
        if key in obj:
            return obj[key]
        for v in obj.values():
            got = find(v, key)
            if got is not None:
                return got
    elif isinstance(obj, list):
        for v in obj:
            got = find(v, key)
            if got is not None:
                return got
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("doctor_dir")
    ap.add_argument("--image-json")
    ap.add_argument("--tier")
    a = ap.parse_args()
    report = load(os.path.join(a.doctor_dir, "report.json"))
    build = load(os.path.join(a.doctor_dir, "build-info.json"))
    if not report:
        print(f"no report.json in {a.doctor_dir}", file=sys.stderr)
        return 2
    content = {
        "os": (report.get("environment") or {}).get("os"),
        "kernel": (report.get("fidelity") or {}).get("kernel"),
        "manifest_sha256": (report.get("image") or {}).get("manifest_sha256"),
        "spacesd": {k: build.get(k) for k in ("version", "git_sha", "git_dirty")},
        "apps": find(report, "app_versions") or {},
        "tools": find(report, "tool_versions") or {},
        "simulator_runtimes": sorted(find(report, "simulator_runtimes") or []),
    }
    if a.image_json:
        image = load(a.image_json)
        content["tier"] = a.tier
        content["claims"] = image.get("claims")
        content["tier_claims"] = (image.get("tiers") or {}).get(a.tier or "", {})
    blob = json.dumps(content, sort_keys=True, separators=(",", ":")).encode()
    print("sha256:" + hashlib.sha256(blob).hexdigest())
    return 0


if __name__ == "__main__":
    sys.exit(main())
