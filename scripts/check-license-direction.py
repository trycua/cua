#!/usr/bin/env python3
"""Check that MIT packages only depend on permissively licensed internal packages.

The script discovers the packages in this monorepo (Cargo crates, npm packages,
and Python projects), reads each package's declared licence, and follows every
internal dependency edge: a Cargo path or workspace dependency, a pnpm
``workspace:`` (or ``file:``/``link:``) dependency, a uv path or workspace
source, or a SwiftPM ``.package(path:)`` dependency. When an MIT package depends on an internal package whose licence is not
in the allow-list, the edge is reported as a violation.

A package's licence comes from its metadata (Cargo ``license``/``license-file``,
npm ``license``, pyproject ``project.license``; SwiftPM has no licence field, so
a Swift package always uses its nearest LICENSE file). When no field is set, the
nearest LICENSE file up the tree is used and the package is reported as having
no declared licence.

Exit status: 0 when clean, 1 on violations. Packages without a declared licence
are warnings unless ``--strict`` is passed. Development-only edges (Cargo
dev-dependencies, npm devDependencies, dependency groups) are skipped unless
``--include-dev`` is passed, because they are not shipped with the package.

Python 3.9 compatible and standard library only.

Usage:
    python3 scripts/check-license-direction.py [--strict] [--include-dev] [--verbose]
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, field
from typing import Any, Dict, Iterable, List, Optional, Tuple

ALLOWED = {"MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "Unlicense"}

MANIFESTS = ("Cargo.toml", "package.json", "pyproject.toml", "Package.swift")
SKIP_DIRS = {"node_modules", ".git", "target", ".venv", "venv", "__pycache__", "dist", "build"}
LICENSE_FILE_RE = re.compile(r"^(LICEN[CS]E|COPYING)([.-].*)?$", re.IGNORECASE)

# Well-known SPDX identifiers, keyed by lower case, so declared values can be
# normalised without guessing.
KNOWN_SPDX = [
    "0BSD",
    "AGPL-3.0",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "BUSL-1.1",
    "CC0-1.0",
    "FSL-1.1-ALv2",
    "FSL-1.1-MIT",
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "ISC",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
    "MIT",
    "MPL-2.0",
    "Unlicense",
    "UNLICENSED",
    "Zlib",
]
SPDX_BY_LOWER = {s.lower(): s for s in KNOWN_SPDX}
ALIASES = {
    "mit license": "MIT",
    "the mit license": "MIT",
    "apache 2.0": "Apache-2.0",
    "apache-2": "Apache-2.0",
    "apache license 2.0": "Apache-2.0",
    "apache license, version 2.0": "Apache-2.0",
    "apache software license": "Apache-2.0",
    "bsd-3": "BSD-3-Clause",
    "bsd-2": "BSD-2-Clause",
}


# --------------------------------------------------------------------------
# TOML: use tomllib (3.11+) when available, otherwise a small reader that
# covers the subset used by Cargo.toml and pyproject.toml files.
# --------------------------------------------------------------------------

try:  # pragma: no cover - depends on interpreter version
    import tomllib as _tomllib  # type: ignore[import-not-found]
except ImportError:  # pragma: no cover
    _tomllib = None


class _TomlReader:
    def __init__(self, text: str) -> None:
        self.s = text
        self.i = 0
        self.n = len(text)

    def error(self, msg: str) -> ValueError:
        line = self.s.count("\n", 0, self.i) + 1
        return ValueError("TOML parse error at line %d: %s" % (line, msg))

    def skip_ws(self, newlines: bool = False) -> None:
        while self.i < self.n:
            c = self.s[self.i]
            if c in " \t\r" or (newlines and c == "\n"):
                self.i += 1
            elif c == "#":
                while self.i < self.n and self.s[self.i] != "\n":
                    self.i += 1
            else:
                break

    def parse(self) -> Dict[str, Any]:
        root: Dict[str, Any] = {}
        current = root
        while True:
            self.skip_ws(newlines=True)
            if self.i >= self.n:
                return root
            if self.s.startswith("[[", self.i):
                self.i += 2
                keys = self.key_path("]]")
                self.i += 2
                parent = self.descend(root, keys[:-1])
                arr = parent.setdefault(keys[-1], [])
                if not isinstance(arr, list):
                    raise self.error("array of tables conflicts with key")
                current = {}
                arr.append(current)
            elif self.s[self.i] == "[":
                self.i += 1
                keys = self.key_path("]")
                self.i += 1
                current = self.descend(root, keys)
            else:
                keys = self.key_path("=")
                self.i += 1
                self.skip_ws()
                value = self.value()
                target = self.descend(current, keys[:-1])
                target[keys[-1]] = value
            self.skip_ws()
            if self.i < self.n and self.s[self.i] != "\n":
                raise self.error("expected end of line")

    def descend(self, table: Dict[str, Any], keys: List[str]) -> Dict[str, Any]:
        for k in keys:
            nxt = table.setdefault(k, {})
            if isinstance(nxt, list):
                nxt = nxt[-1]
            if not isinstance(nxt, dict):
                raise self.error("key %r is not a table" % k)
            table = nxt
        return table

    def key_path(self, terminator: str) -> List[str]:
        keys = []
        while True:
            self.skip_ws()
            c = self.s[self.i] if self.i < self.n else ""
            if c == '"':
                keys.append(self.basic_string())
            elif c == "'":
                keys.append(self.literal_string())
            else:
                m = re.compile(r"[A-Za-z0-9_-]+").match(self.s, self.i)
                if not m:
                    raise self.error("invalid key")
                keys.append(m.group(0))
                self.i = m.end()
            self.skip_ws()
            if self.s.startswith(terminator, self.i):
                return keys
            if self.i < self.n and self.s[self.i] == ".":
                self.i += 1
                continue
            raise self.error("expected %r" % terminator)

    def basic_string(self) -> str:
        if self.s.startswith('"""', self.i):
            end = self.s.find('"""', self.i + 3)
            while end != -1 and self.s[end - 1] == "\\":
                end = self.s.find('"""', end + 1)
            if end == -1:
                raise self.error("unterminated string")
            raw = self.s[self.i + 3 : end]
            self.i = end + 3
            return self.unescape(raw.lstrip("\n"))
        self.i += 1
        out = []
        while self.i < self.n:
            c = self.s[self.i]
            if c == "\\":
                out.append(self.s[self.i : self.i + 2])
                self.i += 2
                continue
            if c == '"':
                self.i += 1
                return self.unescape("".join(out))
            if c == "\n":
                break
            out.append(c)
            self.i += 1
        raise self.error("unterminated string")

    @staticmethod
    def unescape(raw: str) -> str:
        try:
            return json.loads('"%s"' % raw.replace("\n", "\\n").replace("\t", "\\t"))
        except ValueError:
            return raw

    def literal_string(self) -> str:
        if self.s.startswith("'''", self.i):
            end = self.s.find("'''", self.i + 3)
            if end == -1:
                raise self.error("unterminated string")
            raw = self.s[self.i + 3 : end]
            self.i = end + 3
            return raw.lstrip("\n")
        end = self.s.find("'", self.i + 1)
        if end == -1:
            raise self.error("unterminated string")
        raw = self.s[self.i + 1 : end]
        self.i = end + 1
        return raw

    def value(self) -> Any:
        c = self.s[self.i] if self.i < self.n else ""
        if c == '"':
            return self.basic_string()
        if c == "'":
            return self.literal_string()
        if c == "[":
            self.i += 1
            items = []
            while True:
                self.skip_ws(newlines=True)
                if self.s[self.i] == "]":
                    self.i += 1
                    return items
                items.append(self.value())
                self.skip_ws(newlines=True)
                if self.s[self.i] == ",":
                    self.i += 1
        if c == "{":
            self.i += 1
            table: Dict[str, Any] = {}
            while True:
                self.skip_ws()
                if self.s[self.i] == "}":
                    self.i += 1
                    return table
                keys = self.key_path("=")
                self.i += 1
                self.skip_ws()
                self.descend(table, keys[:-1])[keys[-1]] = self.value()
                self.skip_ws()
                if self.s[self.i] == ",":
                    self.i += 1
        m = re.compile(r"[^\s,\]}#]+").match(self.s, self.i)
        if not m:
            raise self.error("invalid value")
        self.i = m.end()
        token = m.group(0)
        if token == "true":
            return True
        if token == "false":
            return False
        try:
            return int(token.replace("_", ""), 0)
        except ValueError:
            try:
                return float(token.replace("_", ""))
            except ValueError:
                return token  # dates and times are kept as text


def load_toml(path: str) -> Dict[str, Any]:
    with open(path, "rb") as fh:
        data = fh.read()
    if _tomllib is not None:
        return _tomllib.loads(data.decode("utf-8"))
    return _TomlReader(data.decode("utf-8")).parse()


# --------------------------------------------------------------------------
# Licences
# --------------------------------------------------------------------------


def detect_license_text(path: str) -> Optional[str]:
    """Return the SPDX identifier a licence file's text states, if recognisable."""
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            text = " ".join(fh.read().split())
    except OSError:
        return None
    low = text.lower()
    if "gnu affero general public license" in low and "version 3" in low:
        return "AGPL-3.0"
    if "gnu lesser general public license" in low:
        return "LGPL-3.0" if "version 3" in low else "LGPL-2.1"
    if "gnu general public license" in low:
        return "GPL-3.0" if "version 3" in low else "GPL-2.0"
    if "mozilla public license version 2.0" in low:
        return "MPL-2.0"
    if "business source license" in low:
        return "BUSL-1.1"
    # Before MIT and Apache: the FSL embeds its future licence's text.
    if "functional source license, version 1.1" in low:
        return "FSL-1.1-MIT" if "mit future license" in low else "FSL-1.1-ALv2"
    if "apache license" in low and "version 2.0" in low:
        return "Apache-2.0"
    if "permission is hereby granted, free of charge, to any person obtaining a copy" in low:
        return "MIT"
    if "this is free and unencumbered software released into the public domain" in low:
        return "Unlicense"
    if "permission to use, copy, modify, and/or distribute this software for any purpose" in low:
        return "ISC" if "provided that the above copyright notice" in low else "0BSD"
    if "redistribution and use in source and binary forms" in low:
        return "BSD-3-Clause" if "neither the name" in low else "BSD-2-Clause"
    return None


def normalise_license(value: str) -> str:
    """Map a declared licence string to SPDX where the mapping is unambiguous."""
    v = value.strip()
    low = v.lower()
    if low in SPDX_BY_LOWER:
        return SPDX_BY_LOWER[low]
    if low in ALIASES:
        return ALIASES[low]

    def repl(m: "re.Match[str]") -> str:
        return SPDX_BY_LOWER.get(m.group(0).lower(), m.group(0))

    return re.sub(r"[A-Za-z0-9.+-]+", repl, v)


def _tokens(expr: str) -> List[str]:
    return re.findall(r"\(|\)|[^\s()]+", expr)


def _parse_or(toks: List[str], pos: int) -> Tuple[List[List[str]], int]:
    """Parse an SPDX expression into disjunctive normal form: [[ids ANDed], ...]."""
    left, pos = _parse_and(toks, pos)
    while pos < len(toks) and toks[pos].upper() == "OR":
        right, pos = _parse_and(toks, pos + 1)
        left = left + right
    return left, pos


def _parse_and(toks: List[str], pos: int) -> Tuple[List[List[str]], int]:
    left, pos = _parse_atom(toks, pos)
    while pos < len(toks) and toks[pos].upper() == "AND":
        right, pos = _parse_atom(toks, pos + 1)
        left = [a + b for a in left for b in right]
    return left, pos


def _parse_atom(toks: List[str], pos: int) -> Tuple[List[List[str]], int]:
    if pos >= len(toks):
        return [[]], pos
    if toks[pos] == "(":
        inner, pos = _parse_or(toks, pos + 1)
        if pos < len(toks) and toks[pos] == ")":
            pos += 1
        return inner, pos
    ident = toks[pos].rstrip("+")
    pos += 1
    if pos < len(toks) and toks[pos].upper() == "WITH":
        pos += 2  # a licence exception does not change the base licence
    return [[ident]], pos


def license_options(expr: Optional[str]) -> List[List[str]]:
    if not expr:
        return []
    try:
        dnf, _ = _parse_or(_tokens(normalise_license(expr)), 0)
    except IndexError:
        return [[expr]]
    return [opt for opt in dnf if opt]


def is_mit(expr: Optional[str]) -> bool:
    return any("MIT" in opt for opt in license_options(expr))


def is_allowed(expr: Optional[str]) -> bool:
    return any(all(i in ALLOWED for i in opt) for opt in license_options(expr))


# --------------------------------------------------------------------------
# Packages
# --------------------------------------------------------------------------


@dataclass
class Package:
    ecosystem: str
    name: str
    directory: str  # relative to the repository root, "." for the root
    manifest: Dict[str, Any]
    declared: Optional[str] = None
    declared_from: str = ""
    license: Optional[str] = None
    license_from: str = ""
    raw_deps: List[Tuple[str, str, Any]] = field(default_factory=list)

    @property
    def label(self) -> str:
        return "%s (%s)" % (self.name, self.directory)


@dataclass
class Edge:
    source: Package
    target: Package
    kind: str


class Repo:
    def __init__(self, root: str) -> None:
        self.root = os.path.abspath(root)
        self.files = self._list_files()
        self.dirs_with_license: Dict[str, str] = {}
        for rel in self.files:
            d, base = os.path.split(rel)
            if LICENSE_FILE_RE.match(base):
                self.dirs_with_license.setdefault(d or ".", rel)
        self.packages: List[Package] = []
        self.warnings: List[str] = []
        self.toml_cache: Dict[str, Optional[Dict[str, Any]]] = {}

    def _list_files(self) -> List[str]:
        try:
            out = subprocess.run(
                ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                cwd=self.root,
                check=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            ).stdout.decode("utf-8")
            files = [f for f in out.split("\0") if f]
        except (OSError, subprocess.CalledProcessError):
            files = []
            for dirpath, dirnames, filenames in os.walk(self.root):
                dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
                for f in filenames:
                    files.append(os.path.relpath(os.path.join(dirpath, f), self.root))
        return sorted(
            f.replace(os.sep, "/")
            for f in files
            if not SKIP_DIRS.intersection(f.split("/")[:-1])
            and os.path.exists(os.path.join(self.root, f))
        )

    def rel(self, path: str) -> str:
        r = os.path.relpath(os.path.normpath(path), self.root).replace(os.sep, "/")
        return "." if r == "" else r

    def abs(self, rel: str) -> str:
        return os.path.normpath(os.path.join(self.root, rel))

    def toml(self, rel_path: str) -> Optional[Dict[str, Any]]:
        if rel_path not in self.toml_cache:
            try:
                self.toml_cache[rel_path] = load_toml(self.abs(rel_path))
            except (OSError, ValueError) as exc:
                self.warnings.append("could not parse %s: %s" % (rel_path, exc))
                self.toml_cache[rel_path] = None
        return self.toml_cache[rel_path]

    def nearest_license_file(self, directory: str) -> Optional[str]:
        d = directory
        while True:
            if d in self.dirs_with_license:
                return self.dirs_with_license[d]
            if d == ".":
                return None
            d = os.path.dirname(d) or "."

    def ancestors_with(self, directory: str, filename: str) -> Iterable[str]:
        d = directory
        while True:
            parent = os.path.dirname(d) or "." if d != "." else None
            if parent is None:
                return
            rel = filename if parent == "." else parent + "/" + filename
            if rel in self.files:
                yield parent
            d = parent

    # ---- discovery ----------------------------------------------------

    def discover(self) -> None:
        for rel in self.files:
            base = rel.rsplit("/", 1)[-1]
            if base not in MANIFESTS:
                continue
            directory = os.path.dirname(rel) or "."
            if base == "Cargo.toml":
                self._add_cargo(rel, directory)
            elif base == "package.json":
                self._add_npm(rel, directory)
            elif base == "Package.swift":
                self._add_swift(rel, directory)
            else:
                self._add_python(rel, directory)
        for pkg in self.packages:
            if pkg.declared is None:
                lic = self.nearest_license_file(pkg.directory)
                if lic:
                    pkg.license = detect_license_text(self.abs(lic))
                    pkg.license_from = lic
            else:
                pkg.license = normalise_license(pkg.declared)
                pkg.license_from = pkg.declared_from
                own = self.dirs_with_license.get(pkg.directory)
                own_text = detect_license_text(self.abs(own)) if own else None
                if own_text and not any(
                    i.split("-only")[0].split("-or-later")[0] == own_text
                    for opt in license_options(pkg.license)
                    for i in opt
                ):
                    self.warnings.append(
                        "%s %s declares %s but %s reads as %s"
                        % (pkg.ecosystem, pkg.label, pkg.license, own, own_text)
                    )

    def _add_cargo(self, rel: str, directory: str) -> None:
        data = self.toml(rel)
        if not data or "package" not in data:
            return
        pkg_table = data["package"]
        pkg = Package("cargo", str(pkg_table.get("name", directory)), directory, data)
        lic = pkg_table.get("license")
        lic_file = pkg_table.get("license-file")
        if isinstance(lic, dict) and lic.get("workspace"):
            ws = self._cargo_workspace(directory)
            if ws:
                ws_pkg = (self.toml(self._join(ws, "Cargo.toml")) or {}).get("workspace", {})
                lic = ws_pkg.get("package", {}).get("license")
                lic_file = lic_file or ws_pkg.get("package", {}).get("license-file")
                pkg.declared_from = "%s [workspace.package] license" % self._join(ws, "Cargo.toml")
        if isinstance(lic, str):
            pkg.declared = lic
            pkg.declared_from = pkg.declared_from or "%s license" % rel
        elif isinstance(lic_file, str):
            detected = detect_license_text(self.abs(self._join(directory, lic_file)))
            if detected:
                pkg.declared = detected
                pkg.declared_from = "%s license-file" % rel
        for kind, table in self._cargo_dep_tables(data):
            for name, spec in table.items():
                pkg.raw_deps.append((kind, name, spec))
        self.packages.append(pkg)

    @staticmethod
    def _cargo_dep_tables(data: Dict[str, Any]) -> Iterable[Tuple[str, Dict[str, Any]]]:
        kinds = (
            ("dependencies", "runtime"),
            ("build-dependencies", "build"),
            ("dev-dependencies", "dev"),
        )
        for key, kind in kinds:
            if isinstance(data.get(key), dict):
                yield kind, data[key]
        for target in (data.get("target") or {}).values():
            if isinstance(target, dict):
                for key, kind in kinds:
                    if isinstance(target.get(key), dict):
                        yield kind, target[key]

    def _cargo_workspace(self, directory: str) -> Optional[str]:
        own = self.toml(self._join(directory, "Cargo.toml")) or {}
        if "workspace" in own:
            return directory
        explicit = (own.get("package") or {}).get("workspace")
        if isinstance(explicit, str):
            return self.rel(self.abs(self._join(directory, explicit)))
        for parent in self.ancestors_with(directory, "Cargo.toml"):
            if "workspace" in (self.toml(self._join(parent, "Cargo.toml")) or {}):
                return parent
        return None

    def _add_npm(self, rel: str, directory: str) -> None:
        try:
            with open(self.abs(rel), encoding="utf-8") as fh:
                data = json.load(fh)
        except (OSError, ValueError) as exc:
            self.warnings.append("could not parse %s: %s" % (rel, exc))
            return
        if not isinstance(data, dict) or not data.get("name"):
            return
        pkg = Package("npm", str(data["name"]), directory, data)
        lic = data.get("license")
        if isinstance(lic, dict):
            lic = lic.get("type")
        if not lic and isinstance(data.get("licenses"), list):
            types = [
                x.get("type") for x in data["licenses"] if isinstance(x, dict) and x.get("type")
            ]
            lic = " OR ".join(types) if types else None
        if isinstance(lic, str) and lic:
            if lic.upper().startswith("SEE LICENSE IN "):
                detected = detect_license_text(self.abs(self._join(directory, lic[15:].strip())))
                lic = detected
            if lic:
                pkg.declared = lic
                pkg.declared_from = "%s license" % rel
        kinds = (
            ("dependencies", "runtime"),
            ("optionalDependencies", "optional"),
            ("peerDependencies", "peer"),
            ("devDependencies", "dev"),
        )
        for key, kind in kinds:
            for name, spec in (data.get(key) or {}).items():
                pkg.raw_deps.append((kind, name, spec))
        self.packages.append(pkg)

    def _add_python(self, rel: str, directory: str) -> None:
        data = self.toml(rel)
        if not data:
            return
        project = data.get("project") or {}
        poetry = ((data.get("tool") or {}).get("poetry")) or {}
        name = project.get("name") or poetry.get("name")
        if not name:
            return
        pkg = Package("python", str(name), directory, data)
        lic = project.get("license")
        if isinstance(lic, dict):
            if lic.get("text"):
                lic = str(lic["text"])
            elif lic.get("file"):
                lic = detect_license_text(self.abs(self._join(directory, str(lic["file"]))))
            else:
                lic = None
        if not lic and isinstance(poetry.get("license"), str):
            lic = poetry["license"]
        if isinstance(lic, str) and lic.strip():
            pkg.declared = lic.strip()
            pkg.declared_from = "%s project.license" % rel
        for req in project.get("dependencies") or []:
            pkg.raw_deps.append(("runtime", req, None))
        for extra, reqs in (project.get("optional-dependencies") or {}).items():
            for req in reqs or []:
                pkg.raw_deps.append(("optional[%s]" % extra, req, None))
        for group, reqs in (data.get("dependency-groups") or {}).items():
            for req in reqs or []:
                if isinstance(req, str):
                    pkg.raw_deps.append(("dev[%s]" % group, req, None))
        for req in ((data.get("tool") or {}).get("uv") or {}).get("dev-dependencies") or []:
            pkg.raw_deps.append(("dev", req, None))
        self.packages.append(pkg)

    def _add_swift(self, rel: str, directory: str) -> None:
        try:
            with open(self.abs(rel), encoding="utf-8") as fh:
                text = fh.read()
        except OSError as exc:
            self.warnings.append("could not read %s: %s" % (rel, exc))
            return
        m = re.search(r'Package\(\s*name:\s*"([^"]+)"', text)
        if not m:
            return
        pkg = Package("swift", m.group(1), directory, {})
        # SwiftPM has no dev or optional dependencies: every package edge ships.
        for dep in re.finditer(r'\.package\(\s*(?:name:\s*"[^"]*"\s*,\s*)?path:\s*"([^"]+)"', text):
            pkg.raw_deps.append(("runtime", dep.group(1), dep.group(1)))
        self.packages.append(pkg)

    def _swift_target(self, pkg, spec, by_dir):
        target_dir = self.rel(self.abs(self._join(pkg.directory, spec)))
        return by_dir.get(("swift", target_dir)), "path %s" % spec

    @staticmethod
    def _join(directory: str, rel: str) -> str:
        return rel if directory == "." else directory + "/" + rel

    # ---- edges ----------------------------------------------------------

    def edges(self) -> List[Edge]:
        by_dir = {(p.ecosystem, p.directory): p for p in self.packages}
        by_name: Dict[Tuple[str, str], Package] = {}
        for p in self.packages:
            key = (p.ecosystem, pep503(p.name) if p.ecosystem == "python" else p.name)
            by_name.setdefault(key, p)
        result: List[Edge] = []
        for pkg in self.packages:
            for kind, name, spec in pkg.raw_deps:
                target, where = None, None
                if pkg.ecosystem == "cargo":
                    target, where = self._cargo_target(pkg, name, spec, by_dir)
                elif pkg.ecosystem == "npm":
                    target, where = self._npm_target(pkg, name, spec, by_dir, by_name)
                elif pkg.ecosystem == "swift":
                    target, where = self._swift_target(pkg, spec, by_dir)
                else:
                    target, where = self._python_target(pkg, name, by_dir, by_name)
                if where and target is None:
                    self.warnings.append(
                        "%s %s: internal dependency %r (%s) does not resolve to a package"
                        % (pkg.ecosystem, pkg.label, name, where)
                    )
                if target is not None and target is not pkg:
                    result.append(Edge(pkg, target, kind))
        return result

    def _cargo_target(self, pkg, name, spec, by_dir):
        if not isinstance(spec, dict):
            return None, None
        base = pkg.directory
        if spec.get("workspace"):
            ws = self._cargo_workspace(pkg.directory)
            ws_deps = (
                ((self.toml(self._join(ws, "Cargo.toml")) or {}).get("workspace") or {}).get(
                    "dependencies"
                )
                if ws
                else None
            )
            ws_spec = (ws_deps or {}).get(name)
            if not isinstance(ws_spec, dict) or "path" not in ws_spec:
                return None, None
            spec, base = ws_spec, ws
        if "path" not in spec:
            return None, None
        target_dir = self.rel(self.abs(self._join(base, str(spec["path"]))))
        return by_dir.get(("cargo", target_dir)), "path %s" % spec["path"]

    def _npm_target(self, pkg, name, spec, by_dir, by_name):
        if not isinstance(spec, str):
            return None, None
        if spec.startswith("workspace:"):
            alias = spec[len("workspace:") :]
            real = name
            if "@" in alias.lstrip("@") and not alias.startswith(("^", "~", "*")):
                real = alias.rsplit("@", 1)[0]
            return by_name.get(("npm", real)), spec
        for prefix in ("file:", "link:"):
            if spec.startswith(prefix):
                target_dir = self.rel(self.abs(self._join(pkg.directory, spec[len(prefix) :])))
                if not os.path.isdir(self.abs(target_dir)):
                    return None, None  # a tarball or an external path
                return by_dir.get(("npm", target_dir)), spec
        return None, None

    def _python_target(self, pkg, req, by_dir, by_name):
        m = re.match(r"\s*([A-Za-z0-9][A-Za-z0-9._-]*)", req)
        if not m:
            return None, None
        name = pep503(m.group(1))
        direct = re.search(r"@\s*file:(?://)?(\S+)", req)
        if direct:
            path = direct.group(1).replace("${PROJECT_ROOT}", self.abs(pkg.directory))
            target_dir = self.rel(
                path if os.path.isabs(path) else self.abs(self._join(pkg.directory, path))
            )
            return by_dir.get(("python", target_dir)), "file reference"
        for base, sources in self._uv_sources(pkg):
            src = sources.get(name)
            if isinstance(src, list):
                src = next((s for s in src if isinstance(s, dict)), None)
            if not isinstance(src, dict):
                continue
            if src.get("workspace"):
                return by_name.get(("python", name)), "uv workspace source"
            if "path" in src:
                target_dir = self.rel(self.abs(self._join(base, str(src["path"]))))
                return by_dir.get(("python", target_dir)), "uv path source %s" % src["path"]
            return None, None  # git, url, or index source: external
        return None, None

    def _uv_sources(self, pkg: Package) -> Iterable[Tuple[str, Dict[str, Any]]]:
        """Yield (base directory, normalised uv sources) in precedence order."""

        def sources_of(data: Dict[str, Any]) -> Dict[str, Any]:
            raw = ((data.get("tool") or {}).get("uv") or {}).get("sources") or {}
            return {pep503(k): v for k, v in raw.items()}

        yield pkg.directory, sources_of(pkg.manifest)
        for parent in self.ancestors_with(pkg.directory, "pyproject.toml"):
            data = self.toml(self._join(parent, "pyproject.toml")) or {}
            ws = ((data.get("tool") or {}).get("uv") or {}).get("workspace")
            if not isinstance(ws, dict):
                continue
            member_rel = os.path.relpath(self.abs(pkg.directory), self.abs(parent)).replace(
                os.sep, "/"
            )
            members = ws.get("members") or []
            excluded = ws.get("exclude") or []
            if any(fnmatch.fnmatch(member_rel, g) for g in members) and not any(
                fnmatch.fnmatch(member_rel, g) for g in excluded
            ):
                yield parent, sources_of(data)
            return  # uv uses the nearest workspace root only


def pep503(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def print_table(headers: List[str], rows: List[List[str]]) -> None:
    widths = [max(len(h), *(len(r[i]) for r in rows)) for i, h in enumerate(headers)]
    line = "  ".join(h.ljust(w) for h, w in zip(headers, widths))
    print(line)
    print("  ".join("-" * w for w in widths))
    for r in rows:
        print("  ".join(c.ljust(w) for c, w in zip(r, widths)))


def main(argv: Optional[List[str]] = None) -> int:
    default_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--root", default=default_root, help="repository root (default: %(default)s)"
    )
    parser.add_argument(
        "--strict", action="store_true", help="fail when a package has no declared licence"
    )
    parser.add_argument(
        "--include-dev", action="store_true", help="also check development-only edges"
    )
    parser.add_argument(
        "--verbose", action="store_true", help="list every package and internal edge"
    )
    args = parser.parse_args(argv)

    repo = Repo(args.root)
    repo.discover()
    all_edges = repo.edges()
    edges = [e for e in all_edges if args.include_dev or not e.kind.startswith("dev")]

    counts: Dict[str, int] = {}
    for p in repo.packages:
        counts[p.ecosystem] = counts.get(p.ecosystem, 0) + 1
    print(
        "Found %d packages (%s) and %d internal dependency edges (%d checked%s)."
        % (
            len(repo.packages),
            ", ".join("%s: %d" % kv for kv in sorted(counts.items())),
            len(all_edges),
            len(edges),
            "" if args.include_dev else "; development-only edges skipped",
        )
    )

    if args.verbose:
        print("\nPackages:")
        print_table(
            ["Ecosystem", "Package", "Licence", "Source"],
            [
                [p.ecosystem, p.label, p.license or "unknown", p.license_from or "-"]
                for p in repo.packages
            ],
        )
        if all_edges:
            print("\nInternal edges:")
            print_table(
                ["Ecosystem", "Package", "Depends on", "Kind"],
                [[e.source.ecosystem, e.source.label, e.target.label, e.kind] for e in all_edges],
            )

    violations = [e for e in edges if is_mit(e.source.license) and not is_allowed(e.target.license)]
    undeclared = [p for p in repo.packages if p.declared is None]

    for w in repo.warnings:
        print("warning: %s" % w)

    if undeclared:
        label = "error" if args.strict else "warning"
        print(
            "\n%s: %d packages have no declared licence (falling back to the nearest LICENSE file):"
            % (label, len(undeclared))
        )
        print_table(
            ["Ecosystem", "Package", "Fallback licence", "From"],
            [
                [p.ecosystem, p.label, p.license or "unknown", p.license_from or "none"]
                for p in undeclared
            ],
        )

    if violations:
        print(
            "\nerror: %d MIT packages depend on internal packages outside the allow-list:"
            % len(violations)
        )
        print_table(
            ["Ecosystem", "Package", "Licence", "Depends on", "Licence", "Kind"],
            [
                [
                    e.source.ecosystem,
                    e.source.label,
                    e.source.license or "unknown",
                    e.target.label,
                    e.target.license or "unknown",
                    e.kind,
                ]
                for e in violations
            ],
        )
        print(
            "\nAllowed licences for dependencies of MIT packages: %s" % ", ".join(sorted(ALLOWED))
        )
    else:
        print("\nNo licence direction violations.")

    if violations or (args.strict and undeclared):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
