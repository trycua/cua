"""Regression fixtures for ci_component_scope.py (CUA-1910)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from ci_component_scope import decide  # noqa: E402

D = "libs/cua-driver/rust/crates/"


def run(lane, files, event="pull_request", ref="feature/x"):
    return decide(lane, files, event, ref)


class ScopeTests(unittest.TestCase):
    def test_docs_only_skips(self):
        for lane in ("spacesd", "contract-clients"):
            self.assertFalse(run(lane, ["docs/content/docs/a.mdx"])[0])
            self.assertFalse(run(lane, ["libs/cua-driver/CHANGELOG.md"])[0])

    def test_spaces_app_only_skips(self):
        files = ["apps/cua-spaces-macos/Sources/A.swift", "libs/cua/crates/cua-spaces-app-core/src/lib.rs"]
        self.assertFalse(run("spacesd", files)[0])
        self.assertFalse(run("contract-clients", files)[0])

    def test_platform_crate_narrows_os(self):
        run_, labels, _ = run("spacesd", [D + "platform-linux/src/x.rs"])
        self.assertTrue(run_)
        self.assertEqual(labels, ["ubuntu-latest"])
        _, labels, _ = run("contract-clients", [D + "platform-macos/src/x.rs", D + "platform-windows/src/y.rs"])
        self.assertEqual(labels, ["macos-latest", "windows-latest"])

    def test_common_driver_crate_runs_all(self):
        run_, labels, _ = run("spacesd", [D + "cua-driver-core/src/lib.rs"])
        self.assertTrue(run_)
        self.assertEqual(len(labels), 3)

    def test_driver_crate_outside_closure_skips_spacesd(self):
        self.assertFalse(run("spacesd", [D + "cua-driver-e2e/tests/a.rs"])[0])
        self.assertFalse(run("spacesd", [D + "cua-driver/src/main.rs"])[0])

    def test_closure_crate_missing_from_old_filter_runs(self):
        self.assertTrue(run("spacesd", ["libs/cua/crates/cua-mesh/src/lib.rs"])[0])

    def test_shared_inputs_run_everything(self):
        for f in (".github/workflows/ci-cua-spacesd.yml", D + "../Cargo.lock", "libs/cua-driver/rust/Cargo.toml",
                  "libs/cua/proto/a.proto", ".github/scripts/ci_component_scope.py"):
            run_, labels, _ = run("spacesd", [D + "platform-linux/a.rs", f])
            self.assertTrue(run_, f)
            self.assertEqual(len(labels), 3, f)

    def test_full_matrix_for_release_and_main(self):
        self.assertEqual(len(run("spacesd", ["docs/a.md"], event="push")[1]), 3)
        self.assertEqual(len(run("spacesd", ["docs/a.md"], ref="release-please--branches--main")[1]), 3)

    def test_unknown_listing_fails_closed(self):
        self.assertTrue(run("spacesd", [])[0])


if __name__ == "__main__":
    unittest.main()
