"""Every pin of our own PyPI packages must resolve to a version on PyPI.

Pruning old releases from PyPI (2026-11-11) deleted versions that requirements
files still pinned, which broke CI. This test scans tracked files for
requirements on cua-driver, cua and cua-cli (exact pins, `~=`, ranges) and uv.lock
entries, and fails if no published version satisfies them.

Network test: skipped offline, or with CUA_PYPI_PIN_CHECK=0. Release lists are
cached in the temp dir for an hour.
"""

import json
import os
import re
import subprocess
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

try:
    from packaging.specifiers import InvalidSpecifier, SpecifierSet
    from packaging.version import InvalidVersion, Version
except ImportError:  # pragma: no cover
    try:
        from pip._vendor.packaging.specifiers import InvalidSpecifier, SpecifierSet
        from pip._vendor.packaging.version import InvalidVersion, Version
    except ImportError:
        SpecifierSet = None

REPO = Path(__file__).resolve().parents[3]
PACKAGES = ("cua-driver", "cua-cli", "cua")
CACHE_TTL = 3600

SPEC = r"(?:(?:===|==|~=|>=|<=|!=|<|>)\s*[0-9][A-Za-z0-9.*+!_-]*)"
REQ_RE = re.compile(
    r"(?<![A-Za-z0-9_.@/-])(cua[-_]driver|cua[-_]cli|cua)(?:\[[A-Za-z0-9_,-]+\])?"
    r"\s*(" + SPEC + r"(?:\s*,\s*" + SPEC + r")*)(?![A-Za-z0-9_-])"
)
LOCK_RE = re.compile(
    r'\[\[package\]\]\nname = "(cua-driver|cua-cli|cua)"\nversion = "([^"]+)"\nsource = \{ registry = "https://pypi\.org/simple" \}'
)
SKIP_PARTS = ("node_modules/", "/tests/", "/test/", "/fixtures/", "CHANGELOG")
SKIP_FILES = {"libs/cua-driver/rust/Cargo.lock"}
SCAN_SUFFIXES = {".txt", ".toml", ".md", ".mdx", ".yml", ".yaml", ".sh", ".py",
                 ".cfg", ".dockerfile", ""}


def tracked_files():
    out = subprocess.run(["git", "ls-files", "-z"], cwd=REPO, check=True,
                         capture_output=True).stdout.decode()
    for rel in filter(None, out.split("\0")):
        path = REPO / rel
        name = path.name
        scannable = (
            name == "uv.lock"
            or (path.suffix.lower() in SCAN_SUFFIXES and not name.endswith(".lock"))
            or name.startswith("Dockerfile")
        )
        if not scannable or rel in SKIP_FILES:
            continue
        if any(p in "/" + rel for p in SKIP_PARTS) or name.startswith("test_"):
            continue
        yield rel, path


def find_requirements():
    """Yield (relpath, line, package, specifier-or-version, is_lock)."""
    for rel, path in tracked_files():
        try:
            text = path.read_text()
        except (UnicodeDecodeError, OSError):
            continue
        if path.name == "uv.lock":
            for m in LOCK_RE.finditer(text):
                yield rel, text.count("\n", 0, m.start()) + 2, m.group(1), m.group(2), True
            continue
        for m in REQ_RE.finditer(text):
            pkg = m.group(1).replace("_", "-")
            yield rel, text.count("\n", 0, m.start()) + 1, pkg, m.group(2), False


def published_versions(pkg):
    cache = Path(tempfile.gettempdir()) / f"cua-pypi-pins-{pkg}.json"
    if cache.exists() and time.time() - cache.stat().st_mtime < CACHE_TTL:
        return json.loads(cache.read_text())
    with urllib.request.urlopen(f"https://pypi.org/pypi/{pkg}/json", timeout=20) as r:
        versions = sorted(json.load(r)["releases"])
    cache.write_text(json.dumps(versions))
    return versions


def unsatisfiable(pkg, spec, is_lock, versions):
    if is_lock:
        return spec not in versions
    try:
        sset = SpecifierSet(spec.replace(" ", ""))
    except InvalidSpecifier:
        return False
    for v in versions:
        try:
            if sset.contains(Version(v), prereleases=True):
                return False
        except InvalidVersion:
            continue
    return True


class PypiPinsExist(unittest.TestCase):
    def test_spec_regex_finds_pins(self):
        m = REQ_RE.search("pip install 'cua-driver[mcp]>=0.35.2,<0.40'")
        self.assertEqual((m.group(1), m.group(2)), ("cua-driver", ">=0.35.2,<0.40"))
        self.assertIsNone(REQ_RE.search("user-agent: cua-cli/0.9.0"))
        self.assertIsNone(REQ_RE.search("ssh cua@127.0.0.1"))
        self.assertIsNone(REQ_RE.search("@trycua/cua@0.4.2"))

    def test_our_pypi_pins_exist(self):
        if os.environ.get("CUA_PYPI_PIN_CHECK") == "0":
            self.skipTest("CUA_PYPI_PIN_CHECK=0")
        if SpecifierSet is None:
            self.skipTest("packaging is not installed")
        reqs = list(find_requirements())
        self.assertTrue(reqs, "scanner found no requirements; regex is broken")
        try:
            versions = {p: published_versions(p) for p in PACKAGES}
        except OSError as exc:
            self.skipTest(f"PyPI unreachable: {exc}")
        bad = [
            f"{rel}:{line}: {pkg} {spec}"
            for rel, line, pkg, spec, is_lock in reqs
            if unsatisfiable(pkg, spec, is_lock, versions[pkg])
        ]
        self.assertEqual(bad, [], "pins that no longer exist on PyPI (repoint them):\n" + "\n".join(bad))


if __name__ == "__main__":
    unittest.main()
