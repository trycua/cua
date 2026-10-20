#!/usr/bin/env python3
"""Decide which heavy CI lanes a pull request needs (CUA-1910).

Reads one changed repo-relative path per line on stdin and writes GitHub
Actions outputs (`run`, `matrix`, `reason`) for one lane.

The dependency map below is explicit. The Rust entries are the workspace
closure that `cargo metadata` reports for the lane's workspace, so a crate
outside the closure cannot affect the lane. Shared inputs fail closed: they run
the lane on every OS. The calling workflow's `paths:` filter bounds the rest.

Always the full lane, on every OS:
  * any event other than pull_request (main pushes, nightly, tags, manual);
  * a release-please branch;
  * a change to CI files, shared actions, lockfiles, manifests, toolchain pins
    or this script.
"""

from __future__ import annotations

import json
import os
import re
import sys

DRIVER = "libs/cua-driver/rust/"
LINUX, MACOS, WINDOWS = "linux", "macos", "windows"
ALL_OS = (LINUX, MACOS, WINDOWS)

# Files that can affect every lane and every OS.
SHARED = (
    r"\.github/(workflows|actions)/",
    r"\.github/scripts/ci_component_scope",
    r"scripts/ci/",
    r"(^|.*/)(Cargo\.lock|rust-toolchain\.toml|flake\.lock|package-lock\.json|uv\.lock)$",
    # Workspace roots; a member's own Cargo.toml is covered by its crate path.
    r"libs/(cua-driver/rust|cua-spacesd|cua)/Cargo\.toml$",
    r"(^|.*/)\.cargo/",
    r"libs/cua/proto/",
    r"libs/cua/crates/cua-proto/",
    r"libs/cua-driver/rust/(include|tests)/",
)
# Never changes behaviour.
INERT = (r".*\.md$", r".*/(CHANGELOG\.md|VERSION|LICENSE)$", r"\.release-please-manifest\.json$")

# Driver crates that only compile on one OS.
DRIVER_PLATFORM = {
    DRIVER + "crates/platform-linux/": (LINUX,),
    DRIVER + "crates/platform-macos/": (MACOS,),
    DRIVER + "crates/platform-windows/": (WINDOWS,),
}
# Driver crates every OS compiles.
DRIVER_COMMON = (
    "cua-driver-core", "cua-driver-contract", "cua-driver-sdk", "cua-cursor-motion",
    "cursor-overlay", "pip-preview",
)

LANES = {
    # `cargo metadata` in libs/cua-spacesd: its workspace plus these path crates.
    "spacesd": {
        "labels": {LINUX: "ubuntu-latest", MACOS: "macos-26", WINDOWS: "windows-latest"},
        "common": (
            r"libs/cua-spacesd/",
            *(DRIVER + f"crates/{c}/" for c in DRIVER_COMMON),
            *(
                f"libs/cua/crates/{c}/"
                for c in (
                    "cua-chromium-storage", "cua-home", "cua-hotspot", "cua-logging",
                    "cua-machine-seal", "cua-media-codec", "cua-media-protocol",
                    "cua-media-transport", "cua-mesh", "cua-spacesd-client",
                    "cua-spacesd-html5", "cua-telemetry", "cua-teleport-bundle", "cua-volume",
                )
            ),
            r"libs/images/windows-2022/(enable-volume|volume-diagnostics)\.ps1",
        ),
    },
    # Runs the driver CLI and the client SDKs on all three OSes.
    "contract-clients": {
        "labels": {LINUX: "ubuntu-latest", MACOS: "macos-latest", WINDOWS: "windows-latest"},
        "common": (
            r"libs/cua-driver/(contract|compat-fixtures|typescript|python|examples/agent-sdks)/",
            r"libs/cua-driver/scripts/(generate-uniffi-bindings|stage-uniffi-library|build-npm-packages)\.mjs",
            r"libs/python/cua-sandbox/",
            r"\.github/scripts/cua-driver-mcp-compat/",
            *(DRIVER + f"crates/{c}/" for c in DRIVER_COMMON),
            DRIVER + "crates/cua-driver/",
            DRIVER + "crates/cua-driver-bindgen/",
            DRIVER + "crates/cua-driver-testkit/",
        ),
    },
}


def _any(patterns, path: str) -> bool:
    return any(re.match(p, path) for p in patterns)


def decide(lane: str, files: list[str], event: str, head_ref: str = "") -> tuple[bool, list[str], str]:
    cfg = LANES[lane]
    full = [cfg["labels"][o] for o in ALL_OS]
    if event != "pull_request":
        return True, full, f"{event} runs the full matrix"
    if head_ref.startswith("release-please--"):
        return True, full, "release PR runs the full matrix"
    if not files:
        return True, full, "no changed files listed, failing closed"
    oses: set[str] = set()
    why = ""
    for f in files:
        if _any(SHARED, f):
            return True, full, f"shared input {f}"
        if _any(INERT, f):
            continue
        if _any(cfg["common"], f):
            return True, full, f"{f} affects every OS"
        for prefix, plat in DRIVER_PLATFORM.items():
            if f.startswith(prefix):
                oses.update(plat)
                why = why or f"{f} affects {plat[0]} only"
                break
        # Anything else is outside the lane's dependency closure.
    if not oses:
        return False, [], "no changed path is in this lane's dependency closure"
    return True, [cfg["labels"][o] for o in ALL_OS if o in oses], why


def main() -> None:
    lane = sys.argv[1]
    files = [l.strip() for l in sys.stdin if l.strip()]
    run, labels, why = decide(lane, files, os.environ.get("EVENT", "pull_request"), os.environ.get("HEAD_REF", ""))
    out = {"run": str(run).lower(), "matrix": json.dumps(labels), "reason": why}
    for k, v in out.items():
        print(f"{k}={v}")
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a") as fh:
            fh.writelines(f"{k}={v}\n" for k, v in out.items())


if __name__ == "__main__":
    main()
