#!/usr/bin/env python3
"""Plumbing for doctor-attest in the image workflows (cd-image-linux.yml, cd-image-omarchy.yml).

    attest-doctor-reports.py lanes DIR
        One line per lane report under DIR (downloaded artifacts):
        ``<arch>\\t<lane>\\t<variant>\\t<report.json>\\t<lane.json>\\t<repo>@<digest>``,
        the digest being the pushed per-arch manifest the lane checked
        (rootfs for runc/runsc, containerdisk for qemu lanes).
    attest-doctor-reports.py descriptor-annotations ATTESTED.json [--variant V] [--format F]
        ai.cua.doctor.status / ai.cua.doctor.report for each platform's
        descriptor in the published index: ``--format cua`` (default) gives
        ``--descriptor-annotation VARIANT/ARCH:KEY=VALUE`` arguments for
        ``cua images publish`` (every variant unless --variant);
        ``--format imagetools`` gives ``--annotation`` arguments for
        ``docker buildx imagetools create`` (one --variant).

Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import sys


def lanes(directory: str) -> list[tuple[str, str, str, str, str, str]]:
    pushed: dict[str, dict] = {}
    for root, _dirs, files in os.walk(directory):
        if "pushed.json" in files:
            with open(os.path.join(root, "pushed.json")) as fh:
                record = json.load(fh)
            # Only per-arch push records (pushed-<arch>/pushed.json); other
            # pushed.json files (windows' {stamp, disk_digest, ...}) are not.
            if isinstance(record, dict) and record.get("arch") and record.get("repo"):
                pushed[record["arch"]] = record
    out = []
    for root, _dirs, files in sorted(os.walk(directory)):
        if "report.json" not in files or "lane.json" not in files:
            continue
        with open(os.path.join(root, "lane.json")) as fh:
            lane = json.load(fh)
        arch = lane.get("arch", "")
        variant = lane.get("variant", "rootfs")
        record = pushed.get(arch)
        if record is None:
            raise SystemExit(f"no push record for arch {arch!r} ({root})")
        name = lane["lane"] + ("-claim-secrets" if lane.get("claim_secrets") else "")
        subject = f"{record['repo']}@{record[variant]}"
        out.append((arch, name, variant, os.path.join(root, "report.json"), os.path.join(root, "lane.json"), subject))
    return out


def descriptor_annotations(attested: list[dict], variant: str | None, fmt: str = "cua") -> list[str]:
    groups: dict[tuple[str, str], list[dict]] = {}
    for entry in attested:
        if variant is None or entry["variant"] == variant:
            groups.setdefault((entry["variant"], entry["arch"]), []).append(entry)
    args = []
    for (var, arch), entries in sorted(groups.items()):
        status = "pass" if all(e["status"] == "pass" for e in entries) else "fail"
        refs = ",".join(e["report_ref"] for e in sorted(entries, key=lambda e: e["lane"]))
        if fmt == "imagetools":
            target = f"manifest-descriptor[linux/{arch}]"
            args += ["--annotation", f"{target}:ai.cua.doctor.status={status}",
                     "--annotation", f"{target}:ai.cua.doctor.report={refs}"]
        else:
            args += ["--descriptor-annotation", f"{var}/{arch}:ai.cua.doctor.status={status}",
                     "--descriptor-annotation", f"{var}/{arch}:ai.cua.doctor.report={refs}"]
    return args


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="cmd", required=True)
    l = sub.add_parser("lanes")
    l.add_argument("dir")
    d = sub.add_parser("descriptor-annotations")
    d.add_argument("attested")
    d.add_argument("--variant")
    d.add_argument("--format", choices=["cua", "imagetools"], default="cua")
    args = parser.parse_args(argv)
    if args.cmd == "lanes":
        for row in lanes(args.dir):
            print("\t".join(row))
    else:
        with open(args.attested) as fh:
            attested = json.load(fh)
        # One argument per line: `mapfile -t args < <(...)`.
        print("\n".join(descriptor_annotations(attested, args.variant, args.format)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
