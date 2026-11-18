"""Decide whether changed paths need the Cua Driver release-gate E2E lanes.

Reads one repo-relative path per line on stdin and prints the matching paths.
Exit status is always 0; an empty output means the change is not gated by path.
The globs cover the code that past release gates broke on: the cursor overlay,
focus handling, background input, and browser routing, plus the gate harness.
"""

import re
import sys

RUST = "libs/cua-driver/rust/"
GATED_PATTERNS = (
    # Whole crates dedicated to the cursor/overlay.
    RUST + r"crates/(cursor-overlay|cua-cursor-motion|cursor-theme-cli)/",
    # Cursor overlay, focus handling, background input, browser routing in any
    # driver crate, by directory or by file name.
    RUST + r"crates/[^/]+/src/(.*/)?(cursor|overlay|focus|background|browser|input|pointer)[^/]*",
    RUST + r"crates/[^/]+/src/(.*/)?[^/]*(cursor|overlay|focus|background_|browser|pointer_shape|key_focus)[^/]*",
    RUST + r"crates/cua-driver-core/src/(browser|background_input|agent_cursor)",
    RUST + r"crates/cua-driver-contract/src/cursor",
    # The behavior harness the lanes execute.
    RUST + r"crates/cua-driver-(e2e|testkit)/",
    RUST + r"test-apps/",
    r"libs/cua-driver/tests/fixtures/",
    # The gate itself.
    r"scripts/ci/(lib/|linux/run-|macos/run-|windows/run-|run-rust-standalone)",
    r"\.github/workflows/(e2e-rust-(linux|windows|macos|standalone-browsers)|e2e-cua-driver-release-gate)\.yml",
)
_COMPILED = [re.compile(p) for p in GATED_PATTERNS]


def gated(path: str) -> bool:
    return any(rx.match(path) for rx in _COMPILED)


def main() -> None:
    for line in sys.stdin:
        path = line.strip()
        if path and gated(path):
            print(path)


if __name__ == "__main__":
    main()
