"""Regression fixtures for ci_swift_samples_scope.py and the two workflows that use it."""

import json
import os
import re
import shutil
import subprocess
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


TYPESCRIPT = scope.LANES["openkoalabot"]["extra"]["typescript"]


def run(lane, files, event="pull_request", labels=()):
    return scope.decide(lane, files, event, tuple(labels))[0]


def run_typescript(files, event="pull_request", labels=()):
    return scope.decide("openkoalabot", files, event, tuple(labels), TYPESCRIPT)[0]


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
                ".github/actions/setup-protoc/action.yml",
                ".github/actions/rust-cache/action.yml",
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
            self.assertTrue(run(lane, ["docs/a.md"], labels=["ci:full"]), lane)
            self.assertFalse(run(lane, ["docs/a.md"], labels=["agent", "bug"]), lane)

    def test_rename_out_of_a_mapped_directory_counts(self):
        # The files API lists a rename under its new name and carries the old one
        # as previous_filename; the workflows feed the scope both.
        for lane, moved in (
            ("infinite-canvas", "examples/infinite-canvas-swift/Sources/CanvasModel/Camera.swift"),
            ("openkoalabot", "examples/openkoalabot-example-swift/Sources/OpenKoalaBotExample/App/A.swift"),
            ("infinite-canvas", "libs/cua/swift/Sources/Cua/Cua.swift"),
            ("openkoalabot", "libs/spaces-sdk-swift/Sources/CuaSpaces/Space.swift"),
        ):
            self.assertFalse(run(lane, ["docs/content/docs/moved/Camera.swift"]), (lane, moved))
            self.assertTrue(run(lane, ["docs/content/docs/moved/Camera.swift", moved]), (lane, moved))

    def test_workflows_list_both_names_of_a_rename(self):
        jq = shutil.which("jq")
        if jq is None:
            self.skipTest("jq is not installed")
        payload = json.dumps([
            {"filename": "docs/moved/Camera.swift", "status": "renamed",
             "previous_filename": "examples/infinite-canvas-swift/Sources/CanvasModel/Camera.swift"},
            {"filename": "docs/a.md", "status": "modified"},
        ])
        for lane in LANES:
            script = WorkflowWiringTests.load(lane)["jobs"]["changes"]["steps"][-1]["run"]
            expression = re.search(r"--jq '([^']+)'", script).group(1)
            listed = subprocess.run([jq, "-r", expression], input=payload, capture_output=True, text=True, check=True)
            files = listed.stdout.split()
            self.assertEqual(files, ["docs/moved/Camera.swift", "examples/infinite-canvas-swift/Sources/CanvasModel/Camera.swift", "docs/a.md"])
            self.assertTrue(run(lane, files) == (lane == "infinite-canvas"), lane)

    def test_typescript_job_keeps_its_old_trigger_paths(self):
        for f in (
            "examples/openkoalabot-example-ts/src/a.ts",
            "libs/cua/crates/cua-daemon/src/lib.rs",
            "libs/cua/README.md",
            "docs/content/docs/spaces/guides/a.mdx",
            "docs/content/docs/spaces/examples/a.mdx",
            "libs/spaces-sdk-swift/Package.swift",
            ".github/actions/rust-cache/action.yml",
            ".github/workflows/ci-openkoalabot-example-samples.yml",
        ):
            self.assertTrue(run_typescript([f]), f)
        for f in ("docs/content/docs/other.mdx", "apps/cua-spaces/src/a.ts", "examples/infinite-canvas-swift/Package.swift"):
            self.assertFalse(run_typescript([f]), f)
        self.assertTrue(run_typescript(["docs/a.md"], event="push"))
        self.assertTrue(run_typescript(["docs/a.md"], labels=["ci:full"]))
        self.assertTrue(run_typescript([]))

    def test_unknown_or_truncated_listing_fails_closed(self):
        for lane in LANES:
            self.assertTrue(run(lane, []), lane)
            self.assertTrue(run(lane, ["docs/a.md"] * scope.FILE_LISTING_LIMIT), lane)

    def test_labels_from_env(self):
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


class CommandTests(unittest.TestCase):
    def scope(self, lane, files, **env):
        output = Path(tempfile.mkdtemp()) / "out"
        completed = subprocess.run(
            [sys.executable, str(ROOT / ".github/scripts/ci_swift_samples_scope.py"), "scope", lane],
            input="\n".join(files), capture_output=True, text=True, check=True,
            env={**os.environ, "GITHUB_OUTPUT": str(output), "LABELS": "[]", **env},
        )
        written = dict(line.split("=", 1) for line in output.read_text().splitlines())
        shutil.rmtree(output.parent)
        self.assertEqual(completed.stdout.splitlines(), [f"{k}={v}" for k, v in written.items()])
        return written

    def test_outputs(self):
        out = self.scope("openkoalabot", ["libs/cua/crates/cua-daemon/src/lib.rs"], EVENT="pull_request")
        self.assertEqual((out["run"], out["typescript"]), ("false", "true"))
        out = self.scope("openkoalabot", ["docs/other.md"], EVENT="pull_request")
        self.assertEqual((out["run"], out["typescript"]), ("false", "false"))
        out = self.scope("openkoalabot", ["docs/other.md"], EVENT="push")
        self.assertEqual((out["run"], out["typescript"]), ("true", "true"))
        out = self.scope("infinite-canvas", ["docs/other.md"], EVENT="pull_request", LABELS='["ci:full"]')
        self.assertEqual(out, {"run": "true", "reason": "ci:full label"})


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

    def test_no_event_level_path_filter(self):
        # A `paths:` filter would stop the `ci:full` label, unrelated pushes to
        # main and renames out of a mapped directory from ever reaching the scope job.
        for lane in LANES:
            on = self.load(lane)[True]
            for event in ("pull_request", "push"):
                self.assertNotIn("paths", on[event] or {}, (lane, event))
                self.assertNotIn("paths-ignore", on[event] or {}, (lane, event))

    def test_actions_both_builds_run_are_inputs(self):
        for lane in LANES:
            self.assertTrue(scope.covered(lane, ".github/actions/setup-protoc/action.yml"), lane)
            self.assertTrue(scope.covered(lane, ".github/actions/rust-cache/action.yml"), lane)
            jobs = self.load(lane)["jobs"]
            uses = [s.get("uses") for j in jobs.values() for s in j.get("steps", [])]
            self.assertIn("./.github/actions/setup-protoc", uses)
            self.assertIn("./.github/actions/rust-cache", uses)

    def test_full_run_on_main_and_nightly(self):
        for lane in LANES:
            on = self.load(lane)[True]
            self.assertEqual(on["push"], {"branches": ["main"]})
            self.assertEqual(len(on["schedule"]), 1)
            self.assertRegex(on["schedule"][0]["cron"], r"^\d+ \d+ \* \* \*$")
            self.assertIn("workflow_dispatch", on)

    @staticmethod
    def evaluate(condition, **context):
        """Evaluate the small subset of the expression language the job conditions use."""
        expression = re.fullmatch(r"\$\{\{\s*(.*?)\s*\}\}", condition.strip()).group(1)
        expression = re.sub(r"startsWith\(([\w.]+), ('[^']*')\)", r"\1.startswith(\2)", expression)
        expression = expression.replace("&&", " and ").replace("||", " or ").replace("!", " not ")
        for name, key in (("needs.changes.outputs.run", "run"), ("needs.changes.outputs.typescript", "typescript"),
                          ("github.head_ref", "head_ref"), ("github.event_name", "event_name")):
            expression = expression.replace(name, key)
        return eval(expression, {"__builtins__": {}}, context)

    def test_macos_job_waits_for_the_scope_job(self):
        for lane in LANES:
            jobs = self.load(lane)["jobs"]
            changes = jobs["changes"]
            self.assertEqual(changes["runs-on"], "ubuntu-24.04")
            self.assertEqual(changes["outputs"]["run"], "${{ steps.scope.outputs.run }}")
            script = changes["steps"][-1]["run"]
            self.assertIn(f"check-manifests {lane}", script)
            self.assertIn(f"scope {lane}", script)
            self.assertNotIn("needs", changes)
            self.assertNotIn("if", changes)
            swift = jobs["swift"]
            self.assertEqual(swift["needs"], "changes")
            # The arm64 larger runner of the same macOS image (macos-26 is 3-core).
            # MAC_RUNNER_LABELS moves it to the self-hosted Macs; unset keeps the larger runner.
            self.assertIn("vars.MAC_RUNNER_LABELS || '\"macos-26-xlarge\"'", swift["runs-on"])
            self.assertIn("github.event.pull_request.head.repo.full_name == github.repository", swift["runs-on"])

    def test_effective_job_conditions(self):
        # The scope decision is final, except that release-please branches skip the
        # job as they did before this change (release pull requests get the Swift
        # binding from ci-cua-sdk.yml and e2b-bindings.yml).
        for lane in LANES:
            jobs = self.load(lane)["jobs"]
            for job, output in (("swift", "run"), ("typescript", "typescript")):
                if job not in jobs:
                    continue
                for head_ref in ("feature/x", "", "release-please--branches--main"):
                    for decision in ("true", "false", ""):
                        context = {"run": "", "typescript": "", "head_ref": head_ref, output: decision}
                        expected = decision == "true" and not head_ref.startswith("release-please--branches--")
                        self.assertEqual(self.evaluate(jobs[job]["if"], **context), expected, (lane, job, head_ref, decision))
        self.assertEqual(self.load("openkoalabot")["jobs"]["typescript"]["needs"], "changes")

    def test_no_other_job_in_the_infinite_canvas_workflow(self):
        self.assertEqual(set(self.load("infinite-canvas")["jobs"]), {"changes", "swift"})


if __name__ == "__main__":
    unittest.main()
