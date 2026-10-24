#!/usr/bin/env python3
"""Decide whether a pull request needs a macOS Swift sample build.

Two workflows build a Swift sample on macOS 26 against the cua SDK dylib
(28 to 33 macOS minutes each): ci-infinite-canvas-swift.yml and the `swift` job
of ci-openkoalabot-example-samples.yml. They used to run on every change under
libs/cua/ (the whole Rust workspace). They only need to run when something they
compile or read changes.

Usage:
    ci_swift_samples_scope.py scope <lane>             changed paths on stdin
    ci_swift_samples_scope.py check-manifests <lane>   run from the repo root

`scope` writes GitHub Actions outputs (`run`, `reason`). `check-manifests`
fails when the dependency map below no longer matches the repository.

What a sample depends on
------------------------
* The sample itself, and the files its tests read from elsewhere in the tree.
* The Swift packages it links, found from its Package.swift `.package(path:)`
  entries: libs/cua/swift (the Cua package, with the generated CuaSDK binding),
  libs/spaces-sdk-swift and libs/spaces-app-swift. `check-manifests` follows
  those entries, so a new path dependency cannot be missed.
* The SDK binding. libcua_sdk is built from libs/cua/crates/cua-sdk, the only
  crate that exports the UniFFI surface the Swift binding is generated from,
  against the wire contract in libs/cua/proto and crates/cua-proto. The Rust
  workspace manifest, lockfile, toolchain pin and cargo config decide how it is
  built, and stage-uniffi-library.mjs stages it.

What is deliberately not a trigger: the rest of cua-sdk's Rust closure (daemon,
vmm, spaces, host, ...). A change there that alters the Swift API must
regenerate libs/cua/swift/Sources/CuaSDK, which the `bindings drift` job of
ci-cua-sdk.yml enforces on every libs/cua pull request, and that path is a
trigger here. A change that only alters runtime behavior is covered by the
full run on every push to main and the nightly schedule.

Always the full build: any event other than pull_request, a release-please
branch, a pull request labelled `ci:full`, a change to the workflow or this
script, and a file listing that is empty or may be truncated (fail closed).
"""

from __future__ import annotations

import json
import os
import posixpath
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# A trailing slash is a directory prefix; anything else is one exact file.
SWIFT_PACKAGES = (
    "libs/cua/swift/",
    "libs/spaces-sdk-swift/",
    "libs/spaces-app-swift/",
)
SDK_BINDING = (
    "libs/cua/crates/cua-sdk/",
    "libs/cua/crates/cua-proto/",
    "libs/cua/proto/",
    "libs/cua/scripts/stage-uniffi-library.mjs",
    "libs/cua/Cargo.toml",
    "libs/cua/Cargo.lock",
    "libs/cua/rust-toolchain.toml",
    "libs/cua/.cargo/",
)
COMMON = SWIFT_PACKAGES + SDK_BINDING + (".github/scripts/ci_swift_samples_scope.py",)

LANES = {
    "infinite-canvas": {
        "sample": "examples/infinite-canvas-swift",
        "inputs": COMMON + (
            "examples/infinite-canvas-swift/",
            # scripts/sync-cursor-assets.sh --check compares the sample's copy.
            "libs/cua-driver/rust/crates/cursor-overlay/assets/",
            ".github/workflows/ci-infinite-canvas-swift.yml",
        ),
    },
    "openkoalabot": {
        "sample": "examples/openkoalabot-example-swift",
        "inputs": COMMON + (
            "examples/openkoalabot-example-swift/",
            # ScenarioTests read scenario.json; SpaceCreationTests and
            # scripts/gen-images.mjs read the shared image list.
            "examples/openkoalabot-example-scenario/",
            "libs/images/sandbox-images.json",
            ".github/workflows/ci-openkoalabot-example-samples.yml",
        ),
    },
}

# A Swift package builds the same without these.
INERT = re.compile(r".*\.md$|(^|.*/)(LICENSE|CHANGELOG\.md|VERSION)$")
# `.package(name: "Cua", path: "../../libs/cua/swift")`
PATH_DEPENDENCY = re.compile(r'\.package\s*\([^)]*?\bpath:\s*"([^"]+)"')
# The pull request files API stops at 3000 files.
FILE_LISTING_LIMIT = 3000


def covered(lane: str, path: str) -> bool:
    return any(path.startswith(i) if i.endswith("/") else path == i for i in LANES[lane]["inputs"])


def decide(lane: str, files: list[str], event: str, head_ref: str = "", labels: tuple[str, ...] = ()) -> tuple[bool, str]:
    if event != "pull_request":
        return True, f"{event} runs the full build"
    if head_ref.startswith("release-please--"):
        return True, "release PR runs the full build"
    if "ci:full" in labels:
        return True, "ci:full label"
    if not files:
        return True, "no changed files listed, failing closed"
    if len(files) >= FILE_LISTING_LIMIT:
        return True, "file listing may be truncated, failing closed"
    for f in files:
        if not INERT.match(f) and covered(lane, f):
            return True, f"{f} is a dependency of the sample"
    return False, "no changed path is a dependency of the sample"


def package_closure(lane: str, root: Path = ROOT) -> dict[str, list[str]]:
    """Swift packages reachable from the sample through `.package(path:)`."""
    seen: dict[str, list[str]] = {}
    stack = [LANES[lane]["sample"]]
    while stack:
        directory = stack.pop()
        if directory in seen:
            continue
        manifest = root / directory / "Package.swift"
        text = manifest.read_text(encoding="utf-8")
        seen[directory] = [posixpath.normpath(posixpath.join(directory, p)) for p in PATH_DEPENDENCY.findall(text)]
        stack.extend(seen[directory])
    return seen


def manifest_problems(lane: str, root: Path = ROOT) -> list[str]:
    problems = [f"{i} does not exist" for i in LANES[lane]["inputs"] if not (root / i).exists()]
    for directory in package_closure(lane, root):
        if not covered(lane, f"{directory}/Package.swift"):
            problems.append(f"{directory} is a Swift package dependency but is not in the {lane} inputs")
    return problems


def emit(**outputs: str) -> None:
    for k, v in outputs.items():
        print(f"{k}={v}")
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a", encoding="utf-8") as fh:
            fh.writelines(f"{k}={v}\n" for k, v in outputs.items())


def labels_from_env() -> tuple[str, ...]:
    raw = os.environ.get("LABELS", "").strip()
    if not raw:
        return ()
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return tuple(s.strip() for s in raw.split(",") if s.strip())
    return tuple(parsed) if isinstance(parsed, list) else ()


def main(argv: list[str]) -> int:
    if len(argv) != 3 or argv[1] not in ("scope", "check-manifests") or argv[2] not in LANES:
        print(__doc__, file=sys.stderr)
        return 2
    command, lane = argv[1], argv[2]
    if command == "check-manifests":
        problems = manifest_problems(lane)
        for p in problems:
            print(f"::error::ci_swift_samples_scope.py: {p}")
        return 1 if problems else 0
    files = [line.strip() for line in sys.stdin if line.strip()]
    run, reason = decide(
        lane, files, os.environ.get("EVENT", "pull_request"), os.environ.get("HEAD_REF", ""), labels_from_env()
    )
    emit(run=str(run).lower(), reason=reason)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
