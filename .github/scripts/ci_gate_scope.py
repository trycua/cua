#!/usr/bin/env python3
"""Decide which workflows `ci-gate` must wait for, from the paths a PR changes.

The area map lives in `.github/ci-areas.json` and is documented in
`docs/contributing/CI.md`. The rules, in order:

1. A changed file that matches `shared`, or matches no area, widens the gate to
   every workflow (the behaviour before path scoping).
2. Otherwise each changed file belongs to the first area whose globs match it.
3. A workflow listed under some area is required only when one of its areas
   changed. A workflow listed under no area is always required, so a new or
   unmapped workflow can never be skipped by accident.

The gate itself never decides what a workflow's own `paths:` filter runs; it
only decides which of the runs that exist it waits for.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MAP_PATH = ROOT / '.github/ci-areas.json'
WORKFLOWS = ROOT / '.github/workflows'

# An urgent PR must name the Linear issue or incident that justifies the lane: a Linear key
# of one of our teams (CUA-123, CLE-45), a Linear issue URL, or an incident reference.
URGENT_REFERENCE = re.compile(
    r'(?<![A-Za-z0-9])(?:CUA|CLE)-\d+\b'
    r'|linear\.app/[\w-]+/issue/[A-Za-z0-9-]+'
    r'|(?i:\bincident\b[^\n]{0,80}(?:#\d+|\d+|https?://\S+))'
)


def load_map(path: Path = MAP_PATH) -> dict:
    return json.loads(path.read_text())


def glob_to_regex(glob: str) -> re.Pattern:
    out, i = [], 0
    while i < len(glob):
        c = glob[i]
        if glob.startswith('**/', i):
            out.append('(?:.*/)?')
            i += 3
        elif glob.startswith('**', i):
            out.append('.*')
            i += 2
        elif c == '*':
            out.append('[^/]*')
            i += 1
        elif c == '?':
            out.append('[^/]')
            i += 1
        else:
            out.append(re.escape(c))
            i += 1
    return re.compile('^' + ''.join(out) + '$')


def matches(path: str, globs: list[str]) -> bool:
    return any(glob_to_regex(g).match(path) for g in globs)


def literal_prefix(glob: str) -> str:
    """The fixed leading part of a glob, cut at the last slash before a wildcard."""
    m = re.search(r'[*?\[]', glob)
    if not m:
        return glob
    return glob[:m.start()].rsplit('/', 1)[0] + '/' if '/' in glob[:m.start()] else ''


def may_overlap(a: str, b: str) -> bool:
    """Conservative: true unless the two globs provably select disjoint files."""
    pa, pb = literal_prefix(a), literal_prefix(b)
    return pa.startswith(pb) or pb.startswith(pa)


def area_of(path: str, cfg: dict) -> str | None:
    """The first area owning `path`, or None when it is shared or unknown."""
    if matches(path, cfg['shared']):
        return None
    for name, area in cfg['areas'].items():
        if matches(path, area['paths']):
            return name
    return None


def scope(files: list[str], cfg: dict) -> tuple[list[str], set[str] | None]:
    """Return (changed areas, workflows to ignore). Ignore is None when nothing is."""
    areas: set[str] = set()
    for f in files:
        a = area_of(f, cfg)
        if a is None:
            return ['all'], set()
        areas.add(a)
    if not files:
        return ['all'], set()
    mapped = {w for a in cfg['areas'].values() for w in a['workflows']}
    needed = {w for name in areas for w in cfg['areas'][name]['workflows']}
    return sorted(areas), mapped - needed


def ignore_regex(ignored: set[str]) -> str:
    """An anchored regex over workflow file names; empty when nothing is ignored."""
    if not ignored:
        return ''
    return '^(' + '|'.join(re.escape(w) for w in sorted(ignored)) + ')$'


def urgent_reference_ok(body: str) -> bool:
    return bool(URGENT_REFERENCE.search(body or ''))


def pr_triggers(workflow: Path) -> list[str] | None:
    """The `pull_request` path globs of a workflow, or None if it has no path filter."""
    import yaml
    on = yaml.safe_load(workflow.read_text()).get(True) or {}
    if not isinstance(on, dict):
        return None
    pr = on.get('pull_request') or {}
    paths = pr.get('paths') if isinstance(pr, dict) else None
    if not paths or 'paths-ignore' in pr:
        return None
    return [p for p in paths if not p.startswith('!')]


def expected_areas(workflow: Path, cfg: dict) -> set[str]:
    """Every area a workflow's own trigger paths could touch (the superset the map must keep)."""
    paths = pr_triggers(workflow)
    if paths is None:
        return set()
    return {name for name, area in cfg['areas'].items()
            if any(may_overlap(p, g) for p in paths for g in area['paths'])}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('--map', type=Path, default=MAP_PATH)
    ap.add_argument('--labels', default='')
    ap.add_argument('--body-file', type=Path)
    args = ap.parse_args()
    cfg = load_map(args.map)
    files = [l.strip() for l in sys.stdin if l.strip()]
    areas, ignored = scope(files, cfg)
    regex = ignore_regex(ignored)
    urgent = 'urgent' in [l.strip() for l in args.labels.split(',')]
    lines = [
        f'changed files: {len(files)}',
        f'areas: {", ".join(areas)}',
        f'workflows the gate ignores: {", ".join(sorted(ignored)) or "none (waits for every run)"}',
    ]
    if urgent:
        body = args.body_file.read_text() if args.body_file else ''
        ok = urgent_reference_ok(body)
        lines.append('urgent lane: ' + ('Linear or incident reference found' if ok else
                     'MISSING Linear issue or incident reference in the PR description'))
        if not ok:
            print('\n'.join(lines))
            print('::error::The `urgent` label needs a Linear issue key (for example CUA-123) or '
                  'an incident reference in the PR description.')
            return 1
    print('\n'.join(lines))
    out = os.environ.get('GITHUB_OUTPUT')
    if out:
        with open(out, 'a') as fh:
            fh.write(f'areas={",".join(areas)}\nignore_re={regex}\n')
    summary = os.environ.get('GITHUB_STEP_SUMMARY')
    if summary:
        with open(summary, 'a') as fh:
            fh.write('### ci-gate scope\n\n' + '\n\n'.join(lines) + '\n')
    return 0


if __name__ == '__main__':
    sys.exit(main())
