"""Regression tests for the canonical cua-fleet PyPI release workflow."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaFleetReleaseWiring(unittest.TestCase):
    """Keep Fleet's promotion workflow aligned with canonical SDK wheels."""

    def test_publisher_promotes_cua_fleet_0_1_11(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-py-fleet.yml").read_text()
        expected_sources = {
            "cua_fleet-0.1.11-py3-none-manylinux_2_34_x86_64.whl": "0539a40aac915368362dd2f3e90d4b6d233e7bece77d13a5733d8cecb7298731",
            "cua_fleet-0.1.11-py3-none-manylinux_2_34_aarch64.whl": "c76052d9cb1710936a59709fd5a2894fe9fb43fe89e2a0313ad0b88faf4c2b1e",
            "cua_fleet-0.1.11-py3-none-macosx_10_12_x86_64.whl": "5884a26388b44e9cf59314d20b9aae34f278c48b108d927ebaf802b8d5b7d7f6",
            "cua_fleet-0.1.11-py3-none-macosx_11_0_arm64.whl": "1f6d1532f76166fe30001c68765c4f3fcb94e9b713751f7a009d3780f523aa9c",
            "cua_fleet-0.1.11-py3-none-win_amd64.whl": "da8e1c40abcd3fee5cdab48801d4bd43a277ecddb7afebcba3d9885d9ec5d431",
        }

        self.assertIn("https://wheels.cua.ai/simple/cua-fleet/$WHEEL", workflow)
        self.assertNotIn("repackage_cua_train_wheel.py", workflow)
        self.assertNotIn("unsupported-windows", workflow)
        for wheel, digest in expected_sources.items():
            self.assertIn(f"wheel: {wheel}\n            sha256: {digest}", workflow)


if __name__ == "__main__":
    unittest.main()
