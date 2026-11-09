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
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ci_graph  # noqa: E402

DRIVER = "libs/cua-driver/rust/"
LINUX, MACOS, WINDOWS = "linux", "macos", "windows"
ALL_OS = (LINUX, MACOS, WINDOWS)

# Files that can affect every lane and every OS.
SHARED = (
    r"\.github/(workflows|actions)/",
    r"\.github/scripts/ci_(component_scope|graph)",
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


# The Docker lanes of ci-cua-spacesd.yml (`relay e2e (docker)`) build and test these packages. The
# lane needs to start only when a changed file is inside their dependency closure on Linux. The
# closure comes from the manifests (ci_graph.py), so a new dependency edge widens it by itself.
RELAY_E2E_ROOTS = (
    "cua-spacesd", "cua-relay", "cua-spacesd-server", "cua-spacesd-socks", "cua-spacesd-teleport",
)
RELAY_E2E_EXTRA_ROOT_DIRS = ("libs/cua-spacesd/tests/teleport-e2e",)
# Linux-only lane: crates that only compile on another OS cannot change it.
NOT_ON_LINUX = (DRIVER + "crates/platform-macos", DRIVER + "crates/platform-windows")


def relay_e2e_closure(graph: "ci_graph.Graph") -> set[str]:
    roots = {d for n in RELAY_E2E_ROOTS for d in graph.by_name(n)} | {
        d for d in RELAY_E2E_EXTRA_ROOT_DIRS if d in graph.packages
    }
    return {d for d in graph.closure(roots) if d not in NOT_ON_LINUX}


def decide_relay_e2e(files: list[str], event: str, head_ref: str, graph: "ci_graph.Graph") -> tuple[bool, str]:
    if event != "pull_request":
        return True, f"{event} runs the lane"
    if head_ref.startswith("release-please--"):
        return True, "release PR runs the lane"
    if not files:
        return True, "no changed files listed, failing closed"
    closure = relay_e2e_closure(graph)
    for f in files:
        if _any(SHARED, f):
            return True, f"shared input {f}"
        if _any(INERT, f):
            continue
        if any(f.startswith(p + "/") for p in NOT_ON_LINUX):
            continue
        owner = graph.owner(f)
        if owner in closure:
            return True, f"{f} is in the dependency closure ({owner})"
        if owner is None and f.startswith("libs/cua-spacesd/"):
            return True, f"{f} is a spacesd file outside any crate (scripts, image, config)"
    return False, "no changed path is in the relay e2e dependency closure"


def main() -> None:
    lane = sys.argv[1]
    files = [l.strip() for l in sys.stdin if l.strip()]
    if lane == "relay-e2e":
        run_, why = decide_relay_e2e(
            files, os.environ.get("EVENT", "pull_request"), os.environ.get("HEAD_REF", ""), ci_graph.load()
        )
        print(f"run={str(run_).lower()}\nreason={why}")
        path = os.environ.get("GITHUB_OUTPUT")
        if path:
            with open(path, "a") as fh:
                fh.write(f"run={str(run_).lower()}\nreason={why}\n")
        return
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
