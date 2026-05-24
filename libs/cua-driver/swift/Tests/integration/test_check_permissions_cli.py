"""Integration test: `cua-driver check_permissions` CLI behavior.

Covers the TCC-responsibility-chain fix:

  1. When no daemon is running, the CLI's in-process fallback emits a
     stderr warning explaining that AXIsProcessTrusted() /
     SCShareableContent.current are evaluated against the calling shell's
     TCC context (not CuaDriver.app), so "NOT granted" here is not
     authoritative. Users need that warning — otherwise they burn 15min
     of onboarding time chasing a non-bug.

  2. The stdout payload (✅/❌ summary) is NOT polluted by the warning —
     stderr and stdout stay cleanly separated so `2>/dev/null` pipelines
     keep working.

  3. When the daemon IS running, the CLI auto-forwards and no warning
     fires — the daemon's in-process AXIsProcessTrusted() has the right
     responsibility chain, so its answer is authoritative.

Stops any running daemon at the start of each subtest to get a clean
baseline; restores nothing on exit (tests should leave a clean slate).

Run:
    scripts/test.sh test_check_permissions_cli
"""

from __future__ import annotations

import os
import subprocess
import sys
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import default_binary_path


WARNING_MARKER = "Not running inside the cua-driver daemon process"


def _run_cli(binary: str, *args: str) -> subprocess.CompletedProcess:
    """Invoke the CLI one-shot and return stdout+stderr+exit code."""
    return subprocess.run(
        [binary, *args],
        capture_output=True,
        text=True,
        timeout=15,
    )


def _stop_daemon(binary: str) -> None:
    """Best-effort stop; the 'not running' case is not an error here."""
    subprocess.run(
        [binary, "stop"], capture_output=True, text=True, timeout=5
    )
    # Let the socket file vanish before the next call.
    time.sleep(0.3)


def _start_daemon(binary: str) -> None:
    """Launch the daemon via the .app bundle so it has the right TCC
    responsibility chain — plain `cua-driver serve &` from the shell
    inherits the shell's TCC context, defeating the point of the test."""
    # Resolve the .app path from the binary path. The build script puts
    # the binary at `.build/CuaDriver.app/Contents/MacOS/cua-driver`.
    app_path = os.path.abspath(
        os.path.join(os.path.dirname(binary), "..", "..")
    )
    assert app_path.endswith(".app"), (
        f"expected binary inside CuaDriver.app, got {app_path!r}"
    )
    subprocess.run(
        ["open", "-a", app_path, "--args", "serve"],
        check=True,
        timeout=5,
    )
    # Poll for the socket to come up. `serve` bootstraps AppKit +
    # PermissionsGate before binding the socket, so this can take ~1s.
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        result = subprocess.run(
            [binary, "status"], capture_output=True, text=True, timeout=2
        )
        if result.returncode == 0:
            return
        time.sleep(0.1)
    raise RuntimeError("daemon did not come up within 5s")


class CheckPermissionsCLITests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.binary = default_binary_path()

    def test_no_daemon_emits_tcc_warning_on_stderr(self) -> None:
        _stop_daemon(self.binary)

        result = _run_cli(self.binary, "check_permissions")

        self.assertEqual(result.returncode, 0, f"stderr={result.stderr!r}")
        self.assertIn(
            WARNING_MARKER,
            result.stderr,
            f"warning missing from stderr: {result.stderr!r}",
        )
        # Stdout carries only the ✅/❌ summary — the warning must not
        # leak into it (pipes like `… | grep granted` must keep working).
        self.assertNotIn(WARNING_MARKER, result.stdout)
        self.assertIn("Accessibility:", result.stdout)
        self.assertIn("Screen Recording:", result.stdout)

    def test_daemon_running_forwards_and_no_warning(self) -> None:
        _stop_daemon(self.binary)
        _start_daemon(self.binary)
        try:
            result = _run_cli(self.binary, "check_permissions")
        finally:
            _stop_daemon(self.binary)

        self.assertEqual(result.returncode, 0, f"stderr={result.stderr!r}")
        # Daemon path is authoritative — no warning, on either stream.
        self.assertNotIn(WARNING_MARKER, result.stderr)
        self.assertNotIn(WARNING_MARKER, result.stdout)
        self.assertIn("Accessibility:", result.stdout)
        self.assertIn("Screen Recording:", result.stdout)


if __name__ == "__main__":
    unittest.main()
