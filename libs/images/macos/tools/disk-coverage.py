#!/usr/bin/env python3
"""Fails when a Lume image manifest's disk chunks do not tile the whole disk.

    disk-coverage.py MANIFEST_JSON   (or - for stdin)

A chunked `lume push` that loses chunks still writes a manifest, just
without them: ghcr.io/trycua/macos:26-20260927-9a3828d shipped 297 of its
300 disk chunks (parts 31, 63 and 64 missing), pulls reassembled those
ranges as zeros, and every VM cloned from it booted into recoveryOS with no
cua-spacesd. push.sh runs this on every push and release-tier.sh verify on
every pin, so an incomplete image is never published or promoted.
"""
import json
import sys

DISK = "application/vnd.trycua.lume.disk.v1"


def gaps(manifest):
    total = int(manifest.get("annotations", {}).get("org.trycua.lume.total-uncompressed-size", 0))
    spans, parts = [], []
    for layer in manifest.get("layers", []):
        if layer.get("mediaType") != DISK:
            continue
        a = layer.get("annotations", {})
        try:
            spans.append((int(a["org.trycua.lume.part.offset"]),
                          int(a["org.trycua.lume.content.uncompressed-size"])))
            parts.append(int(a["org.trycua.lume.part.number"]))
        except (KeyError, ValueError):
            return [f"disk layer {a.get('org.opencontainers.image.title', '?')} has no part offset/size"]
    if not spans:
        return ["no disk layers"]
    if total <= 0:
        return ["no org.trycua.lume.total-uncompressed-size annotation"]
    bad, end = [], 0
    for offset, size in sorted(spans):
        if offset > end:
            bad.append(f"bytes {end}..{offset} are in no layer")
        elif offset < end:
            bad.append(f"layers overlap at byte {offset}")
        end = max(end, offset + size)
    if end != total:
        bad.append(f"layers end at byte {end}, the disk has {total}")
    missing = sorted(set(range(max(parts) + 1)) - set(parts))
    if missing:
        bad.append(f"missing parts {missing}")
    return bad


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else "-"
    manifest = json.load(sys.stdin if src == "-" else open(src))
    bad = gaps(manifest)
    if bad:
        sys.exit("incomplete lume disk: " + "; ".join(bad))
    n = sum(1 for layer in manifest["layers"] if layer.get("mediaType") == DISK)
    print(f"disk complete: {n} chunks cover all "
          f"{manifest['annotations']['org.trycua.lume.total-uncompressed-size']} bytes")


if __name__ == "__main__":
    main()
