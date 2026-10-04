"""attest-doctor-reports.sh ledger_checkout: a new ledger branch starts empty."""

from __future__ import annotations

import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "..", "attest-doctor-reports.sh")


def git(*args: str, cwd: str) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


class LedgerCheckoutTests(unittest.TestCase):
    def test_first_ledger_commit_holds_only_the_ledger(self) -> None:
        with tempfile.TemporaryDirectory() as d:
            origin, repo, attest = (os.path.join(d, n) for n in ("origin.git", "repo", "attest"))
            git("init", "-q", "--bare", origin, cwd=d)
            git("init", "-q", repo, cwd=d)
            for name in ("README.md", "src/lib.rs"):
                os.makedirs(os.path.dirname(os.path.join(repo, name)) or repo, exist_ok=True)
                with open(os.path.join(repo, name), "w") as fh:
                    fh.write("repository file\n")
            git("add", "-A", cwd=repo)
            git("-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-qm", "init", cwd=repo)
            git("remote", "add", "origin", origin, cwd=repo)
            # Only the function under test, with the repository it acts on.
            with open(SCRIPT) as fh:
                lines = fh.read().splitlines()
            start = lines.index("ledger_checkout() {")
            end = lines.index("}", start)
            remove = lines.index("ledger_remove() {")
            body = "\n".join(lines[start : end + 1] + lines[remove : lines.index("}", remove) + 1])
            out = subprocess.run(
                ["bash", "-c", f'set -euo pipefail\n{body}\nledger_checkout'],
                cwd=repo,
                env={**os.environ, "REPO_ROOT": repo, "ATTEST": attest, "LEDGER_BRANCH": "image-doctor-ledger"},
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()
            self.assertEqual(sorted(os.listdir(out)), [".git", "README.md"])
            git("add", "-A", cwd=out)
            self.assertEqual(git("ls-files", cwd=out).split(), ["README.md"])
            with open(os.path.join(out, "README.md")) as fh:
                self.assertIn("image-doctor ledger", fh.read())


if __name__ == "__main__":
    unittest.main()
