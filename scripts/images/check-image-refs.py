#!/usr/bin/env python3
"""Repo gate: every container image reference must come from a source of truth.

Scans every tracked text file (code, tests, samples, CI workflows, Dockerfiles,
docs and READMEs) for registry image references (``ghcr.io/...:tag``,
``docker.io/...``, ``public.ecr.aws/...``, Docker Hub ``trycua/...:tag``, any
``@sha256:`` digest ref) and requires each one to be known:

1. an entry of the shared catalog, ``libs/images/sandbox-images.json`` (the
   images the apps, SDKs and docs offer), accepted anywhere;
2. a benchmark pin: a tag or digest recorded in a catalog benchmark entry's
   ``lock`` file (``libs/images/bench/<id>/lock.json``, written by the bench
   image publisher), accepted anywhere;
3. an entry of ``scripts/images/image-refs-allowlist.json``: a legacy,
   internal or synthetic cua ref (``refs``, only in the listed paths), or a
   third-party / placeholder pattern (``thirdParty``) with a reason.

Every cua repository mentioned without a tag (``ghcr.io/trycua/<name>``) must
also be a catalog repository or listed in ``repos``.

Frozen legacy names (``legacy``: a name, the regex that finds it and the
historical paths it may still appear in) fail everywhere else, in any form: a
tagged ref, a bare repository, a path or prose. ``cua-desktop-linux``, the
pre-rename name of ``ghcr.io/trycua/linux``, is one: nothing new may use it. Allowlist entries that no
longer match anything fail the gate (stale), so the allowlist only shrinks as
references move to the catalog.

    check-image-refs.py [--root .] [--inventory FILE] [--list]

``--inventory`` writes a Markdown inventory grouped by image and by file kind.
Python 3 standard library only; no network.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from dataclasses import dataclass, field

CATALOG = "libs/images/sandbox-images.json"
ALLOWLIST = "scripts/images/image-refs-allowlist.json"

# A tag never ends in "." or "-" (so a sentence's full stop is not part of it).
TAG = r"[A-Za-z0-9_](?:[A-Za-z0-9_.-]{0,126}[A-Za-z0-9_])?"
HOST = (
    r"(?:ghcr\.io|docker\.io|index\.docker\.io|registry-1\.docker\.io|public\.ecr\.aws|quay\.io"
    r"|mcr\.microsoft\.com|gcr\.io|registry\.k8s\.io|registry\.example\.com"
    r"|[a-z0-9-]+\.azurecr\.io|[0-9]+\.dkr\.ecr\.[a-z0-9-]+\.amazonaws\.com|[a-z0-9-]+-docker\.pkg\.dev)"
)
PATH = r"[a-z0-9][a-z0-9._-]*(?:/[a-z0-9][a-z0-9._-]*)*"
REF_RE = re.compile(
    rf"(?<![\w./-])({HOST}/{PATH}|trycua/[a-z0-9][a-z0-9._-]*[a-z0-9])"
    rf"(?::({TAG})|@(sha256:[0-9a-f]{{64}}))(?![\w/:@<>{{$]|\.\w|-[<{{$])"
)
# A cua-owned repository named without a tag or digest.
CUA_REPO_RE = re.compile(
    r"(?<![\w./-])((?:ghcr\.io/trycua|docker\.io/trycua|public\.ecr\.aws/k5j5w0x5)/[a-z0-9][a-z0-9._-]*[a-z0-9])"
    r"(?![\w./:@-]|\.\w)"
)
CUA_OWNED = ("ghcr.io/trycua/", "docker.io/trycua/", "public.ecr.aws/k5j5w0x5/")
TEXT_SUFFIX_DENY = (
    ".png",
    ".jpg",
    ".jpeg",
    ".gif",
    ".webp",
    ".ico",
    ".icns",
    ".pdf",
    ".zip",
    ".gz",
    ".tar",
    ".woff",
    ".woff2",
    ".ttf",
    ".otf",
    ".mp4",
    ".mov",
    ".wasm",
    ".so",
    ".dylib",
)


@dataclass
class Hit:
    path: str
    line: int
    ref: str
    kind: str = "ref"  # "ref" (tagged or digest) or "repo" (cua repository, no tag)
    generated: bool = False  # in a generated file or a GENERATED:<name> region
    test: bool = False  # in a test file, or after a Rust file's first #[cfg(test)]
    verdict: str = ""
    rule: str = ""


@dataclass
class Result:
    hits: list[Hit] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)


def normalize(repo: str) -> str:
    if repo.startswith("trycua/"):
        return "docker.io/" + repo
    for alias in ("index.docker.io/", "registry-1.docker.io/"):
        if repo.startswith(alias):
            return "docker.io/" + repo[len(alias) :]
    return repo


def repo_of(ref: str) -> str:
    return ref.split("@", 1)[0] if "@" in ref else ref.rsplit(":", 1)[0]


REGION_RE = re.compile(r"GENERATED:([\w-]+):(start|end)")


def extract(text: str) -> list[tuple[int, str, str, bool]]:
    """(line, ref, kind, in a GENERATED region) for every image reference in ``text``."""
    out = []
    region = None
    for i, line in enumerate(text.splitlines(), 1):
        marker = REGION_RE.search(line)
        if marker and marker.group(2) == "start":
            region = marker.group(1)
        spans = []
        for m in REF_RE.finditer(line):
            repo, tag, digest = normalize(m.group(1)), m.group(2), m.group(3)
            out.append(
                (i, f"{repo}:{tag}" if tag else f"{repo}@{digest}", "ref", region is not None)
            )
            spans.append(m.span())
        for m in CUA_REPO_RE.finditer(line):
            if any(a <= m.start() < b for a, b in spans):
                continue
            out.append((i, m.group(1), "repo", region is not None))
        if marker and marker.group(2) == "end":
            region = None
    return out


def list_files(root: str) -> list[str]:
    try:
        got = subprocess.run(
            ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
            cwd=root,
            capture_output=True,
            check=True,
        )
        return sorted(p for p in got.stdout.decode().split("\0") if p)
    except (OSError, subprocess.CalledProcessError):
        found = []
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [d for d in dirnames if d not in {".git", "node_modules", "target"}]
            found += [os.path.relpath(os.path.join(dirpath, n), root) for n in filenames]
        return sorted(found)


def matches(path: str, globs: list[str]) -> bool:
    return any(fnmatch.fnmatchcase(path, g) for g in globs)


def load_json(root: str, rel: str) -> dict:
    with open(os.path.join(root, rel), encoding="utf-8") as fh:
        return json.load(fh)


def catalog_facts(root: str) -> tuple[set[str], set[str], dict[str, str]]:
    """(catalog refs, catalog repos, pinned ref -> lock file) from the catalog and its locks."""
    catalog = load_json(root, CATALOG)
    refs = {i["ref"] for i in catalog["images"]}
    repos = {repo_of(r) for r in refs}
    pins: dict[str, str] = {}
    for image in catalog["images"]:
        lock_path = image.get("lock")
        if not lock_path:
            continue
        lock = load_json(root, lock_path)
        repo = lock["repository"]
        for key in ("index", "disk_index"):
            entry = lock.get(key) or {}
            if entry.get("ref"):
                pins[entry["ref"]] = lock_path
            if entry.get("digest"):
                pins[f"{repo}@{entry['digest']}"] = lock_path
        for child in lock.get("children", []):
            for key in ("rootfs", "containerdisk"):
                if child.get(key):
                    pins[f"{repo}@{child[key]}"] = lock_path
    return refs, repos, pins


def load_allowlist(root: str, path: str | None) -> dict:
    if path is None:
        return load_json(root, ALLOWLIST)
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def is_generated(path: str, allow: dict) -> bool:
    return matches(path, allow.get("generated", []))


def run(root: str, allowlist: str | None = None) -> Result:
    allow = load_allowlist(root, allowlist)
    catalog_refs, catalog_repos, pins = catalog_facts(root)
    exclude = allow.get("exclude", [])
    third_party: dict[str, str] = allow.get("thirdParty", {})
    known: dict[str, dict] = allow.get("refs", {})
    repos: dict[str, str] = allow.get("repos", {})
    used_third: set[str] = set()
    used_known: dict[str, set[str]] = defaultdict(set)  # ref -> globs used
    used_repos: set[str] = set()
    legacy: dict[str, dict] = allow.get("legacy", {})
    legacy_re = {name: re.compile(e["pattern"]) for name, e in legacy.items()}
    used_legacy: dict[str, set[str]] = defaultdict(set)
    result = Result()

    for rel in list_files(root):
        if rel == ALLOWLIST or matches(rel, exclude) or rel.lower().endswith(TEXT_SUFFIX_DENY):
            continue
        try:
            with open(os.path.join(root, rel), encoding="utf-8") as fh:
                text = fh.read()
        except (OSError, UnicodeDecodeError):
            continue
        for name, rx in legacy_re.items():
            lines = [i for i, l in enumerate(text.splitlines(), 1) if rx.search(l)]
            if not lines:
                continue
            globs = [g for g in legacy[name].get("paths", []) if fnmatch.fnmatchcase(rel, g)]
            if globs:
                used_legacy[name].update(globs)
                continue
            for i in lines:
                result.errors.append(
                    f"{rel}:{i}: {name} is {legacy[name].get('reason', 'a frozen legacy name')}; "
                    f"new uses are not allowed (use {legacy[name].get('use', 'the canonical name')})"
                )
        generated_file = is_generated(rel, allow)
        test_file = bool(TEST_RE.search(rel))
        tests_from = None  # Rust unit tests live in the same file, after #[cfg(test)]
        if rel.endswith(".rs") and "#[cfg(test)]" in text:
            tests_from = text[: text.find("#[cfg(test)]")].count("\n") + 1
        for line, ref, kind, in_region in extract(text):
            hit = Hit(
                rel,
                line,
                ref,
                kind,
                generated_file or in_region,
                test_file or (tests_from is not None and line >= tests_from),
            )
            result.hits.append(hit)
            if kind == "repo":
                if ref in catalog_repos or any(repo_of(p) == ref for p in pins):
                    hit.verdict, hit.rule = "ok", "catalog repo"
                elif ref in repos:
                    hit.verdict, hit.rule = "ok", "allowlist repo"
                    used_repos.add(ref)
                else:
                    hit.verdict = "fail"
                    result.errors.append(
                        f"{rel}:{line}: unknown cua repository {ref}: add it to {CATALOG} "
                        f"or, with a reason, to repos in {ALLOWLIST}"
                    )
                continue
            if ref in catalog_refs:
                hit.verdict, hit.rule = "ok", "catalog"
                continue
            if ref in pins:
                hit.verdict, hit.rule = "ok", f"pin ({pins[ref]})"
                continue
            entry = known.get(ref)
            if entry is not None:
                globs = entry.get("paths", [])
                hit_globs = [g for g in globs if fnmatch.fnmatchcase(rel, g)]
                if hit_globs:
                    hit.verdict, hit.rule = "ok", "allowlist ref"
                    used_known[ref].update(hit_globs)
                    continue
                hit.verdict = "fail"
                result.errors.append(
                    f"{rel}:{line}: {ref} is {entry.get('reason', 'not a catalog image')}; "
                    f"it is only allowed in {', '.join(globs)}. Use a {CATALOG} image"
                )
                continue
            if not ref.startswith(CUA_OWNED):
                pattern = next((p for p in third_party if fnmatch.fnmatchcase(ref, p)), None)
                if pattern is not None:
                    hit.verdict, hit.rule = "ok", "third party"
                    used_third.add(pattern)
                    continue
                hit.verdict = "fail"
                result.errors.append(
                    f"{rel}:{line}: unknown third-party image {ref}: add a pattern with a reason "
                    f"to thirdParty in {ALLOWLIST}"
                )
                continue
            hit.verdict = "fail"
            if repo_of(ref) in catalog_repos or any(repo_of(p) == repo_of(ref) for p in pins):
                result.errors.append(
                    f"{rel}:{line}: {ref} is not in {CATALOG} (stale tag or unpinned digest); "
                    f"use a catalog ref or a lock pin"
                )
            else:
                result.errors.append(f"{rel}:{line}: unknown cua image {ref}: add it to {CATALOG}")

    files = list_files(root)
    for g in allow.get("generated", []):
        if not any(fnmatch.fnmatchcase(f, g) for f in files):
            result.errors.append(f"{ALLOWLIST}: generated {g} matches no file: remove it")
    for pattern in sorted(set(third_party) - used_third):
        result.errors.append(f"{ALLOWLIST}: thirdParty {pattern} matches nothing: remove it")
    for ref, entry in sorted(known.items()):
        if ref in catalog_refs or ref in pins:
            result.errors.append(
                f"{ALLOWLIST}: {ref} is a catalog image or pin: remove it from refs"
            )
        for g in entry.get("paths", []):
            if g not in used_known.get(ref, set()):
                result.errors.append(
                    f"{ALLOWLIST}: refs[{ref}] path {g} matches nothing: remove it"
                )
    for name, entry in sorted(legacy.items()):
        for g in entry.get("paths", []):
            if g not in used_legacy.get(name, set()):
                result.errors.append(
                    f"{ALLOWLIST}: legacy[{name}] path {g} matches nothing: remove it"
                )
    for repo in sorted(set(repos) - used_repos):
        result.errors.append(f"{ALLOWLIST}: repos {repo} matches nothing: remove it")
    return result


KINDS = [
    (
        "Source of truth (catalog, bench locks)",
        lambda p: p == CATALOG or fnmatch.fnmatchcase(p, "libs/images/bench/*/lock.json"),
    ),
    ("CI workflow", lambda p: p.startswith(".github/")),
    (
        "Dockerfile",
        lambda p: os.path.basename(p).startswith("Dockerfile") or p.endswith(".dockerfile"),
    ),
    ("Docs page (mdx)", lambda p: p.endswith(".mdx")),
    ("Markdown / README", lambda p: p.endswith(".md")),
    ("Rust", lambda p: p.endswith(".rs")),
    ("Python", lambda p: p.endswith(".py")),
    ("TypeScript / JS", lambda p: p.endswith((".ts", ".tsx", ".mjs", ".js", ".cjs"))),
    ("Swift", lambda p: p.endswith(".swift")),
    ("Kotlin", lambda p: p.endswith((".kt", ".kts"))),
    ("Shell", lambda p: p.endswith((".sh", ".bash"))),
    ("JSON data", lambda p: p.endswith(".json")),
    ("YAML / Terraform / other", lambda p: True),
]
TEST_RE = re.compile(
    r"(^|/)(tests?|__tests__|fixtures)/|(^|/)test[_-][^/]*$|[._-]test\.[a-z]+$|_tests?\.rs$|/tests\.rs$"
)


def kind_of(path: str, test: bool | None = None) -> str:
    base = next(name for name, pred in KINDS if pred(path))
    if test is None:
        test = bool(TEST_RE.search(path))
    return f"{base} (test)" if test else base


def inventory(result: Result, root: str) -> str:
    refs = [h for h in result.hits if h.kind == "ref"]
    hand = [h for h in refs if not h.generated]
    gen = [h for h in refs if h.generated]
    out = [
        "# Image reference inventory",
        "",
        f"Generated by `scripts/images/check-image-refs.py --inventory`. Scope: every tracked text file except "
        f"the `exclude` globs of `{ALLOWLIST}` (libs/fleet mirror, blog, changelogs, historical evidence and notes).",
        "",
        f"- Tagged or digest references: **{len(refs)}** ({len(hand)} hand-written, {len(gen)} in generated files "
        f"or regions)",
        f"- Hand-written outside tests: **{sum(1 for h in hand if not h.test)}**; in tests: "
        f"**{sum(1 for h in hand if h.test)}**",
        f"- Distinct references: **{len({h.ref for h in refs})}**",
        f"- Files with references: **{len({h.path for h in refs})}**",
        f"- Untagged cua repository mentions: **{sum(1 for h in result.hits if h.kind == 'repo')}**",
        f"- Gate failures: **{len(result.errors)}**",
        "",
        "## By file kind (hand-written)",
        "",
        "| Kind | References | Files |",
        "|---|---|---|",
    ]
    by_kind: dict[str, list[Hit]] = defaultdict(list)
    for h in hand:
        by_kind[kind_of(h.path, h.test)].append(h)
    for k in sorted(by_kind, key=lambda k: -len(by_kind[k])):
        out.append(f"| {k} | {len(by_kind[k])} | {len({h.path for h in by_kind[k]})} |")
    out += [
        "",
        "## By image",
        "",
        "| Image | Rule | Hand-written | Generated | Files (first 6) |",
        "|---|---|---|---|---|",
    ]
    by_ref: dict[str, list[Hit]] = defaultdict(list)
    for h in refs:
        by_ref[h.ref].append(h)
    for ref in sorted(by_ref, key=lambda r: (-len(by_ref[r]), r)):
        hs = by_ref[ref]
        files = sorted({h.path for h in hs})
        rule = hs[0].rule or hs[0].verdict
        n_gen = sum(1 for h in hs if h.generated)
        shown = ", ".join(f"`{f}`" for f in files[:6]) + (
            f" (+{len(files) - 6})" if len(files) > 6 else ""
        )
        out.append(f"| `{ref}` | {rule} | {len(hs) - n_gen} | {n_gen} | {shown} |")
    out += ["", "## By file", "", "| File | Kind | References |", "|---|---|---|"]
    by_file: dict[str, list[Hit]] = defaultdict(list)
    for h in refs:
        by_file[h.path].append(h)
    for f in sorted(by_file):
        n_gen = sum(1 for h in by_file[f] if h.generated)
        tag = (
            " (generated)"
            if n_gen == len(by_file[f])
            else (f" ({n_gen} generated)" if n_gen else "")
        )
        out.append(f"| `{f}`{tag} | {kind_of(f)} | {len(by_file[f])} |")
    if result.errors:
        out += ["", "## Gate failures", ""] + [f"- {e}" for e in result.errors]
    return "\n".join(out) + "\n"


def main(argv: list[str] | None = None) -> int:
    here = os.path.dirname(os.path.abspath(__file__))
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--root", default=os.path.abspath(os.path.join(here, "..", "..")))
    parser.add_argument("--inventory", help="write a Markdown inventory to this file")
    parser.add_argument("--list", action="store_true", help="print every reference and its verdict")
    parser.add_argument("--allowlist", help=f"allowlist file (default: <root>/{ALLOWLIST})")
    args = parser.parse_args(argv)
    result = run(args.root, args.allowlist)
    if args.list:
        for h in result.hits:
            print(f"{h.verdict}\t{h.rule}\t{h.ref}\t{h.path}:{h.line}")
    if args.inventory:
        with open(args.inventory, "w", encoding="utf-8") as fh:
            fh.write(inventory(result, args.root))
    for e in result.errors:
        print(f"error: {e}", file=sys.stderr)
    refs = sum(1 for h in result.hits if h.kind == "ref")
    if result.errors:
        print(f"image refs: {len(result.errors)} problem(s) in {refs} references", file=sys.stderr)
        return 1
    print(f"image refs: {refs} references, all from {CATALOG}, a bench lock or {ALLOWLIST}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
