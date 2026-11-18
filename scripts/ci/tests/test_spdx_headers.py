"""Policy: scripts/spdx-headers.py never stamps a file that carries another party's notice."""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
VENDORED = "// Copyright (c) 2019 Some Upstream Author\n// Licensed under the Apache License, Version 2.0\n\npub fn f() {}\n"
COLON = "// Copyright: Some Upstream Author\n\npub fn h() {}\n"
OWN = 'pub const COPYRIGHT: &str = "2026";\nlet total = copyright + 1;\n'


class DriverTreeTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.repo = Path(tmp.name)
        (self.repo / "scripts").mkdir()
        shutil.copy(REPO / "scripts/spdx-headers.py", self.repo / "scripts/spdx-headers.py")
        src = self.repo / "libs/cua-driver/src"
        src.mkdir(parents=True)
        self.vendored = src / "vendored.rs"
        self.own = src / "own.rs"
        self.vendored.write_text(VENDORED, encoding="utf-8")
        self.colon = src / "colon.rs"
        self.colon.write_text(COLON, encoding="utf-8")
        self.own.write_text(OWN, encoding="utf-8")
        subprocess.run(["git", "init", "-q"], cwd=self.repo, check=True)
        subprocess.run(["git", "add", "-A"], cwd=self.repo, check=True)

    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, str(self.repo / "scripts/spdx-headers.py"), *args],
            cwd=self.repo, capture_output=True, text=True,
        )

    def test_apply_leaves_a_vendored_file_alone_and_stamps_our_own(self):
        result = self.run_script("--apply")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.vendored.read_text(encoding="utf-8"), VENDORED)
        self.assertEqual(self.colon.read_text(encoding="utf-8"), COLON)
        self.assertEqual(
            self.own.read_text(encoding="utf-8"),
            "// SPDX-License-Identifier: MIT\n// Copyright (c) 2026 Cua AI, Inc.\n\n" + OWN,
        )
        self.assertIn("libs/cua-driver/src/vendored.rs", result.stdout.split("left alone")[-1])

    def test_check_does_not_ask_for_a_header_on_a_vendored_file(self):
        result = self.run_script("--check")
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing header: libs/cua-driver/src/own.rs", result.stdout)
        self.assertEqual(result.stdout.count("missing header:"), 1, result.stdout)


if __name__ == "__main__":
    unittest.main()
