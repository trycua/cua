"""The cua-sandbox `driver` extra must cover the newest Cua Driver release.

The extra pins a range (`cua-driver>=0.37.0,<0.NN`). Cua Driver is 0.x, where a
minor may break the typed SDK, so the upper bound stays below the next minor
instead of being open. That only works if a new minor cannot ship without the
bound being raised: until 12 Nov 2026 the bound sat at `<0.48` while Driver
0.48.0 shipped, and `pip install cua-sandbox[driver]` stayed on 0.47.x.

The release-please manifest holds the version the Driver is released at. This
test fails when that version is outside the range, so the release PR that moves
the Driver to a new minor cannot merge before the bound is raised. Nobody raises
it by hand: release-please.yml notices the open Driver release PR and opens a
`fix(cua-sandbox)` PR that runs .github/scripts/sync_cua_sandbox_driver_bound.py,
gated by the cua-driver contract-clients CI. Offline; no network.
"""

import json
import tomllib
import unittest
from pathlib import Path

try:
    from packaging.requirements import Requirement
    from packaging.version import Version
except ImportError:  # CI images may only have pip's vendored copy
    from pip._vendor.packaging.requirements import Requirement
    from pip._vendor.packaging.version import Version

REPO = Path(__file__).resolve().parents[3]


def driver_requirement():
    pyproject = tomllib.loads(
        (REPO / "libs/python/cua-sandbox/pyproject.toml").read_text()
    )
    reqs = [
        Requirement(r)
        for r in pyproject["project"]["optional-dependencies"]["driver"]
    ]
    (req,) = [r for r in reqs if r.name == "cua-driver"]
    return req


class CuaSandboxDriverRange(unittest.TestCase):
    def test_manifest_driver_version_is_in_range(self):
        manifest = json.loads((REPO / ".release-please-manifest.json").read_text())
        latest = Version(manifest["libs/cua-driver"])
        req = driver_requirement()
        self.assertIn(
            latest,
            req.specifier,
            f"cua-driver {latest} is outside cua-sandbox's driver extra "
            f"({req.specifier}). The bound-bump PR (branch "
            "automation/cua-sandbox-driver-bound) should be open or merging; if it is "
            "not, run `python3 .github/scripts/sync_cua_sandbox_driver_bound.py "
            f"--driver-version {latest}` and open the PR.",
        )

    def test_upper_bound_is_not_open(self):
        req = driver_requirement()
        self.assertTrue(
            any(s.operator == "<" for s in req.specifier),
            "keep an upper bound: Driver 0.x minors can break the typed SDK",
        )


if __name__ == "__main__":
    unittest.main()
