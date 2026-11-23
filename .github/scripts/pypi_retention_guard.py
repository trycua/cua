"""Refuse a PyPI delete list that breaks the retention rule (docs/pypi-retention.md).

Deleting a PyPI release is permanent: the version can never be uploaded again
and every pin of it breaks. On 12 Nov 2026 a prune deleted cua-driver 0.45.2,
which a partner pinned, and cua-driver 0.26.0 / 0.28.2, which published
cua-sandbox releases require. This guard runs before any deletion and exits
non-zero if any version on the list:

  1. is not on PyPI (typo, or already gone);
  2. is the newest final patch of its minor (kept forever, safe to pin);
  3. is younger than 90 days (first upload);
  4. is not yanked yet (every file of the release);
  5. was not announced at least 30 days ago in release notes that name it;
  6. is the last version satisfying a requirement in requires_dist of any
     release of any of our PyPI projects;
  7. is the last version satisfying a pin in a scanned repository checkout
     (requirements files, pyproject.toml, uv.lock, Dockerfiles, docs, workflows).

Delete list format: one version per line; `#` comments. The announcement is
given as header lines (or --announced-on / --announcement-url):

    # announced: 2026-12-01 https://github.com/trycua/cua/releases/tag/cua-driver-rs-v0.48.0
    0.12.3
    0.12.4

Usage:
    python3 .github/scripts/pypi_retention_guard.py --project cua-driver \
        --delete-list pypi-delete-list.txt [--repo-root PATH ...] [--today YYYY-MM-DD]

Exit 0: every version passes. Exit 1: violations (printed, one per line).
Exit 2: usage or network error (fails closed).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Callable, Iterable

try:
    from packaging.requirements import InvalidRequirement, Requirement
    from packaging.specifiers import InvalidSpecifier, SpecifierSet
    from packaging.version import InvalidVersion, Version
except ImportError:  # pragma: no cover - pip always vendors it
    from pip._vendor.packaging.requirements import InvalidRequirement, Requirement
    from pip._vendor.packaging.specifiers import InvalidSpecifier, SpecifierSet
    from pip._vendor.packaging.version import InvalidVersion, Version

MIN_AGE_DAYS = 90
ANNOUNCE_DAYS = 30

# Every PyPI project trycua publishes (names not on PyPI yet are skipped). A
# release of any of them may require another one in requires_dist. Add new
# projects here; --also-project covers a one-off.
OUR_PROJECTS = (
    "cua", "cua-cli", "cua-driver", "cua-sdk", "cua-agent", "cua-core", "cua-computer",
    "cua-computer-server", "cua-mcp-server", "cua-auto", "cua-fleet",
    "cua-sandbox", "cua-sandbox-apps", "cua-som", "cua-bench", "cua-bench-ui",
    "cua-bench-rl", "cua-bench-s1", "cua-e2b", "cua-s1",
)

Fetch = Callable[[str], "dict | None"]


def pypi_fetch(path: str) -> dict | None:
    """GET https://pypi.org/pypi/<path>/json; None on 404."""
    try:
        with urllib.request.urlopen(f"https://pypi.org/pypi/{path}/json", timeout=30) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def norm(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def parse_version(v: str) -> Version | None:
    try:
        return Version(v)
    except InvalidVersion:
        return None


def latest_patch_per_minor(versions: Iterable[str]) -> set[str]:
    """The newest final release of each (major, minor)."""
    best: dict[tuple[int, int], Version] = {}
    for v in versions:
        pv = parse_version(v)
        if pv is None or pv.is_prerelease or pv.is_devrelease:
            continue
        key = (pv.major, pv.minor)
        if key not in best or pv > best[key]:
            best[key] = pv
    return {str(v) for v in best.values()}


def first_upload(files: list[dict]) -> dt.datetime | None:
    times = [f.get("upload_time_iso_8601") or f.get("upload_time") for f in files]
    times = [t for t in times if t]
    if not times:
        return None
    return min(dt.datetime.fromisoformat(t.replace("Z", "+00:00")) for t in times)


def satisfying(spec: SpecifierSet, versions: Iterable[str]) -> set[str]:
    out = set()
    for v in versions:
        pv = parse_version(v)
        if pv is not None and spec.contains(pv, prereleases=True):
            out.add(v)
    return out


def orphaned(spec: SpecifierSet, available: set[str], deleting: set[str]) -> set[str]:
    """Versions on the list that are the last ones satisfying `spec`, or an
    empty set if the requirement still resolves (or already did not)."""
    sat = satisfying(spec, available)
    if sat and sat <= deleting:
        return sat
    return set()


# --- requires_dist of our published releases ---------------------------------


def published_requirements(project: str, fetch: Fetch, projects: Iterable[str]) -> list[tuple[str, str, SpecifierSet]]:
    """(release label, raw requirement, specifier) for every requirement on
    `project` in any release of `projects`."""
    target = norm(project)

    def releases_of(p: str) -> list[tuple[str, str]]:
        doc = fetch(p)
        if not doc:
            return []
        return [(p, v) for v, files in doc.get("releases", {}).items() if files]

    with ThreadPoolExecutor(8) as ex:
        pairs = [pv for chunk in ex.map(releases_of, projects) for pv in chunk]
        docs = list(ex.map(lambda pv: (pv, fetch(f"{pv[0]}/{pv[1]}")), pairs))

    found = []
    for (p, v), doc in docs:
        for raw in ((doc or {}).get("info") or {}).get("requires_dist") or []:
            try:
                req = Requirement(raw)
            except InvalidRequirement:
                continue
            if norm(req.name) == target:
                found.append((f"{p}=={v}", raw, req.specifier))
    return found


# --- pins in repository checkouts ---------------------------------------------

SPEC = r"(?:(?:===|==|~=|>=|<=|!=|<|>)\s*[0-9][A-Za-z0-9.*+!_-]*)"
LOCK_RE_TMPL = r'\[\[package\]\]\nname = "{name}"\nversion = "([^"]+)"'
SCAN_SUFFIXES = {".txt", ".toml", ".md", ".mdx", ".yml", ".yaml", ".sh", ".py", ".cfg",
                 ".dockerfile", ".in", ".json", ".ps1", ""}
SKIP_PARTS = ("node_modules/", "/CHANGELOG", "CHANGELOG.md")


def pin_regex(project: str) -> re.Pattern:
    name = re.escape(project).replace(r"\-", "[-_]")
    return re.compile(
        r"(?<![A-Za-z0-9_.@/-])" + name + r"(?:\[[A-Za-z0-9_,-]+\])?"
        r"\s*(" + SPEC + r"(?:\s*,\s*" + SPEC + r")*)(?![A-Za-z0-9_-])",
        re.IGNORECASE,
    )


def tracked_files(root: Path) -> list[Path]:
    try:
        out = subprocess.run(["git", "ls-files", "-z"], cwd=root, check=True, capture_output=True).stdout
        rels = [r for r in out.decode().split("\0") if r]
    except (OSError, subprocess.CalledProcessError):
        rels = [str(p.relative_to(root)) for p in root.rglob("*") if p.is_file() and ".git" not in p.parts]
    files = []
    for rel in rels:
        p = root / rel
        if any(s in "/" + rel for s in SKIP_PARTS):
            continue
        if p.name == "uv.lock" or p.name.startswith("Dockerfile") or (
            p.suffix.lower() in SCAN_SUFFIXES and not p.name.endswith(".lock")
        ):
            files.append(p)
    return files


def repo_pins(project: str, roots: Iterable[Path]) -> list[tuple[str, str, SpecifierSet]]:
    """(file:line, raw text, specifier) for pins of `project` in each root."""
    rx = pin_regex(project)
    lock_rx = re.compile(LOCK_RE_TMPL.format(name=re.escape(project)))
    found = []
    for root in roots:
        for path in tracked_files(root):
            try:
                text = path.read_text()
            except (UnicodeDecodeError, OSError):
                continue
            where = lambda m: f"{root.name}/{path.relative_to(root)}:{text.count(chr(10), 0, m.start()) + 1}"  # noqa: E731
            if path.name == "uv.lock":
                for m in lock_rx.finditer(text):
                    found.append((where(m), m.group(0).splitlines()[-1], SpecifierSet(f"=={m.group(1)}")))
                continue
            for m in rx.finditer(text):
                try:
                    spec = SpecifierSet(m.group(1).replace(" ", ""))
                except InvalidSpecifier:
                    continue
                found.append((where(m), m.group(0), spec))
    return found


# --- the check ------------------------------------------------------------------


def read_delete_list(path: Path) -> tuple[list[str], dt.date | None, str | None]:
    versions, announced_on, url = [], None, None
    for line in path.read_text().splitlines():
        line = line.strip()
        m = re.match(r"#\s*announced:\s*(\d{4}-\d{2}-\d{2})\s*(\S+)?", line)
        if m:
            announced_on = dt.date.fromisoformat(m.group(1))
            url = m.group(2)
            continue
        line = line.split("#", 1)[0].strip()
        if line:
            versions.append(line)
    return versions, announced_on, url


def check(
    project: str,
    versions: list[str],
    *,
    today: dt.date,
    announced_on: dt.date | None,
    announcement_text: str | None,
    fetch: Fetch,
    repo_roots: Iterable[Path] = (),
    projects: Iterable[str] = OUR_PROJECTS,
) -> list[str]:
    doc = fetch(project)
    if doc is None:
        return [f"{project}: not on PyPI"]
    releases: dict[str, list[dict]] = {v: fs for v, fs in doc.get("releases", {}).items() if fs}
    deleting = set(versions)
    keep = latest_patch_per_minor(releases)
    problems: list[str] = []

    if len(deleting) != len(versions):
        problems.append(f"{project}: the list repeats a version")
    if announced_on is None:
        problems.append(f"{project}: no announcement (add '# announced: YYYY-MM-DD <release notes URL>')")
    elif (today - announced_on).days < ANNOUNCE_DAYS:
        problems.append(
            f"{project}: announced {announced_on}, only {(today - announced_on).days} days ago "
            f"(needs {ANNOUNCE_DAYS}; earliest deletion {announced_on + dt.timedelta(days=ANNOUNCE_DAYS)})"
        )

    for v in versions:
        files = releases.get(v)
        if not files:
            problems.append(f"{project} {v}: not on PyPI")
            continue
        if v in keep:
            problems.append(f"{project} {v}: newest patch of {v.rsplit('.', 1)[0]}, kept permanently")
        up = first_upload(files)
        if up is None:
            problems.append(f"{project} {v}: no upload time on PyPI")
        else:
            age = (today - up.date()).days
            if age < MIN_AGE_DAYS:
                problems.append(
                    f"{project} {v}: uploaded {up.date()}, {age} days ago (minimum {MIN_AGE_DAYS})"
                )
        if not all(f.get("yanked") for f in files):
            problems.append(f"{project} {v}: not yanked yet (yank it when announcing)")
        if announcement_text is not None and not re.search(
            r"(?<![0-9.])" + re.escape(v) + r"(?![0-9])", announcement_text
        ):
            problems.append(f"{project} {v}: not named in the announcement")

    for label, raw, spec in published_requirements(project, fetch, projects):
        lost = orphaned(spec, set(releases), deleting)
        if lost:
            problems.append(
                f"{project} {', '.join(sorted(lost, key=Version))}: the only versions left for "
                f"'{raw}' in published {label}"
            )
    for where, raw, spec in repo_pins(project, repo_roots):
        lost = orphaned(spec, set(releases), deleting)
        if lost:
            problems.append(
                f"{project} {', '.join(sorted(lost, key=Version))}: the only versions left for "
                f"'{raw.strip()}' at {where}"
            )
    return problems


def fetch_announcement(url: str) -> str:
    """Release notes text: a GitHub release page via `gh`, else the raw URL."""
    m = re.match(r"https://github\.com/([^/]+/[^/]+)/releases/tag/(.+)$", url)
    if m:
        out = subprocess.run(
            ["gh", "release", "view", m.group(2), "--repo", m.group(1), "--json", "body", "--jq", ".body"],
            check=True, capture_output=True, text=True,
        )
        return out.stdout
    with urllib.request.urlopen(url, timeout=30) as r:
        return r.read().decode("utf-8", "replace")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--project", required=True, help="PyPI project, e.g. cua-driver")
    ap.add_argument("--delete-list", required=True, type=Path)
    ap.add_argument("--repo-root", action="append", type=Path, default=[],
                    help="checkout to scan for pins (repeatable; default: this repository)")
    ap.add_argument("--today", type=dt.date.fromisoformat, default=dt.datetime.now(dt.timezone.utc).date())
    ap.add_argument("--announced-on", type=dt.date.fromisoformat)
    ap.add_argument("--announcement-url")
    ap.add_argument("--also-project", action="append", default=[],
                    help="another PyPI project whose requires_dist to check (repeatable)")
    args = ap.parse_args(argv)

    versions, announced_on, url = read_delete_list(args.delete_list)
    announced_on = args.announced_on or announced_on
    url = args.announcement_url or url
    if not versions:
        print("pypi-retention-guard: the delete list is empty", file=sys.stderr)
        return 2
    roots = args.repo_root or [Path(__file__).resolve().parents[2]]
    try:
        text = fetch_announcement(url) if url else None
        problems = check(
            args.project, versions, today=args.today, announced_on=announced_on,
            announcement_text=text, fetch=pypi_fetch, repo_roots=roots,
            projects=(*OUR_PROJECTS, *args.also_project),
        )
    except (OSError, subprocess.CalledProcessError, ValueError) as e:
        print(f"pypi-retention-guard: cannot verify ({e}); refusing", file=sys.stderr)
        return 2
    if url is None and announced_on is not None:
        problems.append(f"{args.project}: no announcement URL to check the versions against")
    if problems:
        print(f"pypi-retention-guard: REFUSED, {len(problems)} problem(s) in {args.delete_list}:")
        for p in problems:
            print(f"  {p}")
        print("Rule: docs/pypi-retention.md")
        return 1
    print(f"pypi-retention-guard: OK, {len(versions)} {args.project} version(s) may be deleted")
    return 0


if __name__ == "__main__":
    sys.exit(main())
