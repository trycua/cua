#!/usr/bin/env python3
"""Docs gate: every cua image a guide points at must have passed the doctor.

Scans docs, skills and READMEs (code blocks included) plus the SDK's image
defaults for cua image references (ghcr.io/trycua/..., public.ecr.aws/k5j5w0x5/...,
Docker Hub trycua/...:tag), resolves each to a digest (an index expands to its
per-platform children, the manifests runtimes actually pull), and requires an
image-doctor ledger entry with status ``pass`` for every one of them
(scripts/images/doctor_ledger.py; CI keeps the ledger on the
``image-doctor-ledger`` branch).

References published before the doctor existed are listed in
scripts/images/docs-image-refs-baseline.json with a reason. The baseline may
only shrink: a new entry fails the gate (``--baseline-base`` is the base
branch's copy), and an entry that now passes must be removed.

Historical plan and spec notes (``EXCLUDE``: docs/superpowers) are not
scanned: they are dated records, not guides.

A ref on a line with (or right after) ``<!-- cua-image-unverified -->``
(``{/* cua-image-unverified */}`` in MDX) is
skipped: an example that is not meant to be pulled as is.

    check-docs-image-refs.py [--root .] [--ledger DIR] [--baseline FILE]
        [--baseline-base FILE] [--resolved FILE] [--list]

``--resolved`` replaces registry lookups with a JSON map (tests):
``{"<ref>": {"digest": "sha256:...", "children": ["sha256:...", ...]}}``.
Python 3 standard library only (plus the ``crane`` CLI for real lookups).
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import subprocess
import sys
from typing import Any

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import doctor_ledger  # noqa: E402

# HTML comment for Markdown; MDX needs the JSX comment form.
UNVERIFIED = ("<!-- cua-image-unverified -->", "{/* cua-image-unverified */}")
SCAN = [
    "docs/**/*.md",
    "docs/**/*.mdx",
    "skills/**/*.md",
    "README.md",
    "libs/**/README.md",
    "libs/cua/crates/cua-image/src/canonical.rs",
    "libs/cua/crates/cua-image/src/resolve.rs",
]
SKIP_DIRS = {"node_modules", ".git", "target", ".venv", "dist", "build"}
# Historical notes, not published docs: dated agent plans and design specs
# record the refs of their day (check-image-refs.py excludes them too).
EXCLUDE = ["docs/superpowers/**"]
# A tag never ends in "." or "-" (so a sentence's full stop is not part of it).
TAG = r"[A-Za-z0-9_](?:[A-Za-z0-9_.-]{0,126}[A-Za-z0-9_])?"
REF_RE = re.compile(
    r"(?<![\w./-])("
    r"(?:ghcr\.io/trycua|public\.ecr\.aws/k5j5w0x5|docker\.io/trycua)/[a-z0-9][a-z0-9._/-]*[a-z0-9]"
    r"|trycua/[a-z0-9][a-z0-9._-]*[a-z0-9]"
    rf")(?::({TAG})|@(sha256:[0-9a-f]{{64}}))(?![\w/:@<>{{$-]|\.\w)"
)


def iter_files(root: str) -> list[str]:
    found = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames:
            rel = os.path.relpath(os.path.join(dirpath, name), root)
            if any(fnmatch.fnmatch(rel, pattern) for pattern in EXCLUDE):
                continue
            if any(fnmatch.fnmatch(rel, pattern) or fnmatch.fnmatch(rel, pattern.replace("**/", ""))
                   for pattern in SCAN):
                found.append(rel)
    return sorted(found)


def extract(text: str) -> list[tuple[int, str]]:
    """(line number, ref) for every image reference not marked unverified."""
    refs = []
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if any(m in line or (i > 0 and m in lines[i - 1]) for m in UNVERIFIED):
            continue
        for m in REF_RE.finditer(line):
            repo, tag, digest = m.group(1), m.group(2), m.group(3)
            if repo.startswith("trycua/"):
                # Bare Docker Hub names need a tag, else they are GitHub repos.
                if not tag:
                    continue
                repo = "docker.io/" + repo
            refs.append((i + 1, f"{repo}:{tag}" if tag else f"{repo}@{digest}"))
    return refs


def collect(root: str) -> dict[str, list[str]]:
    """ref -> ["file:line", ...]."""
    out: dict[str, list[str]] = {}
    for rel in iter_files(root):
        try:
            with open(os.path.join(root, rel), encoding="utf-8") as fh:
                text = fh.read()
        except (OSError, UnicodeDecodeError):
            continue
        for line, ref in extract(text):
            out.setdefault(ref, []).append(f"{rel}:{line}")
    return out


def repo_of(ref: str) -> str:
    return ref.split("@", 1)[0] if "@" in ref else ref.rsplit(":", 1)[0]


def resolve_crane(ref: str) -> dict[str, Any]:
    digest = subprocess.run(["crane", "digest", ref], capture_output=True, text=True, timeout=60)
    if digest.returncode != 0:
        return {"error": digest.stderr.strip() or "crane digest failed"}
    manifest = subprocess.run(["crane", "manifest", ref], capture_output=True, text=True, timeout=60)
    children: list[str] = []
    if manifest.returncode == 0:
        try:
            body = json.loads(manifest.stdout)
            for m in body.get("manifests", []):
                platform = m.get("platform", {})
                # Skip attestation manifests (buildx) and referrer artifacts.
                if platform.get("os") == "unknown" or m.get("artifactType"):
                    continue
                children.append(m["digest"])
        except json.JSONDecodeError:
            pass
    return {"digest": digest.stdout.strip(), "children": children}


def check(refs: dict[str, list[str]], ledger: str | None, baseline: dict[str, str],
          resolver) -> tuple[list[dict[str, Any]], list[str]]:
    rows, errors = [], []
    for ref in sorted(refs):
        resolved = resolver(ref)
        row = {"ref": ref, "where": refs[ref][:3], "digest": resolved.get("digest", ""), "status": "", "detail": ""}
        if "error" in resolved:
            row.update(status="unresolved", detail=resolved["error"])
        else:
            digests = resolved.get("children") or [resolved["digest"]]
            verdicts = []
            for d in digests:
                if ledger:
                    verdict, detail = doctor_ledger.status(ledger, repo_of(ref), d, [])
                else:
                    verdict, detail = "missing", "no ledger"
                verdicts.append((d, verdict, detail))
            bad = [v for v in verdicts if v[1] != "pass"]
            row["status"] = "pass" if not bad else bad[0][1]
            row["detail"] = "; ".join(f"{d[:19]}: {v} ({x})" for d, v, x in (bad or verdicts))[:300]
        in_baseline = ref in baseline
        if row["status"] == "pass" and in_baseline:
            errors.append(f"{ref} now passes the doctor: remove it from the baseline")
            row["gate"] = "stale baseline"
        elif row["status"] != "pass" and not in_baseline:
            errors.append(f"{ref} ({', '.join(refs[ref][:2])}): {row['status']}: {row['detail']}")
            row["gate"] = "FAIL"
        else:
            row["gate"] = "baseline" if in_baseline else "ok"
        rows.append(row)
    return rows, errors


def table(rows: list[dict[str, Any]]) -> str:
    out = ["### Docs image refs", "", "| gate | ref | digest | doctor | detail | used in |", "|---|---|---|---|---|---|"]
    for r in rows:
        out.append(
            f"| {r['gate']} | `{r['ref']}` | `{r['digest'][:19]}` | {r['status']} | "
            f"{r['detail'].replace('|', '/')} | {', '.join(r['where'])} |"
        )
    return "\n".join(out) + "\n"


def main(argv: list[str] | None = None) -> int:
    here = os.path.dirname(os.path.abspath(__file__))
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", default=os.path.abspath(os.path.join(here, "..", "..")))
    parser.add_argument("--ledger", help="checked-out image-doctor-ledger tree")
    parser.add_argument("--baseline", default=os.path.join(here, "docs-image-refs-baseline.json"))
    parser.add_argument("--baseline-base", help="the base branch's baseline (shrink-only)")
    parser.add_argument("--resolved", help="JSON map replacing registry lookups")
    parser.add_argument("--list", action="store_true", help="print the refs found and exit")
    parser.add_argument("--write-baseline", action="store_true",
                        help="rewrite the baseline to the refs that do not pass (maintainers)")
    args = parser.parse_args(argv)

    refs = collect(args.root)
    if args.list:
        for ref, where in sorted(refs.items()):
            print(f"{ref}\t{', '.join(where[:3])}")
        return 0
    with open(args.baseline, encoding="utf-8") as fh:
        baseline: dict[str, str] = json.load(fh).get("refs", {})
    problems: list[str] = []
    if args.baseline_base and os.path.exists(args.baseline_base):
        with open(args.baseline_base, encoding="utf-8") as fh:
            base = json.load(fh).get("refs", {})
        grown = sorted(set(baseline) - set(base))
        if grown:
            problems.append("the baseline may only shrink; new entries: " + ", ".join(grown))
    for ref in sorted(set(baseline) - set(refs)):
        problems.append(f"{ref} is in the baseline but no longer referenced: remove it")
    if args.resolved:
        with open(args.resolved, encoding="utf-8") as fh:
            fixed = json.load(fh)
        resolver = lambda ref: fixed.get(ref, {"error": "not in --resolved"})  # noqa: E731
    else:
        resolver = resolve_crane
    rows, errors = check(refs, args.ledger, baseline, resolver)
    if args.write_baseline:
        keep = {r["ref"]: baseline.get(r["ref"], "published before the image doctor gate")
                for r in rows if r["status"] != "pass"}
        with open(args.baseline, "w", encoding="utf-8") as fh:
            json.dump({"_doc": "Image refs in docs that have no passing image-doctor ledger entry yet. "
                               "Shrink-only: fix or remove, never add.", "refs": keep}, fh, indent=2, sort_keys=True)
            fh.write("\n")
        return 0
    sys.stdout.write(table(rows))
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(table(rows))
    problems += errors
    for p in problems:
        print(f"error: {p}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
