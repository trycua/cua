#!/usr/bin/env python3
"""The repo's package dependency graph, read from the manifests.

`ci_gate_scope.py` uses it so a change to a shared crate or package waits for the
workflows of everything that depends on it, and nothing else. It reads only
manifests (every `Cargo.toml`, `package.json` and `pnpm-workspace.yaml`), so the
gate needs no toolchain and no network. `.github/scripts/tests/test_ci_graph.py`
checks the Cargo edges against `cargo metadata`.

Nodes are package directories (repo-relative, no trailing slash):

* Cargo: every manifest with a `[package]`. An edge is a `path` dependency in
  `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]` or any
  `[target.*.*dependencies]`, including `workspace = true` entries resolved
  through the workspace root's `[workspace.dependencies]`.
* JavaScript: every `package.json` with a `name`. An edge is a `workspace:`,
  `link:` or `file:` dependency on another package in the repo.

A workspace is the directory of a `[workspace]` manifest or a
`pnpm-workspace.yaml`. Its lockfile, toolchain pin and root manifest are
workspace files: changing one affects every member.
"""
from __future__ import annotations

import json
import os
import posixpath
import sys
from dataclasses import dataclass, field
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # Python < 3.11
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[2]
SKIP_DIRS = {'.git', 'node_modules', 'target', '.build', 'dist', '.next', '.venv', 'vendor'}
DEP_TABLES = ('dependencies', 'dev-dependencies', 'build-dependencies')
JS_DEP_TABLES = ('dependencies', 'devDependencies', 'peerDependencies', 'optionalDependencies')
# Files at a workspace root that every member builds with.
CARGO_WORKSPACE_FILES = ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'rust-toolchain', '.cargo/')
PNPM_WORKSPACE_FILES = ('pnpm-workspace.yaml', 'pnpm-lock.yaml', 'package.json', '.npmrc')


@dataclass
class Package:
    dir: str
    name: str
    kind: str  # 'cargo' or 'js'
    workspace: str
    deps: set[str] = field(default_factory=set)  # dirs of packages this one depends on


@dataclass
class Graph:
    packages: dict[str, Package]
    workspaces: dict[str, str]  # workspace dir -> 'cargo' or 'js'

    # --- queries -------------------------------------------------------------

    def owner(self, path: str) -> str | None:
        """The deepest package directory that contains `path`."""
        d = posixpath.dirname(path)
        while True:
            if d in self.packages:
                return d
            if not d:
                return None
            d = posixpath.dirname(d)

    def workspace_file(self, path: str) -> str | None:
        """The workspace whose root lockfile, manifest or toolchain pin `path` is."""
        for ws, kind in self.workspaces.items():
            rel = path[len(ws) + 1:] if ws and path.startswith(ws + '/') else (path if not ws else None)
            if rel is None:
                continue
            names = CARGO_WORKSPACE_FILES if kind == 'cargo' else PNPM_WORKSPACE_FILES
            if any(rel == n or (n.endswith('/') and rel.startswith(n)) for n in names):
                return ws
        return None

    def members(self, ws: str) -> set[str]:
        return {d for d, p in self.packages.items() if p.workspace == ws}

    def dependents(self, dirs: set[str]) -> set[str]:
        """`dirs` plus every package that depends on one of them, transitively."""
        rev: dict[str, set[str]] = {}
        for d, p in self.packages.items():
            for dep in p.deps:
                rev.setdefault(dep, set()).add(d)
        out, todo = set(dirs), list(dirs)
        while todo:
            for d in rev.get(todo.pop(), ()):
                if d not in out:
                    out.add(d)
                    todo.append(d)
        return out

    def closure(self, dirs: set[str]) -> set[str]:
        """`dirs` plus everything they depend on, transitively."""
        out, todo = set(dirs), list(dirs)
        while todo:
            p = self.packages.get(todo.pop())
            for dep in (p.deps if p else ()):
                if dep not in out:
                    out.add(dep)
                    todo.append(dep)
        return out

    def by_name(self, name: str) -> list[str]:
        return sorted(d for d, p in self.packages.items() if p.name == name)


# --- loading -----------------------------------------------------------------


def _walk(root: Path, names: set[str]) -> list[str]:
    found = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for f in filenames:
            if f in names:
                found.append(Path(dirpath, f).relative_to(root).as_posix())
    return sorted(found)


def _norm(base: str, rel: str) -> str:
    out = posixpath.normpath(posixpath.join(base, rel))
    return '' if out == '.' else out


def _cargo_dep_specs(manifest: dict):
    for table in DEP_TABLES:
        yield from (manifest.get(table) or {}).items()
    for target in (manifest.get('target') or {}).values():
        for table in DEP_TABLES:
            yield from (target.get(table) or {}).items()


def load(root: Path = ROOT) -> Graph:
    files = _walk(root, {'Cargo.toml', 'package.json', 'pnpm-workspace.yaml'})
    packages: dict[str, Package] = {}
    workspaces: dict[str, str] = {}
    ws_deps: dict[str, dict] = {}
    cargo: dict[str, dict] = {}

    for f in files:
        d = posixpath.dirname(f)
        name = posixpath.basename(f)
        if name == 'Cargo.toml':
            try:
                data = tomllib.loads((root / f).read_text())
            except (OSError, tomllib.TOMLDecodeError):
                continue  # templates and fixtures that are not real manifests
            cargo[d] = data
            if 'workspace' in data:
                workspaces[d] = 'cargo'
                ws_deps[d] = (data['workspace'].get('dependencies') or {})
        elif name == 'pnpm-workspace.yaml':
            workspaces[d] = 'js'

    def nearest_ws(d: str, kind: str) -> str:
        cur = d
        while True:
            if workspaces.get(cur) == kind:
                return cur
            if not cur:
                return d
            cur = posixpath.dirname(cur)

    for d, data in cargo.items():
        pkg = data.get('package')
        if not isinstance(pkg, dict) or 'name' not in pkg:
            continue
        ws = nearest_ws(d, 'cargo')
        p = Package(d, pkg['name'], 'cargo', ws)
        for key, spec in _cargo_dep_specs(data):
            if not isinstance(spec, dict):
                continue
            if spec.get('workspace') is True:
                spec = ws_deps.get(ws, {}).get(key)
                if isinstance(spec, dict) and 'path' in spec:
                    p.deps.add(_norm(ws, spec['path']))
            elif 'path' in spec:
                p.deps.add(_norm(d, spec['path']))
        packages[d] = p

    js: dict[str, dict] = {}
    for f in files:
        if posixpath.basename(f) != 'package.json':
            continue
        try:
            data = json.loads((root / f).read_text())
        except (OSError, ValueError):
            continue
        if isinstance(data, dict) and isinstance(data.get('name'), str):
            js[posixpath.dirname(f)] = data
    names: dict[tuple[str, str], str] = {}
    for d, data in js.items():
        ws = nearest_ws(d, 'js')
        if d in packages:  # a crate and a JS package in one directory: keep the crate
            continue
        packages[d] = Package(d, data['name'], 'js', ws)
        names[(ws, data['name'])] = d
    for d, data in js.items():
        p = packages.get(d)
        if not p or p.kind != 'js':
            continue
        for table in JS_DEP_TABLES:
            for dep, spec in (data.get(table) or {}).items():
                if not isinstance(spec, str):
                    continue
                if spec.startswith('workspace:'):
                    target = names.get((p.workspace, dep))
                    if target:
                        p.deps.add(target)
                elif spec.startswith(('link:', 'file:')):
                    target = _norm(d, spec.split(':', 1)[1])
                    if target in js:
                        p.deps.add(target)

    for p in packages.values():
        p.deps = {dep for dep in p.deps if dep in packages and dep != p.dir}
    return Graph(packages, workspaces)


def union(graphs: list[Graph]) -> Graph:
    """One graph with every package, workspace and edge of `graphs` (base and head trees)."""
    packages: dict[str, Package] = {}
    workspaces: dict[str, str] = {}
    for g in graphs:
        workspaces.update(g.workspaces)
        for d, p in g.packages.items():
            if d in packages:
                packages[d].deps |= p.deps
            else:
                packages[d] = Package(p.dir, p.name, p.kind, p.workspace, set(p.deps))
    return Graph(packages, workspaces)


def main() -> int:
    g = load()
    if len(sys.argv) > 1:
        for name in sys.argv[1:]:
            for d in g.by_name(name) or [name]:
                deps = sorted(g.dependents({d}) - {d})
                print(f'{d} ({len(deps)} dependents)')
                for dep in deps:
                    print(f'  {dep}')
        return 0
    print(json.dumps({d: sorted(p.deps) for d, p in sorted(g.packages.items())}, indent=1))
    return 0


if __name__ == '__main__':
    sys.exit(main())
