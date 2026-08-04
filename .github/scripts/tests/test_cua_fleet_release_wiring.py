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
            "cua_train-0.1.5-py3-none-manylinux_2_34_x86_64.whl": "72824b317888d1bbf21585907c8291ce648007e052e92c1e4cd8d475750b57a5",
            "cua_train-0.1.5-py3-none-manylinux_2_34_aarch64.whl": "d6adca168fbd9777ef3fccf21f0d39b180cde440494f8f0ff7d47a46de2e6d5a",
            "cua_train-0.1.5-py3-none-macosx_10_12_x86_64.whl": "605cdd794edbd04a88f5a99f3713fd5b87fa8c634285839015a61894544cd1d1",
            "cua_train-0.1.5-py3-none-macosx_11_0_arm64.whl": "82ec2795fad892dc6238fcbcabb861e78d214f6603d765419a2b10fddef05fda",
        }

        self.assertIn('SOURCE_VERSION = "0.1.5"', repackager)
        for wheel, digest in expected_sources.items():
            self.assertIn(f"wheel: {wheel}\n            sha256: {digest}", workflow)


if __name__ == "__main__":
    unittest.main()
