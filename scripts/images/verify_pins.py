#!/usr/bin/env python3
"""One child of a published index against what was pushed and doctored
(scripts/images/verify-pins.sh, the verify step of `cua images release`).

    verify_pins.py child INDEX.json --arch ARCH --variant rootfs|containerdisk
        --want sha256:... [--attested ATTESTED.json]

Prints the child digest. Fails when the arch has no single child, the child
is not the pushed digest, or its ``ai.cua.doctor.status`` descriptor
annotation is anything but ``pass``. A child without the annotation passes
only when no doctor lane ran for that variant and arch (ATTESTED.json, from
doctor-attest): hosted arm64 runners have no KVM, so an arm64 disk is boot
smoked, not doctored, and publish has no verdict to annotate.

Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import json
import sys


def check_child(index: dict, arch: str, variant: str, want: str, attested: list[dict] | None) -> str:
    children = [m for m in index.get("manifests", []) if m.get("platform", {}).get("architecture") == arch]
    if len(children) != 1:
        raise ValueError(f"{arch}: expected one child, found {len(children)}")
    child = children[0]
    if child["digest"] != want:
        raise ValueError(f"{arch} {variant} child {child['digest']} is not the pushed {want}")
    status = (child.get("annotations") or {}).get("ai.cua.doctor.status")
    if status == "pass":
        return child["digest"]
    if status is not None:
        raise ValueError(f"{arch} {variant} child is annotated ai.cua.doctor.status={status}")
    lanes = [e for e in (attested or []) if e.get("arch") == arch and e.get("variant") == variant]
    if lanes:
        raise ValueError(
            f"{arch} {variant} child has no doctor annotation although lanes ran: "
            + ", ".join(f"{e.get('lane')}={e.get('status')}" for e in lanes)
        )
    return child["digest"]


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("child")
    c.add_argument("index")
    c.add_argument("--arch", required=True)
    c.add_argument("--variant", required=True, choices=["rootfs", "containerdisk"])
    c.add_argument("--want", required=True)
    c.add_argument("--attested")
    a = p.parse_args(argv)
    with open(a.index) as fh:
        index = json.load(fh)
    attested = None
    if a.attested:
        with open(a.attested) as fh:
            attested = json.load(fh)
    try:
        print(check_child(index, a.arch, a.variant, a.want, attested))
    except ValueError as e:
        print(f"verify: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
