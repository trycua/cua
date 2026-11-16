"""Offline contract for the Cua Spaces driver pin automation."""

import json
import sys
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / ".github/scripts"))
import spaces_cua_driver_pin as pin  # noqa: E402

BUMP = yaml.safe_load((ROOT / ".github/workflows/release-spaces-bundle-cua-driver.yml").read_text())
CHECK = yaml.safe_load((ROOT / ".github/workflows/ci-cua-spaces-driver-pin.yml").read_text())


class PinFile(unittest.TestCase):
    def test_pin_is_self_consistent(self):
        p = json.loads(pin.PIN.read_text())
        v = p["version"]
        self.assertEqual(p["tag"], f"cua-driver-rs-v{v}")
        self.assertEqual(p["asset"], f"cua-driver-rs-{v}-darwin-universal-binary.tar.gz")
        self.assertRegex(p["sha256"], r"^[0-9a-f]{64}$")
        self.assertGreater(p["bytes"], 0)
        self.assertEqual(p["files"], ["cua-driver", "cua-cursor-theme"])

    def test_tags(self):
        self.assertEqual(pin.version_of("cua-driver-rs-v0.44.0"), (0, 44, 0))
        self.assertLess(pin.version_of("cua-driver-rs-v0.9.0"), pin.version_of("cua-driver-rs-v0.10.0"))
        for bad in ("cua-driver-rs-v0.44.0-nightly.20261108.1", "cua-spaces-v0.15.0", "cua-driver-v1.0.0"):
            with self.assertRaises(SystemExit):
                pin.version_of(bad)


class Workflows(unittest.TestCase):
    def on(self, wf):
        return wf.get("on") or wf[True]

    def test_bump_triggers(self):
        on = self.on(BUMP)
        self.assertEqual(on["release"]["types"], ["published"])
        self.assertIn("workflow_dispatch", on)
        self.assertIn("schedule", on)  # GITHUB_TOKEN-made releases do not fire `release`

    def test_bump_does_not_skip_prereleases(self):
        # Every published driver release carries the prerelease flag.
        self.assertNotIn("prerelease", json.dumps(BUMP["jobs"]["bundle"]["if"]))

    def test_bump_uses_app_token_and_queues_automerge(self):
        text = (ROOT / ".github/workflows/release-spaces-bundle-cua-driver.yml").read_text()
        self.assertIn("create-github-app-token", text)
        self.assertIn("gh pr merge --auto --squash", text)
        self.assertIn("gh pr edit", text)  # updates the open PR, never a second one

    def test_check_runs_on_schedule(self):
        self.assertIn("schedule", self.on(CHECK))


if __name__ == "__main__":
    unittest.main()
