"""Regression tests for the canonical cua-fleet PyPI release workflow."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaFleetReleaseWiring(unittest.TestCase):
    """Keep Fleet's promotion workflow aligned with the published train wheel."""

    def test_publisher_promotes_cua_train_0_1_5(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-py-fleet.yml").read_text()
        repackager = (REPO_ROOT / ".github/scripts/repackage_cua_train_wheel.py").read_text()
        expected_sources = {
            "cua_train-0.1.6-py3-none-manylinux_2_34_x86_64.whl": "418db974949f4020fae2f3ed2f5a8d0db6ddce38e34f5db89283ac479d816a55",
            "cua_train-0.1.6-py3-none-manylinux_2_34_aarch64.whl": "80526bf94ed535383b3dddd9a7da800da9fcb32837c21c2e2caa39390e3548c9",
            "cua_train-0.1.6-py3-none-macosx_10_12_x86_64.whl": "87cb4220054514ac5f44118d490ae404fbca5c97d2b964cd4fcaf1facd391fc0",
            "cua_train-0.1.6-py3-none-macosx_11_0_arm64.whl": "17563c0504c9270e570332924f3092faec3dcdbf48557a70ee830e165ac7751c",
        }

        self.assertIn('SOURCE_VERSION = "0.1.6"', repackager)
        for wheel, digest in expected_sources.items():
            self.assertIn(f"wheel: {wheel}\n            sha256: {digest}", workflow)


if __name__ == "__main__":
    unittest.main()
