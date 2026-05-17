"""Integration parity tests for macOS focus-steal prevention.

`launch_app` must NOT change the user's frontmost application — that's
the contract Swift's `SystemFocusStealPreventer` + 3-phase wrap in
`LaunchAppTool` already enforce. This test suite encodes the same
contract for Rust `cua-driver-rs` so we can catch regressions and
verify the Swift↔Rust parity needed to flip macOS Rust BETA → GA.

Each test runs against the Rust binary (`CUA_DRIVER_BINARY`). When
`CUA_SWIFT_BINARY` is set, the same checks run against the Swift
binary too — both must pass for parity. Set `FOCUS_STEAL_RUST_ONLY=1`
to skip the Swift half (useful while iterating on Rust changes
without a Swift build on the path).

Test cases (mirror the plan's Verification section 1:1):

  1. test_launch_passive_app_preserves_frontmost — Calculator (passive)
  2. test_launch_self_activating_app_preserves_frontmost — Safari
  3. test_launch_with_url_preserves_frontmost — Safari + about:blank
  4. test_cold_launch_creates_window — kill + relaunch Calculator,
     assert window appears (verifies the `oapp` AppleEvent on the
     no-URL path)
  5. test_concurrent_launches_independent_suppression — back-to-back
     launches, both targets suppressed, frontmost unchanged.
  6. test_deadline_reaps_leaked_entry — exercised by the Rust unit
     test `focus_steal::tests::deadline_reaps_leaked_entry`. This file
     re-asserts the integration-level behavior: a launch followed by
     a stale-state probe doesn't keep re-activating the prior
     frontmost.

The frontmost-baseline app is Finder (`com.apple.finder`). It's the
only app guaranteed to be running on every logged-in macOS session
and it never self-activates on launch, which gives us a stable,
predictable "prior" pid between tests. (Earlier iterations of this
suite used a shared FocusMonitorApp fixture under
`libs/cua-driver/Tests/FocusMonitorApp/`, but the dependency on a
built helper made the integration tests flaky to bootstrap on a
fresh runner — Finder removes that dependency.)

Run:
    cd libs/cua-driver-rs/tests/integration
    python3 -m unittest test_focus_steal_parity -v
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path  # noqa: E402


# ── Paths / fixtures ─────────────────────────────────────────────────────────
def _swift_binary() -> str | None:
    """Locate the Swift binary, or None if not available."""
    from_env = os.environ.get("CUA_SWIFT_BINARY", "")
    if from_env and os.path.isfile(from_env):
        return from_env
    for p in (
        os.path.expanduser("~/.local/bin/cua-driver"),
        shutil.which("cua-driver") or "",
    ):
        if p and os.path.isfile(p) and os.access(p, os.X_OK):
            return p
    return None


def _frontmost_bundle_id() -> str | None:
    """Bundle id of the currently-frontmost app, queried via osascript.

    We deliberately don't use the driver's `list_apps` for this — the
    driver process itself may not be frontmost yet when invoked via
    `call`, and we want a host-truth answer that doesn't go through
    the same driver path we're testing.
    """
    try:
        out = subprocess.run(
            [
                "osascript",
                "-e",
                'tell application "System Events" to bundle identifier of first '
                "application process whose frontmost is true",
            ],
            capture_output=True,
            text=True,
            timeout=5,
        )
        bid = out.stdout.strip()
        return bid or None
    except Exception:
        return None


def _activate_bundle(bundle_id: str) -> None:
    """Bring `bundle_id` to the foreground via osascript."""
    subprocess.run(
        ["osascript", "-e", f'tell application id "{bundle_id}" to activate'],
        timeout=5,
        check=False,
    )


def _kill_bundle(bundle_id: str) -> None:
    """pkill the running app for `bundle_id` if any. Idempotent."""
    name = bundle_id.split(".")[-1]
    subprocess.run(["pkill", "-x", name], check=False)
    subprocess.run(["pkill", "-x", name.capitalize()], check=False)
    # Safari sometimes needs a moment for its child helper processes
    # (Networking, WebContent) to also exit before LaunchServices will
    # cleanly relaunch it from the Cryptex path. The 1s settle below
    # is conservative but cheap relative to the launch time itself.
    time.sleep(1.0)


def _wait_for_frontmost(bundle_id: str, timeout_s: float = 3.0) -> bool:
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if _frontmost_bundle_id() == bundle_id:
            return True
        time.sleep(0.1)
    return False


# ── Skipping logic ───────────────────────────────────────────────────────────
def _rust_binary() -> str | None:
    p = default_binary_path()
    return p if os.path.isfile(p) and os.access(p, os.X_OK) else None


_RUST = _rust_binary()
_SWIFT = _swift_binary()
_RUST_ONLY = os.environ.get("FOCUS_STEAL_RUST_ONLY", "") == "1"


# ── Mixin: the actual test cases ─────────────────────────────────────────────
class _FocusStealMixin:
    """Parametrized test cases. Subclasses set `BINARY` and `LABEL`."""

    BINARY: str = ""
    LABEL: str = ""

    # A stable "prior frontmost" app id we can reasonably expect every
    # macOS host to have running. Finder is always running on a logged-in
    # macOS session and never self-activates on launch.
    PRIOR_BUNDLE = "com.apple.finder"

    def setUp(self) -> None:
        # Bring the baseline app to the foreground, confirm. We retry a
        # few times because dev hosts with overlay / focus-grabber apps
        # (e.g. Gumbranch, screen recorders, status-bar apps that
        # forcibly steal focus on click) can racily re-steal during the
        # setup window.
        for _ in range(3):
            _activate_bundle(self.PRIOR_BUNDLE)
            if _wait_for_frontmost(self.PRIOR_BUNDLE, timeout_s=2.0):
                return
            time.sleep(0.5)
        self.skipTest(
            f"could not bring {self.PRIOR_BUNDLE} to the foreground after "
            "3 attempts — test host has an aggressive focus-grabber that "
            "makes the test environment unstable"
        )

    def _launch(self, args: dict) -> dict:
        with DriverClient(self.BINARY) as client:
            return client.call_tool("launch_app", args)

    # ── Case 1: passive app (Calculator) ────────────────────────────────
    def test_launch_passive_app_preserves_frontmost(self) -> None:
        _kill_bundle("com.apple.calculator")
        time.sleep(0.5)
        _activate_bundle(self.PRIOR_BUNDLE)
        time.sleep(0.5)
        baseline = _frontmost_bundle_id()
        self.assertEqual(baseline, self.PRIOR_BUNDLE)

        result = self._launch({"bundle_id": "com.apple.calculator"})
        self.assertIn("structuredContent", result)
        self.assertIn("pid", result["structuredContent"])

        # Settle, then assert the prior frontmost is unchanged.
        time.sleep(1.0)
        self.assertEqual(
            _frontmost_bundle_id(),
            self.PRIOR_BUNDLE,
            f"{self.LABEL}: Calculator launch stole focus from {self.PRIOR_BUNDLE}",
        )

    # ── Case 2: self-activating app (Safari, no URLs) ───────────────────
    def test_launch_self_activating_app_preserves_frontmost(self) -> None:
        _kill_bundle("com.apple.Safari")
        time.sleep(0.5)
        _activate_bundle(self.PRIOR_BUNDLE)
        time.sleep(0.5)
        self.assertEqual(_frontmost_bundle_id(), self.PRIOR_BUNDLE)

        # No URLs — exercises the `openApplicationAtURL` path with the
        # `oapp` AppleEvent.
        self._launch({"bundle_id": "com.apple.Safari"})
        time.sleep(1.5)  # allow Safari's reflex NSApp.activate to fire
        self.assertEqual(
            _frontmost_bundle_id(),
            self.PRIOR_BUNDLE,
            f"{self.LABEL}: Safari (no URL) launch stole focus from {self.PRIOR_BUNDLE}",
        )

    # ── Case 3: launch-with-URL preserves frontmost ─────────────────────
    def test_launch_with_url_preserves_frontmost(self) -> None:
        _kill_bundle("com.apple.Safari")
        time.sleep(0.5)
        _activate_bundle(self.PRIOR_BUNDLE)
        time.sleep(0.5)
        self.assertEqual(_frontmost_bundle_id(), self.PRIOR_BUNDLE)

        result = self._launch(
            {"bundle_id": "com.apple.Safari", "urls": ["about:blank"]}
        )
        # NOTE: the Swift binary on macOS Sonoma+ fails this launch with
        # "The application 'Safari' could not be launched because it was
        # not found" — that's a Cryptex-app + URL-handoff regression on
        # the Swift side (`AppLauncher.launch` attaches an `oapp`
        # AppleEvent to the openURLs:withApplicationAtURL: path, which
        # LaunchServices rejects for Safari). The Rust port fixes this
        # by skipping `oapp` on the URL-handoff path — see commit body
        # for `feat(launch_app): wire focus-steal preventer into
        # LaunchAppTool`. The Swift binary will fail this assertion;
        # treat that as a pre-existing Swift bug, not a Rust regression,
        # and surface it via `expectedFailure` on the Swift subclass.
        self.assertNotIn("isError", result, f"launch_app reported isError={result}")

        time.sleep(1.5)
        self.assertEqual(
            _frontmost_bundle_id(),
            self.PRIOR_BUNDLE,
            f"{self.LABEL}: Safari (about:blank) launch stole focus",
        )

    # ── Case 4: cold launch creates a window ────────────────────────────
    def test_cold_launch_creates_window(self) -> None:
        _kill_bundle("com.apple.calculator")
        time.sleep(0.5)
        _activate_bundle(self.PRIOR_BUNDLE)
        time.sleep(0.5)

        result = self._launch({"bundle_id": "com.apple.calculator"})
        windows = result["structuredContent"].get("windows", [])
        self.assertGreater(
            len(windows),
            0,
            f"{self.LABEL}: cold-launched Calculator had no windows — "
            "the `oapp` AppleEvent may not be reaching the target",
        )

    # ── Case 5: back-to-back launches each preserve frontmost ───────────
    def test_concurrent_launches_independent_suppression(self) -> None:
        _kill_bundle("com.apple.calculator")
        _kill_bundle("com.apple.TextEdit")
        time.sleep(0.5)
        _activate_bundle(self.PRIOR_BUNDLE)
        time.sleep(0.5)
        self.assertEqual(_frontmost_bundle_id(), self.PRIOR_BUNDLE)

        # Sequential, not truly concurrent — but each launch reads the
        # current frontmost (which should still be `PRIOR_BUNDLE` after
        # the first one succeeded if focus-steal worked). If the first
        # launch leaked focus, the second's prior would be Calculator
        # and the assertion below would fail.
        self._launch({"bundle_id": "com.apple.calculator"})
        time.sleep(0.7)
        self.assertEqual(_frontmost_bundle_id(), self.PRIOR_BUNDLE)

        self._launch({"bundle_id": "com.apple.TextEdit"})
        time.sleep(1.0)
        self.assertEqual(
            _frontmost_bundle_id(),
            self.PRIOR_BUNDLE,
            f"{self.LABEL}: back-to-back Calculator + TextEdit launches "
            "leaked focus from one suppression to the next",
        )

    # ── Case 6: deadline reaper integration probe ───────────────────────
    def test_deadline_reaps_leaked_entry(self) -> None:
        # The fine-grained reap behavior is unit-tested in
        # `focus_steal::tests::deadline_reaps_leaked_entry`. This
        # integration probe verifies the user-visible consequence:
        # launching, waiting 6s, then activating the launched app
        # manually does NOT cause an unsolicited re-activation of the
        # prior frontmost (which would happen if a leaked entry kept
        # firing past the 5s deadline).
        _kill_bundle("com.apple.calculator")
        time.sleep(0.5)
        _activate_bundle(self.PRIOR_BUNDLE)
        time.sleep(0.5)

        result = self._launch({"bundle_id": "com.apple.calculator"})
        calc_pid = result["structuredContent"]["pid"]
        self.assertIsInstance(calc_pid, int)
        # Wait past the 5s deadline.
        time.sleep(6.0)
        # Now manually activate Calculator. If the dispatcher leaked an
        # entry, it would yank focus back to PRIOR_BUNDLE here — and
        # the assertion below would fail.
        _activate_bundle("com.apple.calculator")
        time.sleep(1.0)
        self.assertEqual(
            _frontmost_bundle_id(),
            "com.apple.calculator",
            f"{self.LABEL}: stale entry yanked focus back after the "
            "5s deadline — the reaper or the observer skipped the prune",
        )


# ── Concrete subclasses ──────────────────────────────────────────────────────
@unittest.skipIf(_RUST is None, "Rust cua-driver-rs binary not built")
class RustFocusStealTests(_FocusStealMixin, unittest.TestCase):
    BINARY = _RUST or ""
    LABEL = "rust"


@unittest.skipIf(
    _SWIFT is None or _RUST_ONLY,
    "Swift cua-driver binary not available (set CUA_SWIFT_BINARY) "
    "or FOCUS_STEAL_RUST_ONLY=1 set",
)
class SwiftFocusStealTests(_FocusStealMixin, unittest.TestCase):
    BINARY = _SWIFT or ""
    LABEL = "swift"

    # Pre-existing Swift bug on macOS Sonoma+: launching Safari with
    # `urls=["about:blank"]` returns "The application 'Safari' could not
    # be launched because it was not found." The Rust port fixes this
    # (the `oapp` AppleEvent on the URL-handoff path is incompatible
    # with Cryptex-installed apps). Re-define the method on this
    # subclass only (NOT via expectedFailure on the mixin attribute —
    # that would also taint the Rust subclass) and mark the override
    # `@expectedFailure`.
    @unittest.expectedFailure
    def test_launch_with_url_preserves_frontmost(self) -> None:  # type: ignore[override]
        super().test_launch_with_url_preserves_frontmost()

    # NOTE: `test_deadline_reaps_leaked_entry` is intentionally NOT
    # marked expectedFailure on Swift. Swift's `SystemFocusStealPreventer`
    # has no monotonic deadline reaper (PR #1521 is unmerged at port
    # time), but in practice the Swift `LaunchAppTool.invoke` calls
    # `endSuppression` cleanly before the integration test's 6s wait
    # finishes, so the test sometimes passes on Swift anyway. The Rust
    # subclass is what locks down the reaper-driven contract.


if __name__ == "__main__":
    unittest.main()
