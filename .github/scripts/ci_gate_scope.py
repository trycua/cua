#!/usr/bin/env python3
"""Decide which workflows `ci-gate` must wait for, from the paths a PR changes.

The area map lives in `.github/ci-areas.json` and is documented in
`docs/contributing/CI.md`. The package graph comes from the manifests
(`ci_graph.py`). The rules, in order:

1. A changed file that matches `shared` widens the gate to every workflow.
2. A workspace file (a Cargo or pnpm workspace's root manifest, lockfile,
   toolchain pin or `.cargo/`) affects every member of that workspace.
3. A file inside a package (the deepest `Cargo.toml` or `package.json`
   directory above it) affects that package; a file listed under `inputs`
   affects the packages named there.
4. Every affected package also affects every package that depends on it,
   transitively. Each affected package adds the area its directory is in.
5. Any other file adds the first area whose globs match it. A file, or an
   affected package, that matches no area widens the gate to every workflow.
6. A workflow listed under some area is required only when one of its areas
   changed. A workflow listed under no area is always required, so a new or
   unmapped workflow can never be skipped by accident.
7. A workflow under `narrow` is required only when one of the packages it
   builds (`packages`, package directories) is affected, or a changed file
   matches its `paths`. "Affected" is rule 4: the package changed or depends on
   a changed package, transitively. Narrowing only ever removes a workflow that
   rule 6 required; a shared file still requires it.

The gate runs this script, the map and the graph from the base branch, so a
pull request cannot narrow its own gate by editing them. It also reads the
pull request's manifests and adds their edges, so a dependency the pull request
adds counts too.

The same decision also gates execution (CUA-3856): every workflow with the
shared `quick` stage (ci-quick.yml) runs `--workflow <file>` from the base
commit and skips its heavy jobs when the workflow is not required. A workflow's
own `paths:` filter is only a cheap pre-filter; it must stay a superset of what
the map requires (test_ci_gate_scope.py checks that).
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci_graph  # noqa: E402

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


def load_graph(roots: list[Path] | None = None) -> ci_graph.Graph:
    """The package graph of the base tree, plus the pull request's edges (CI_GATE_HEAD_ROOT)."""
    if roots is None:
        roots = [ROOT]
        head = os.environ.get('CI_GATE_HEAD_ROOT')
        if head:
            roots.append(Path(head))
    return ci_graph.union([ci_graph.load(r) for r in roots])


def affected_packages(files: list[str], cfg: dict, graph: ci_graph.Graph) -> tuple[set[str], set[str], bool]:
    """Return (packages the files touch directly, non-package files, everything)."""
    seeds: set[str] = set()
    loose: set[str] = set()
    for f in files:
        if matches(f, cfg['shared']):
            return set(), set(), True
        ws = graph.workspace_file(f)
        if ws is not None:
            seeds |= graph.members(ws)
            continue
        inputs = [pkg for glob, pkgs in cfg.get('inputs', {}).items() if matches(f, [glob]) for pkg in pkgs]
        if inputs:
            seeds.update(p for p in inputs if p in graph.packages)
            loose.add(f)  # an input still counts for its own area
            continue
        owner = graph.owner(f)
        if owner is not None:
            seeds.add(owner)
        else:
            loose.add(f)
    return seeds, loose, False


def narrowed_out(needed: set[str], loose: set[str], affected: set[str], cfg: dict) -> set[str]:
    """The required workflows that rule 7 drops: none of their packages or paths changed."""
    out = set()
    for workflow, rule in cfg.get('narrow', {}).items():
        if workflow not in needed:
            continue
        if affected & set(rule.get('packages', [])):
            continue
        if any(matches(f, rule.get('paths', [])) for f in loose):
            continue
        out.add(workflow)
    return out


def scope(files: list[str], cfg: dict, graph: ci_graph.Graph | None = None) -> tuple[list[str], set[str] | None]:
    """Return (changed areas, workflows to ignore). Ignore is empty when nothing is."""
    if not files:
        return ['all'], set()
    graph = graph if graph is not None else load_graph()
    seeds, loose, everything = affected_packages(files, cfg, graph)
    if everything:
        return ['all'], set()
    affected = graph.dependents(seeds)
    areas: set[str] = set()
    for path in sorted(loose) + [d + '/' for d in sorted(affected)]:
        a = area_of(path, cfg)
        if a is None:
            return ['all'], set()
        areas.add(a)
    mapped = {w for a in cfg['areas'].values() for w in a['workflows']}
    needed = {w for name in areas for w in cfg['areas'][name]['workflows']}
    # A narrowed workflow that no area lists (one with no `paths:` filter) starts out required.
    narrow = set(cfg.get('narrow', {}))
    needed |= narrow - mapped
    needed -= narrowed_out(needed, loose, affected, cfg)
    return sorted(areas), (mapped | narrow) - needed


def decide_workflow(workflow: str, files: list[str], cfg: dict, graph: ci_graph.Graph) -> int:
    """The execution side of the same decision: one workflow, skip or run."""
    workflow = Path(workflow).name
    areas, ignored = scope(files, cfg, graph)
    skip = workflow in ignored
    line = (f'{workflow}: ' + ('skip, ' if skip else 'run, ') + f'{len(files)} changed files, '
            f'areas: {", ".join(areas)}')
    print(line)
    out = os.environ.get('GITHUB_OUTPUT')
    if out:
        with open(out, 'a') as fh:
            fh.write(f'skip={"true" if skip else "false"}\nareas={",".join(areas)}\n')
    summary = os.environ.get('GITHUB_STEP_SUMMARY')
    if summary:
        with open(summary, 'a') as fh:
            fh.write('### CI scope\n\n' + line + ('\n\nThe changed files do not reach this workflow '
                     '(.github/ci-areas.json), so its jobs are skipped and ci-gate does not wait for it.\n'
                     if skip else '\n'))
    return 0


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
    ap.add_argument('--workflow', help='decide for one workflow file: writes skip=true|false')
    args = ap.parse_args()
    cfg = load_map(args.map)
    files = [l.strip() for l in sys.stdin if l.strip()]
    graph = load_graph()
    if args.workflow:
        return decide_workflow(args.workflow, files, cfg, graph)
    seeds, _, _ = affected_packages(files, cfg, graph)
    affected = graph.dependents(seeds)
    areas, ignored = scope(files, cfg, graph)
    regex = ignore_regex(ignored)
    urgent = 'urgent' in [l.strip() for l in args.labels.split(',')]
    lines = [
        f'changed files: {len(files)}',
        f'packages affected (changed, plus their dependents): {len(affected)}'
        + (f' ({", ".join(sorted(graph.packages[d].name for d in affected))})' if 0 < len(affected) <= 40 else ''),
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
