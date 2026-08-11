"""Regression tests for the canonical cua-fleet PyPI release workflow."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaFleetReleaseWiring(unittest.TestCase):
    """Keep Fleet's promotion workflow aligned with canonical SDK wheels."""

    def test_publisher_promotes_cua_fleet_0_1_8(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-py-fleet.yml").read_text()
        expected_sources = {
            "cua_fleet-0.1.8-py3-none-manylinux_2_34_x86_64.whl": "0bac329552d351604ad15b7eb4f4f3ed944921083238d74f51277d657d8f0a34",
            "cua_fleet-0.1.8-py3-none-manylinux_2_34_aarch64.whl": "d7604668a7186d06cefb2bd23974b033c0b2cda61ccbe88d944af622b37f58d3",
            "cua_fleet-0.1.8-py3-none-macosx_10_12_x86_64.whl": "20611a14c185157e6f41502e1bbdcc1e98663347e94463ca538755b0efc173bb",
            "cua_fleet-0.1.8-py3-none-macosx_11_0_arm64.whl": "7c21d0a36a8d74bfe0768422e4f3d9367ee51837920e2c22ee57521fc93c4f3b",
            "cua_fleet-0.1.8-py3-none-win_amd64.whl": "d78fe6934a2f650dcf5b105a08833418245c720765da9c3b40e160b86a3d203d",
        }

        self.assertIn("https://wheels.cua.ai/simple/cua-fleet/$WHEEL", workflow)
        self.assertNotIn("repackage_cua_train_wheel.py", workflow)
        self.assertNotIn("unsupported-windows", workflow)
        for wheel, digest in expected_sources.items():
            self.assertIn(f"wheel: {wheel}\n            sha256: {digest}", workflow)


if __name__ == "__main__":
    unittest.main()
