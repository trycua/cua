"""Decide whether changed paths need the Cua Driver release-gate E2E lanes.

Reads one repo-relative path per line on stdin and prints the matching paths.
Exit status is always 0; an empty output means the change is not gated by path.

Any change to the driver's code, build inputs, test harness or fixtures is gated.
This used to be an allowlist of file names (cursor, overlay, focus, background
input, browser routing). It missed `platform-macos/src/tools/type_text.rs` and
`platform-macos/src/apps/` (#8089, #8220), so two regressions shipped in 0.55.1
through 0.56.x without a single lane running on their PRs. A path allowlist by
behavior name always lags the next break; gating the whole tree does not.
Documentation and changelogs stay out.
"""

import re
import sys

RUST = "libs/cua-driver/rust/"
GATED_PATTERNS = (
    # Every driver crate, test app, build input and toolchain pin.
    RUST + r"(crates|test-apps|include|scripts|\.cargo)/",
    RUST + r"(Cargo\.toml|Cargo\.lock|rust-toolchain\.toml)$",
    # Fixtures and the contract the lanes execute.
    r"libs/cua-driver/(tests/fixtures|contract)/",
    # The gate itself.
    r"scripts/ci/(lib/|linux/run-|macos/|windows/run-|run-rust-standalone)",
    r"\.github/scripts/cua_driver_e2e_gate_paths\.py$",
    r"\.github/workflows/(e2e-rust-(linux|windows|macos|standalone-browsers)|e2e-cua-driver-release-gate)\.yml$",
)
# Prose never changes behavior: markdown and docs stay out even under a gated tree.
EXCLUDED = re.compile(r"(\.(md|mdx|txt)$|/docs/|/CHANGELOG)")
_COMPILED = [re.compile(p) for p in GATED_PATTERNS]


def gated(path: str) -> bool:
    return not EXCLUDED.search(path) and any(rx.match(path) for rx in _COMPILED)


def main() -> None:
    for line in sys.stdin:
        path = line.strip()
        if path and gated(path):
            print(path)


if __name__ == "__main__":
    main()
