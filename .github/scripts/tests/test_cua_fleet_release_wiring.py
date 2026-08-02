"""Regression tests for the canonical cua-fleet PyPI release workflow."""

from pathlib import Path
import unittest


REPO_ROOT = Path(__file__).resolve().parents[3]


class TestCuaFleetReleaseWiring(unittest.TestCase):
    """Keep Fleet's promotion workflow aligned with the published train wheel."""

    def test_publisher_promotes_cua_train_0_1_4(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/cd-py-fleet.yml").read_text()
        repackager = (REPO_ROOT / ".github/scripts/repackage_cua_train_wheel.py").read_text()
        expected_sources = {
            "cua_train-0.1.4-py3-none-manylinux_2_34_x86_64.whl": "8b2c20a603772408ad25629e5fa16bfc36bb81ed6a876fd99f1ccfab5c242252",
            "cua_train-0.1.4-py3-none-manylinux_2_34_aarch64.whl": "d0ea3e67a67c394ca05fad227ddd53987b53fbfbff907e43aa2ac6476f941df4",
            "cua_train-0.1.4-py3-none-macosx_10_12_x86_64.whl": "20e59a46ae5bc20bc48e34bc8b0c8b623682068be5c31b430477bdf57e946745",
            "cua_train-0.1.4-py3-none-macosx_11_0_arm64.whl": "cf5f4e26f77b4438849b5e6216829422980150439a88aa9a62a78dab74a95eb6",
        }

        self.assertIn('SOURCE_VERSION = "0.1.4"', repackager)
        for wheel, digest in expected_sources.items():
            self.assertIn(f"wheel: {wheel}\n            sha256: {digest}", workflow)


if __name__ == "__main__":
    unittest.main()
