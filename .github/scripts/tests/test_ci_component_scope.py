"""Regression fixtures for ci_component_scope.py (CUA-1910)."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ci_graph  # noqa: E402
from ci_component_scope import decide, decide_relay_e2e  # noqa: E402

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


class RelayE2eScopeTests(unittest.TestCase):
    """`relay e2e (docker)` starts only for the packages its closure reaches (CUA-3558)."""

    graph = ci_graph.load()

    def relay(self, files, event="pull_request", ref="feature/x"):
        return decide_relay_e2e(files, event, ref, self.graph)[0]

    def test_driver_only_changes_skip(self):
        self.assertFalse(self.relay([D + "cua-driver/src/main.rs"]))
        self.assertFalse(self.relay([D + "cua-driver-e2e/tests/a.rs"]))
        self.assertFalse(self.relay([D + "platform-macos/src/x.rs", D + "platform-windows/src/y.rs"]))

    def test_spacesd_crates_outside_the_closure_skip(self):
        self.assertFalse(self.relay(["libs/cua-spacesd/crates/cua-oidc/src/lib.rs"]))

    def test_relay_server_and_their_dependencies_run(self):
        for f in ("libs/cua-spacesd/crates/cua-relay/src/lib.rs",
                  "libs/cua-spacesd/crates/cua-spacesd-server/tests/conformance.rs",
                  D + "cua-driver-core/src/lib.rs",
                  D + "platform-linux/src/x.rs",
                  "libs/cua/crates/cua-mesh/src/lib.rs"):
            self.assertTrue(self.relay([f]), f)

    def test_lane_scripts_and_shared_inputs_run(self):
        for f in ("libs/cua-spacesd/scripts/ci/relay-e2e.sh", "libs/cua-spacesd/Cargo.lock",
                  ".github/workflows/ci-cua-spacesd.yml", ".github/scripts/ci_graph.py"):
            self.assertTrue(self.relay([f]), f)

    def test_other_events_and_release_prs_run(self):
        self.assertTrue(self.relay([D + "cua-driver/src/main.rs"], event="push"))
        self.assertTrue(self.relay([D + "cua-driver/src/main.rs"], ref="release-please--branches--main"))
        self.assertTrue(self.relay([]))
