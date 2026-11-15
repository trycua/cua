"""Policy: every package with a pnpm-lock.yaml pins its pnpm (packageManager), and the shim enforces it."""

from __future__ import annotations

import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
SHIM = REPO / "scripts/ci/lib/pnpm/pnpm"
PIN_RX = re.compile(r"pnpm@\d+\.\d+\.\d+(\+\S+)?")


def lockfile_dirs():
    out = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "--", "*pnpm-lock.yaml"],
        cwd=REPO, capture_output=True, text=True, check=True,
    ).stdout.split()
    return sorted({(REPO / f).parent for f in out if not re.search(r"(^|/)(node_modules|target)/", f)})


class PnpmPinPolicy(unittest.TestCase):
    def test_every_pnpm_lockfile_package_declares_package_manager(self):
        dirs = lockfile_dirs()
        self.assertTrue(dirs, "found no pnpm-lock.yaml: the discovery is broken")
        missing = []
        for d in dirs:
            pkg = d / "package.json"
            pin = json.loads(pkg.read_text(encoding="utf-8")).get("packageManager", "") if pkg.exists() else ""
            if not PIN_RX.fullmatch(pin):
                missing.append(f'{d.relative_to(REPO).as_posix() or "."}: packageManager={pin!r}')
        self.assertEqual(
            missing, [],
            'every package with a pnpm-lock.yaml needs "packageManager": "pnpm@x.y.z" in its package.json '
            "(bin/ci runs that exact pnpm through corepack)",
        )


@unittest.skipIf(os.name == "nt", "the shim is a POSIX sh script")
class PnpmShim(unittest.TestCase):
    def run_shim(self, package_json, *args):
        with tempfile.TemporaryDirectory() as tmp:
            if package_json is not None:
                Path(tmp, "package.json").write_text(json.dumps(package_json))
            return subprocess.run([str(SHIM), *args], cwd=tmp, capture_output=True, text=True)

    def test_fails_without_a_pin(self):
        r = self.run_shim({"name": "x"}, "--version")
        self.assertEqual(r.returncode, 127)
        self.assertIn("packageManager", r.stderr)

    def test_fails_when_the_pin_is_not_pnpm(self):
        r = self.run_shim({"name": "x", "packageManager": "yarn@4.0.0"}, "--version")
        self.assertEqual(r.returncode, 127)
        self.assertIn("not pnpm", r.stderr)

    def test_fails_when_the_pin_is_not_exact(self):
        r = self.run_shim({"name": "x", "packageManager": "pnpm@9"}, "--version")
        self.assertEqual(r.returncode, 127)
        self.assertIn("exact version", r.stderr)


if __name__ == "__main__":
    unittest.main()
