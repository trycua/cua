"""Headless contract check for the separately applied read-only proof patch."""
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


class BridgeContract(unittest.TestCase):
    def test_additive_v1_bridge_has_reload_generation_and_exact_uuid(self):
        patch = pathlib.Path(__file__).with_name("helper-identity.patch")
        self.assertTrue(patch.exists(), "feasibility identity bridge is missing")
        with tempfile.TemporaryDirectory() as directory:
            target = pathlib.Path(directory) / "libs/cua-driver/kwin-target-helper"
            target.mkdir(parents=True)
            source = ROOT / "libs/cua-driver/kwin-target-helper/kwin_target_helper.cpp"
            (target / source.name).write_bytes(source.read_bytes())
            subprocess.run(["git", "apply", str(patch)], cwd=directory, check=True)
            result = (target / source.name).read_text()
            self.assertIn("kProtocolVersion = 1", result)
            self.assertIn("Q_SCRIPTABLE QString GetIdentitySnapshot()", result)
            self.assertIn('record["internal_id"]', result)
            self.assertIn('result["generation"] = m_generation', result)
            self.assertIn("const QString m_generation = QUuid::createUuid()", result)
            self.assertNotIn("activateWindow(", result)
            self.assertNotIn("activeWindow =", result)


if __name__ == "__main__":
    unittest.main()
