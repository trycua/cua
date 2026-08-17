"""Regression tests for the canonical cua-fleet PyPI release workflow."""

from pathlib import Path
import unittest

REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaFleetReleaseWiring(unittest.TestCase):
    """Keep Fleet's promotion workflow aligned with canonical SDK wheels."""

    def test_publisher_promotes_cua_fleet_0_1_12(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-py-fleet.yml").read_text()
        expected_sources = {
            "cua_fleet-0.1.12-py3-none-manylinux_2_34_x86_64.whl": "d8ef6a0c8ac6e6f8dda937e3aebeef7f5b4fa9e3f29edc55a602a1c874f3f96a",
            "cua_fleet-0.1.12-py3-none-manylinux_2_34_aarch64.whl": "0a39e52cbec94a2bc71f45282dd34c896fd154bc7f7a137fc6ceff82edfc0973",
            "cua_fleet-0.1.12-py3-none-macosx_10_12_x86_64.whl": "d5114ba97ef208e257bef261f6d3585b265f1f2c32cb627bcc9f44d753e60b75",
            "cua_fleet-0.1.12-py3-none-macosx_11_0_arm64.whl": "82762fae4943b20aae05b39362f5bd0c179f84c16dac1e948debb3d58db5adc2",
            "cua_fleet-0.1.12-py3-none-win_amd64.whl": "c280d7ceadedc1d5c4c59869940c3390aac321f12ac9c9ee52250f212b6aac75",
        }

        self.assertIn("https://wheels.cua.ai/simple/cua-fleet/$WHEEL", workflow)
        self.assertNotIn("repackage_cua_train_wheel.py", workflow)
        self.assertNotIn("unsupported-windows", workflow)
        for wheel, digest in expected_sources.items():
            self.assertIn(f"wheel: {wheel}\n            sha256: {digest}", workflow)


if __name__ == "__main__":
    unittest.main()
