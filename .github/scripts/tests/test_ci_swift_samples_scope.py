"""Regression fixtures for ci_swift_samples_scope.py and the two workflows that use it."""

import sys
import tempfile
import unittest
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ci_swift_samples_scope as scope  # noqa: E402

ROOT = Path(__file__).resolve().parents[3]
WORKFLOWS = ROOT / ".github/workflows"
LANE_WORKFLOWS = {
    "infinite-canvas": "ci-infinite-canvas-swift.yml",
    "openkoalabot": "ci-openkoalabot-example-samples.yml",
}
LANES = tuple(LANE_WORKFLOWS)


def run(lane, files, event="pull_request", ref="feature/x", labels=()):
    return scope.decide(lane, files, event, ref, tuple(labels))[0]


class ScopeTests(unittest.TestCase):
    def test_rust_internals_skip(self):
        # Everything outside the SDK crate: the daemon, vmm, spaces, host, the CLI, the apps' cores.
        for lane in LANES:
            for f in (
                "libs/cua/crates/cua-daemon/src/lib.rs",
                "libs/cua/crates/cua-vmm/src/managed.rs",
                "libs/cua/crates/cua-spaces/src/lib.rs",
                "libs/cua/crates/cua-host/src/driver.rs",
                "libs/cua/crates/cua-cli/src/main.rs",
                "libs/cua/crates/cua-spaces-app-core/src/lib.rs",
                "libs/cua/crates/cua-spaces-ffi/src/lib.rs",
                "libs/cua/python/src/cua/__init__.py",
                "libs/cua/typescript/src/index.ts",
                "libs/cua/kotlin/build.gradle.kts",
                "libs/cua/VERSION",
            ):
                self.assertFalse(run(lane, [f]), (lane, f))

    def test_docs_and_other_samples_skip(self):
        for lane in LANES:
            self.assertFalse(run(lane, ["docs/content/docs/spaces/guides/a.mdx"]), lane)
            self.assertFalse(run(lane, ["examples/openkoalabot-example-ts/src/a.ts"]), lane)
            self.assertFalse(run(lane, ["examples/openkoalabot-example-tauri/src-tauri/src/main.rs"]), lane)
        self.assertFalse(run("infinite-canvas", ["examples/openkoalabot-example-swift/Package.swift"]))
        self.assertFalse(run("openkoalabot", ["examples/infinite-canvas-swift/Package.swift"]))

    def test_shared_dependencies_run_both(self):
        for lane in LANES:
            for f in (
                "libs/cua/swift/Sources/CuaSDK/CuaSDK.swift",
                "libs/cua/swift/Package.swift",
                "libs/spaces-sdk-swift/Sources/CuaSpaces/Space.swift",
                "libs/spaces-app-swift/Sources/CuaSpacesStreaming/View.swift",
                "libs/cua/crates/cua-sdk/src/lib.rs",
                "libs/cua/crates/cua-sdk/uniffi.toml",
                "libs/cua/crates/cua-proto/build.rs",
                "libs/cua/proto/cua/env/v1/stream.proto",
                "libs/cua/scripts/stage-uniffi-library.mjs",
                "libs/cua/Cargo.toml",
                "libs/cua/Cargo.lock",
                "libs/cua/rust-toolchain.toml",
                "libs/cua/.cargo/config.toml",
                ".github/scripts/ci_swift_samples_scope.py",
            ):
                self.assertTrue(run(lane, ["docs/a.mdx", f]), (lane, f))

    def test_lane_specific_inputs(self):
        self.assertTrue(run("infinite-canvas", ["examples/infinite-canvas-swift/Sources/CanvasModel/Camera.swift"]))
        self.assertTrue(run("infinite-canvas", ["libs/cua-driver/rust/crates/cursor-overlay/assets/cua.default.lottie"]))
        self.assertTrue(run("infinite-canvas", [".github/workflows/ci-infinite-canvas-swift.yml"]))
        self.assertFalse(run("infinite-canvas", [".github/workflows/ci-openkoalabot-example-samples.yml"]))
        self.assertFalse(run("openkoalabot", ["libs/cua-driver/rust/crates/cursor-overlay/assets/cua.default.lottie"]))
        for f in (
            "examples/openkoalabot-example-swift/Sources/OpenKoalaBotExample/App/A.swift",
            "examples/openkoalabot-example-scenario/scenario.json",
            "libs/images/sandbox-images.json",
            ".github/workflows/ci-openkoalabot-example-samples.yml",
        ):
            self.assertTrue(run("openkoalabot", [f]), f)
        self.assertFalse(run("openkoalabot", [".github/workflows/ci-infinite-canvas-swift.yml"]))
        self.assertFalse(run("infinite-canvas", ["libs/images/sandbox-images.json"]))

    def test_markdown_is_inert_but_only_markdown(self):
        for lane in LANES:
            self.assertFalse(run(lane, ["libs/cua/swift/README.md", "libs/spaces-sdk-swift/README.md"]), lane)
            self.assertTrue(run(lane, ["libs/cua/swift/README.md", "libs/cua/swift/Package.swift"]), lane)
        self.assertFalse(run("openkoalabot", ["examples/openkoalabot-example-swift/FRICTION.md"]))

    def test_full_build_everywhere_else(self):
        for lane in LANES:
            for event in ("push", "schedule", "workflow_dispatch"):
                self.assertTrue(run(lane, ["docs/a.md"], event=event), (lane, event))
            self.assertTrue(run(lane, ["docs/a.md"], ref="release-please--branches--main"), lane)
            self.assertTrue(run(lane, ["docs/a.md"], labels=["ci:full"]), lane)
            self.assertFalse(run(lane, ["docs/a.md"], labels=["agent", "bug"]), lane)

    def test_unknown_or_truncated_listing_fails_closed(self):
        for lane in LANES:
            self.assertTrue(run(lane, []), lane)
            self.assertTrue(run(lane, ["docs/a.md"] * scope.FILE_LISTING_LIMIT), lane)

    def test_labels_from_env(self):
        import os

        for raw, expected in (
            ('["ci:full", "x"]', ("ci:full", "x")),
            ("[]", ()),
            ("null", ()),
            ("", ()),
            ("ci:full, x", ("ci:full", "x")),
        ):
            os.environ["LABELS"] = raw
            self.assertEqual(scope.labels_from_env(), expected, raw)
        del os.environ["LABELS"]


class ManifestTests(unittest.TestCase):
    def test_repository_matches_the_map(self):
        for lane in LANES:
            self.assertEqual(scope.manifest_problems(lane), [], lane)

    def test_closure_follows_path_dependencies(self):
        for lane in LANES:
            closure = set(scope.package_closure(lane))
            self.assertEqual(
                closure,
                {scope.LANES[lane]["sample"], "libs/cua/swift", "libs/spaces-sdk-swift", "libs/spaces-app-swift"},
                lane,
            )

    def test_a_new_path_dependency_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sample = scope.LANES["openkoalabot"]["sample"]
            for directory, deps in {
                sample: ['.package(path: "../../libs/spaces-sdk-swift")', '.package(name: "New", path: "../../libs/new-swift")'],
                "libs/spaces-sdk-swift": ['.package(name: "Cua", path: "../cua/swift")'],
                "libs/cua/swift": [],
                "libs/new-swift": [],
            }.items():
                (root / directory).mkdir(parents=True)
                (root / directory / "Package.swift").write_text("dependencies: [\n" + ",\n".join(deps) + "\n]")
            problems = scope.manifest_problems("openkoalabot", root)
            self.assertIn("libs/new-swift is a Swift package dependency but is not in the openkoalabot inputs", problems)
            self.assertFalse([p for p in problems if "does not exist" not in p and "new-swift" not in p], problems)

    def test_missing_input_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sample = scope.LANES["infinite-canvas"]["sample"]
            (root / sample).mkdir(parents=True)
            (root / sample / "Package.swift").write_text("dependencies: []")
            problems = scope.manifest_problems("infinite-canvas", root)
            self.assertIn("libs/cua/crates/cua-sdk/ does not exist", problems)


class WorkflowWiringTests(unittest.TestCase):
    @staticmethod
    def load(lane):
        return yaml.safe_load((WORKFLOWS / LANE_WORKFLOWS[lane]).read_text())

    @staticmethod
    def pattern_covers(pattern, path):
        prefix = pattern[:-3] if pattern.endswith("/**") else pattern
        return path.rstrip("/") == prefix or path.startswith(prefix + "/")

    def test_trigger_paths_cover_every_input(self):
        # An input the workflow's own `paths:` filter does not admit can never run.
        for lane in LANES:
            on = self.load(lane)[True]
            for event in ("pull_request", "push"):
                for item in scope.LANES[lane]["inputs"]:
                    self.assertTrue(
                        any(self.pattern_covers(p, item) for p in on[event]["paths"]),
                        (lane, event, item),
                    )

    def test_full_run_on_main_and_nightly(self):
        for lane in LANES:
            on = self.load(lane)[True]
            self.assertEqual(on["push"]["branches"], ["main"])
            self.assertEqual(len(on["schedule"]), 1)
            self.assertRegex(on["schedule"][0]["cron"], r"^\d+ \d+ \* \* \*$")
            self.assertIn("workflow_dispatch", on)

    def test_macos_job_waits_for_the_scope_job(self):
        for lane in LANES:
            jobs = self.load(lane)["jobs"]
            changes = jobs["changes"]
            self.assertEqual(changes["runs-on"], "ubuntu-24.04")
            self.assertEqual(changes["outputs"], {"run": "${{ steps.scope.outputs.run }}"})
            script = changes["steps"][-1]["run"]
            self.assertIn(f"check-manifests {lane}", script)
            self.assertIn(f"scope {lane}", script)
            swift = jobs["swift"]
            self.assertEqual(swift["needs"], "changes")
            self.assertIn("needs.changes.outputs.run == 'true'", swift["if"])
            self.assertIn("release-please--branches--", swift["if"])
            self.assertEqual(swift["runs-on"], "macos-26")

    def test_typescript_job_is_not_scoped(self):
        jobs = self.load("openkoalabot")["jobs"]
        self.assertNotIn("needs", jobs["typescript"])
        self.assertNotIn("changes.outputs", jobs["typescript"]["if"])

    def test_no_other_job_in_the_infinite_canvas_workflow(self):
        self.assertEqual(set(self.load("infinite-canvas")["jobs"]), {"changes", "swift"})


if __name__ == "__main__":
    unittest.main()
