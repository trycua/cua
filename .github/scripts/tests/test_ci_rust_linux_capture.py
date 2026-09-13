"""Execute the capture CI shell with synthetic commands and no desktop."""

import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github/workflows/ci-rust-linux.yml"


def capture_script():
    steps = yaml.safe_load(WORKFLOW.read_text())["jobs"]["unit"]["steps"]
    return next(step["run"] for step in steps if step.get("name") ==
                "Run X11 MIT-SHM capture correctness and performance regression")


class CaptureCiTests(unittest.TestCase):
    def run_script(self, script, cargo_status):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            commands = {
                "Xvfb": "exec sleep 30",
                "xdpyinfo": "exit 0",
                "git": "printf '%040d\\n' 1",
                "cargo": f"printf 'synthetic capture result\\n'; exit {cargo_status}",
                "sha256sum": "exit 0",
            }
            for name, body in commands.items():
                path = root / name
                path.write_text("#!/bin/sh\n" + body + "\n")
                path.chmod(0o700)
            # GitHub's implicit bash runner uses -e, not pipefail. Execute the
            # actual step, including its child-process cleanup and tee pipeline.
            result = subprocess.run(["bash", "--noprofile", "--norc", "-e", "-c", script],
                cwd=root, env={**os.environ, "PATH": str(root) + os.pathsep + os.environ["PATH"]},
                text=True, capture_output=True, timeout=10)
            self.assertIn("synthetic capture result", (root / "x11-capture-evidence.log").read_text())
            return result.returncode

    def test_capture_failure_is_not_hidden_by_tee(self):
        self.assertNotEqual(self.run_script(capture_script(), 23), 0)

    def test_capture_success_succeeds(self):
        self.assertEqual(self.run_script(capture_script(), 0), 0)

    def test_regression_reproduces_the_prior_false_green(self):
        old = capture_script().replace("set -euo pipefail\n", "")
        self.assertEqual(self.run_script(old, 23), 0)

    def test_readiness_probes_do_not_reset_disposable_servers(self):
        servers = [shlex.split(line) for line in capture_script().splitlines()
                   if line.startswith("Xvfb ")]
        self.assertEqual(len(servers), 2)
        self.assertTrue(all("-noreset" in command for command in servers))
        source = (ROOT / "libs/cua-driver/rust/crates/platform-linux/src/capture.rs").read_text()
        helper = source.split("impl XvfbServer {", 1)[1].split("fn stop(", 1)[0]
        self.assertIn('"-noreset"', helper)


if __name__ == "__main__":
    unittest.main()
