"""The SwiftUI app's CI builds the Rust library once and checks it in parallel.

`ci-cua-spaces-macos.yml` used to be one 60 minute job: compile
libcua_spaces_ffi, then run every Swift check one after the other, behind a
`core` job it waited for. It is now `ffi` (build once, or take main's compiled
library) plus three jobs that download the library and run side by side. These
tests keep the split honest: every check still runs, none compiles the library
again, and none waits for `core`.
"""

from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = yaml.safe_load((ROOT / ".github/workflows/ci-cua-spaces-macos.yml").read_text())
JOBS = WORKFLOW["jobs"]
SWIFT_JOBS = ("swift-tests", "bindings", "app-bundle")
ARTIFACT = "cua-spaces-ffi-dylib"

# Every check the single SwiftUI job ran, and the job that now runs it.
CHECKS = {
    "libs/spaces-app-swift/scripts/test.sh": "swift-tests",
    "libs/spaces-notch-swift/scripts/test.sh": "swift-tests",
    "scripts/test.sh\n": "swift-tests",
    "--filter BridgeContractTests": "swift-tests",
    "npx vitest run src/bridge/__tests__/contract.test.ts": "swift-tests",
    "generate-bindings.mjs --check --skip-build": "bindings",
    "pnpm native -- --skip-build --check-bindings": "bindings",
    "scripts/build-app.sh release": "app-bundle",
    "scripts/appcast-dry-run.sh": "app-bundle",
    "xcodebuild test": "app-bundle",
}


def run_text(job: str) -> str:
    return "\n".join(step.get("run", "") + "\n" for step in JOBS[job]["steps"])


def uses(job: str) -> list[str]:
    return [step["uses"] for step in JOBS[job]["steps"] if "uses" in step]


def test_every_swift_check_still_runs_in_exactly_one_job():
    for needle, owner in CHECKS.items():
        holders = [name for name in JOBS if needle in run_text(name)]
        assert holders == [owner], f"{needle!r} runs in {holders}, expected [{owner!r}]"


def test_the_library_is_compiled_once_and_downloaded_by_the_rest():
    builders = [n for n in JOBS if "cargo build" in run_text(n) and "cua-spaces-ffi" in run_text(n)]
    assert builders == ["ffi"]
    for name in SWIFT_JOBS:
        job = JOBS[name]
        assert job["needs"] == "ffi", name
        download = [s for s in job["steps"] if str(s.get("uses", "")).startswith("actions/download-artifact")]
        assert len(download) == 1 and download[0]["with"]["name"] == ARTIFACT, name
        assert download[0]["with"]["path"] == "libs/cua/target/release", name
    upload = [s for s in JOBS["ffi"]["steps"] if str(s.get("uses", "")).startswith("actions/upload-artifact")]
    assert len(upload) == 1 and upload[0]["with"]["name"] == ARTIFACT
    assert upload[0]["with"]["if-no-files-found"] == "error"


def test_the_jobs_that_load_the_library_stage_it_after_downloading_it():
    for name in ("swift-tests", "app-bundle"):
        names = [s.get("name", "") for s in JOBS[name]["steps"]]
        assert names.index("Stage the Cua Spaces app library") > names.index("Download the Cua Spaces app library")
        assert "stage-library.sh" in run_text(name)


def test_the_swift_checks_do_not_wait_for_core():
    # `core` is a 4 minute Linux job; waiting for it put 4 minutes in front of
    # the 60 minute macOS path. It still runs, and ci-gate still waits for it.
    assert "core" in JOBS
    for name in ("ffi", *SWIFT_JOBS):
        assert "core" not in str(JOBS[name].get("needs", "")), name


def test_release_please_pull_requests_still_skip_the_macos_jobs():
    # `swift` used to inherit core's skip through `needs`; `ffi` carries it now
    # and the rest are skipped with it.
    assert "release-please--branches--" in JOBS["ffi"]["if"]
    assert "release-please--branches--" in JOBS["core"]["if"]


def test_ffi_takes_main_slice_only_by_restoring_it():
    steps = {s.get("id"): s for s in JOBS["ffi"]["steps"] if s.get("id")}
    slice_step = steps["slice"]
    assert slice_step["uses"] == "./.github/actions/spaces-rust-slice"
    assert slice_step["with"] == {"triple": "aarch64-apple-darwin", "build": "false"}
    assert JOBS["ffi"]["env"]["CUA_RELEASE_REPOSITORY"] == "${{ github.repository }}"
    # Every compile step is for the miss path only.
    for step in JOBS["ffi"]["steps"]:
        if step.get("name") in ("Install Rust (toolchain from rust-toolchain.toml)", "Install protoc",
                                "Build the Cua Spaces app library") or step.get("uses") == "./.github/actions/rust-cache":
            assert step["if"] == "steps.slice.outputs.cache-hit != 'true'", step
    copy = next(s for s in JOBS["ffi"]["steps"] if s.get("name") == "Use the restored library")
    assert copy["if"] == "steps.slice.outputs.cache-hit == 'true'"
    assert "target/aarch64-apple-darwin/release/libcua_spaces_ffi.dylib" in copy["run"]


def test_only_ffi_saves_the_sccache_family_on_main():
    rust_cache = [s for s in JOBS["bindings"]["steps"] if s.get("uses") == "./.github/actions/rust-cache"]
    assert len(rust_cache) == 1 and rust_cache[0]["with"]["save-if"] == "false"
    for name in ("swift-tests", "app-bundle"):
        assert "./.github/actions/rust-cache" not in uses(name)


def test_snapshot_differences_are_still_uploaded_on_failure():
    step = next(s for s in JOBS["swift-tests"]["steps"] if s.get("name") == "Upload snapshot differences")
    assert step["if"] == "failure()"


def test_the_slice_action_builds_by_default_and_restores_only_on_request():
    action = yaml.safe_load((ROOT / ".github/actions/spaces-rust-slice/action.yml").read_text())
    assert action["inputs"]["build"]["default"] == "true"
    steps = action["runs"]["steps"]
    by_id = {s.get("id"): s for s in steps if s.get("id")}
    assert by_id["cache"]["uses"].startswith("actions/cache@") and by_id["cache"]["if"] == "inputs.build == 'true'"
    assert by_id["restore"]["uses"].startswith("actions/cache/restore@")
    assert by_id["restore"]["if"] == "inputs.build != 'true'"
    # Nothing compiles unless build is on.
    for step in steps:
        if step.get("name") in ("Install protoc", "Build the slice") or str(step.get("uses", "")).startswith(
            ("dtolnay/rust-toolchain", "Swatinem/rust-cache")
        ):
            assert step["if"].startswith("inputs.build == 'true' &&"), step
    # The producer and the release keep their behaviour: they pass no `build`.
    for workflow in ("ci-cua-spaces-rust-slice.yml", "cd-cua-spaces.yml", "cd-cua-spaces-build.yml"):
        text = (ROOT / ".github/workflows" / workflow).read_text()
        assert "spaces-rust-slice" in text
        assert 'build: "false"' not in text
