"""Path globs that put a pull request behind the release-gate E2E lanes."""

import importlib.util
from pathlib import Path

import pytest

MODULE_PATH = Path(__file__).resolve().parents[1] / "cua_driver_e2e_gate_paths.py"
SPEC = importlib.util.spec_from_file_location("cua_driver_e2e_gate_paths", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
paths = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(paths)

R = "libs/cua-driver/rust/crates/"


@pytest.mark.parametrize(
    "path",
    [
        R + "cursor-overlay/src/lib.rs",
        R + "cua-cursor-motion/src/lib.rs",
        R + "platform-macos/src/cursor/mod.rs",
        R + "platform-macos/src/focus_guard.rs",
        R + "platform-macos/src/focus_steal.rs",
        R + "platform-macos/src/background_mutation.rs",
        R + "platform-macos/src/browser/mod.rs",
        R + "platform-macos/src/input/click.rs",
        R + "platform-windows/src/overlay.rs",
        R + "platform-windows/src/browser_platform.rs",
        R + "platform-linux/src/overlay_capture.rs",
        R + "cua-driver-core/src/background_input.rs",
        R + "cua-driver-core/src/agent_cursor.rs",
        R + "cua-driver-core/src/browser/routes.rs",
        R + "cua-driver-e2e/tests/standalone_browser_behavior_test.rs",
        "scripts/ci/macos/run-rust-e2e.sh",
        "scripts/ci/lib/e2e-retry-once.sh",
        ".github/workflows/e2e-rust-linux.yml",
    ],
)
def test_gated(path: str) -> None:
    assert paths.gated(path)


@pytest.mark.parametrize(
    "path",
    [
        "docs/guide.md",
        "libs/cua-driver/docs/test-harnesses-guide.md",
        R + "cua-perception/src/lib.rs",
        R + "cua-driver/src/updater.rs",
        R + "platform-macos/src/history.rs",
        "libs/lume/Package.swift",
    ],
)
def test_not_gated(path: str) -> None:
    assert not paths.gated(path)
