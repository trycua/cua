"""Regression tests for the canonical cua-fleet PyPI release workflow."""

from pathlib import Path
import unittest

REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaFleetReleaseWiring(unittest.TestCase):
    """Keep Fleet's promotion workflow aligned with canonical SDK wheels."""

    def test_publisher_promotes_cua_fleet_0_1_17(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-py-fleet.yml").read_text()
        expected_sources = {
            "cua_fleet-0.1.17-py3-none-manylinux_2_34_x86_64.whl": "4cdbc89e850d841481713abf1e8bf9719b2e33b3c31f26bbea5f63e1ead3c6c0",
            "cua_fleet-0.1.17-py3-none-manylinux_2_34_aarch64.whl": "bda49298c77c65eb349e8420f9237f6b7fb2fd25203747df98a9513b98fd5d73",
            "cua_fleet-0.1.17-py3-none-macosx_10_12_x86_64.whl": "a8908b5c7f0f3c75d09b908b58867d43270f69d3726bb0d90316443f2265d0e3",
            "cua_fleet-0.1.17-py3-none-macosx_11_0_arm64.whl": "6aea577f21343326aeb06df03c07df65df8dd2f3b0097e02f25a7bc6b9d2c458",
            "cua_fleet-0.1.17-py3-none-win_amd64.whl": "ee4c139b1c0cebe1702114a34fd0fd60d216c5862d9fdc3472bb8b9879ac9eae",
        }

        self.assertIn("https://wheels.cua.ai/simple/cua-fleet/$WHEEL", workflow)
        self.assertNotIn("repackage_cua_train_wheel.py", workflow)
        self.assertNotIn("unsupported-windows", workflow)
        for wheel, digest in expected_sources.items():
            self.assertIn(f"wheel: {wheel}\n            sha256: {digest}", workflow)


if __name__ == "__main__":
    unittest.main()
