"""Regression tests for the canonical cua-fleet PyPI release workflow."""

from pathlib import Path
import unittest

REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaFleetReleaseWiring(unittest.TestCase):
    """Keep Fleet's promotion workflow aligned with canonical SDK wheels."""

    def test_publisher_promotes_cua_fleet_0_1_14(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-py-fleet.yml").read_text()
        expected_sources = {
            "cua_fleet-0.1.16-py3-none-manylinux_2_34_x86_64.whl": "6d1b2b2095b5cb629a303cdc314b8223c1205a49b0647a0d50a966f9a572131e",
            "cua_fleet-0.1.16-py3-none-manylinux_2_34_aarch64.whl": "d29cc4cc2768ce2e9b48df9180ab1e544c5b659416f9d130af99f002fa9dd2f2",
            "cua_fleet-0.1.16-py3-none-macosx_10_12_x86_64.whl": "27a8621170df2a14d4af8912f902266a0cc32c1ed91ebb592f6ddb3bcf600bb1",
            "cua_fleet-0.1.16-py3-none-macosx_11_0_arm64.whl": "31d0373884f2d04dac8c3a88d06da7669491e5cb0c9d7c47ec839f8e42dc4e76",
            "cua_fleet-0.1.16-py3-none-win_amd64.whl": "ccd088f6a3d167a48f106518d24d3c601e3190d946f663f4c4ccec160b33ac8e",
        }

        self.assertIn("https://wheels.cua.ai/simple/cua-fleet/$WHEEL", workflow)
        self.assertNotIn("repackage_cua_train_wheel.py", workflow)
        self.assertNotIn("unsupported-windows", workflow)
        for wheel, digest in expected_sources.items():
            self.assertIn(f"wheel: {wheel}\n            sha256: {digest}", workflow)


if __name__ == "__main__":
    unittest.main()
