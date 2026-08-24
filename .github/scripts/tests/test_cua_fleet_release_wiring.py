"""Regression tests for the canonical cua-fleet PyPI release workflow."""

from pathlib import Path
import unittest

REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaFleetReleaseWiring(unittest.TestCase):
    """Keep Fleet's promotion workflow aligned with canonical SDK wheels."""

    def test_publisher_promotes_cua_fleet_0_1_14(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-py-fleet.yml").read_text()
        expected_sources = {
            "cua_fleet-0.1.14-py3-none-manylinux_2_34_x86_64.whl": "2dc70e98b3e8c691bf0fff0494b0a85d2405e872f1ca99dbfa2680473cea565b",
            "cua_fleet-0.1.14-py3-none-manylinux_2_34_aarch64.whl": "d6ea25902cf9ffc89779530b6ae7cff5413383ea7dc3c632a4fb47e00699a6d4",
            "cua_fleet-0.1.14-py3-none-macosx_10_12_x86_64.whl": "d370f83773574da8edcca080e9d86aec8eb7ed758d6c551b900482a8575f6810",
            "cua_fleet-0.1.14-py3-none-macosx_11_0_arm64.whl": "e19784c0ce8aa2d9a77bf096fc6ff10e990842f05c67bdfb96a16ecd5e7123e2",
            "cua_fleet-0.1.14-py3-none-win_amd64.whl": "3e73327b7c99dc95a4b4194628d3575a8707cab77d6929ed1bf34a82192a4464",
        }

        self.assertIn("https://wheels.cua.ai/simple/cua-fleet/$WHEEL", workflow)
        self.assertNotIn("repackage_cua_train_wheel.py", workflow)
        self.assertNotIn("unsupported-windows", workflow)
        for wheel, digest in expected_sources.items():
            self.assertIn(f"wheel: {wheel}\n            sha256: {digest}", workflow)


if __name__ == "__main__":
    unittest.main()
