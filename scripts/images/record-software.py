#!/usr/bin/env python3
"""Record what an image has installed, from a passing strict doctor report.

    scripts/images/record-software.py REPORT.json --image REF
        [--source TEXT] [--out-dir libs/images/software] [--allow-unstrict]

REPORT is `cua-spacesd doctor --strict --json` output of a booted guest of
REF (a canonical tag: `<repo>:<os-version>[-<tier>][-disk]`). The versions
come from the report's `fidelity` block (`app_versions`, `tool_versions`,
`simulator_runtimes`), which the doctor's `software` checks measured and
enforced. Writes `<out-dir>/<os>-<version>-<tier>.json`:

    {"image": REF without -disk, "tier": ..., "os": ..., "version": ...,
     "recorded_from": "<report date>, <runtime>/<arch>, <source>",
     "apps": {...}, "tools": {...}, "simulator_runtimes": [...]}

The docs generator (scripts/docs-generators/image-software.ts) renders these
into the per-image "what's installed" tables. Only a passing strict report
is recorded (--allow-unstrict relaxes the strict requirement, never the
pass), and an entry reported `unavailable` is refused.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from typing import Any

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DEFAULT_OUT = os.path.join(ROOT, "libs", "images", "software")
TAG_RE = re.compile(r"^(?P<version>[0-9][0-9.]*)(?:-(?P<tier>slim|xcode(?:-[0-9.]+)?))?(?:-disk)?$")


def fail(message: str) -> None:
    print(f"record-software: {message}", file=sys.stderr)
    sys.exit(1)


def parse_ref(ref: str) -> dict[str, str]:
    """os, version, tier and the primary (non-disk) ref of a canonical tag."""
    if "@" in ref or ":" not in ref.rsplit("/", 1)[-1]:
        fail(f"{ref}: want <repo>:<os-version>[-<tier>][-disk]")
    repo, tag = ref.rsplit(":", 1)
    m = TAG_RE.match(tag)
    if not m:
        fail(f"{ref}: tag {tag!r} is not <os-version>[-<tier>][-disk]")
    tier = m.group("tier") or "full"
    primary = f"{repo}:{m.group('version')}" + ("" if tier == "full" else f"-{tier}")
    return {
        "os": repo.rsplit("/", 1)[-1],
        "version": m.group("version"),
        "tier": tier,
        "image": primary,
    }


def inventory(report: dict[str, Any], ref: str, source: str, allow_unstrict: bool) -> dict[str, Any]:
    summary = report.get("summary", {})
    if summary.get("status") != "pass":
        fail(f"the report did not pass (status {summary.get('status')!r})")
    if not summary.get("strict") and not allow_unstrict:
        fail("the report is not from `doctor --strict` (pass --allow-unstrict to record it anyway)")
    fidelity = report.get("fidelity", {})
    apps = dict(sorted(fidelity.get("app_versions", {}).items()))
    tools = dict(sorted(fidelity.get("tool_versions", {}).items()))
    missing = [n for n, v in {**apps, **tools}.items() if not v or v == "unavailable"]
    if missing:
        fail(f"unavailable in the report: {', '.join(sorted(missing))}")
    env = report.get("environment", {})
    provenance = [
        str(report.get("started_at") or "undated")[:10],
        "/".join(x for x in (env.get("runtime", ""), env.get("arch", "")) if x) or "unknown runtime",
    ]
    if source:
        provenance.append(source)
    meta = parse_ref(ref)
    return {
        "image": meta["image"],
        "os": meta["os"],
        "version": meta["version"],
        "tier": meta["tier"],
        "recorded_from": ", ".join(provenance),
        "apps": apps,
        "tools": tools,
        "simulator_runtimes": sorted(fidelity.get("simulator_runtimes", [])),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("report", help="doctor --json report")
    parser.add_argument("--image", required=True, help="the canonical ref the report is of")
    parser.add_argument("--source", default="", help="digest or build revision, for provenance")
    parser.add_argument("--out-dir", default=DEFAULT_OUT)
    parser.add_argument("--allow-unstrict", action="store_true")
    args = parser.parse_args(argv)
    try:
        with open(args.report, encoding="utf-8") as fh:
            report = json.load(fh)
    except (OSError, json.JSONDecodeError) as error:
        fail(f"{args.report}: {error}")
    inv = inventory(report, args.image, args.source, args.allow_unstrict)
    os.makedirs(args.out_dir, exist_ok=True)
    out = os.path.join(args.out_dir, f"{inv['os']}-{inv['version']}-{inv['tier']}.json")
    with open(out, "w", encoding="utf-8") as fh:
        fh.write(json.dumps(inv, indent=2) + "\n")
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
