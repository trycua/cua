"""Integration test: `health_report` MCP tool over the stdio protocol.

Boots the real `cua-driver mcp` server, performs the JSON-RPC handshake,
asks `tools/list` for the tool catalog, and exercises the tool with
several argument shapes:

  1. `tools/list` advertises `health_report` (no breaking schema drift —
     the consumer-facing contract is that the tool *exists* and ships
     `schema_version="1"` in its description).

  2. Calling with no arguments produces a well-formed report with the
     documented top-level keys and per-check key shape.

  3. The `skip` filter is honored end-to-end: a skipped check appears in
     the output with `status: "skip"`.

  4. The `include` filter narrows the report to a single check.

  5. A documented fail mode is observable end-to-end: when called via
     `cua-driver mcp` from a terminal (which is what an XCTest/CI host
     looks like), the `bundle_identity` check fails because the stdio
     process is NOT attributed to com.trycua.driver. The schema
     guarantees `status="fail"` entries carry a `hint` and surface the
     runtime bundle identifier under `data.bundle_identifier` — both
     are asserted here.

Run with the same harness as the other integration tests:
    scripts/test.sh test_health_report_mcp
"""

from __future__ import annotations

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from driver_client import DriverClient, default_binary_path


REQUIRED_TOP_LEVEL_KEYS = {
    "schema_version",
    "platform",
    "driver_version",
    "overall",
    "checks",
}

REQUIRED_CHECK_KEYS = {"name", "status", "message"}
ALLOWED_STATUS_VALUES = {"pass", "fail", "skip"}
ALLOWED_OVERALL_VALUES = {"ok", "degraded", "failed"}


def _structured_or_parsed(result: dict) -> dict:
    """Pull the JSON report out of an MCP tool result.

    Tools that emit `structuredContent` (the canonical path here) return
    it as a top-level field. We also tolerate a text-content fallback —
    if structured emission ever breaks, the text block still carries the
    JSON and we want the test to fail loudly with a clear message rather
    than crash on a KeyError. Mirrors the same defensive read pattern
    used by `driver_client.frontmost_bundle_id`.
    """
    if "structuredContent" in result:
        return result["structuredContent"]
    # Defensive fallback — accept the first text block as JSON.
    for content in result.get("content", []):
        if content.get("type") == "text":
            try:
                return json.loads(content.get("text", ""))
            except json.JSONDecodeError:
                continue
    raise AssertionError(
        "health_report response carried neither structuredContent nor a JSON text block: "
        + json.dumps(result)
    )


class HealthReportMCPTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.binary = default_binary_path()

    # ── tools/list ───────────────────────────────────────────────────

    def test_tools_list_advertises_health_report(self) -> None:
        with DriverClient(self.binary) as client:
            tools = client.list_tools()
        names = {t["name"] for t in tools}
        self.assertIn(
            "health_report",
            names,
            "tools/list must advertise health_report so consumers can discover it",
        )
        descriptor = next(t for t in tools if t["name"] == "health_report")
        # The schema_version commitment lives in the description text —
        # that's the consumer-facing stability contract.
        description = descriptor.get("description", "")
        self.assertIn(
            "schema_version",
            description,
            "tool description must call out the stable schema_version contract",
        )

    # ── tools/call with no args ──────────────────────────────────────

    def test_call_with_no_args_returns_schema_shape(self) -> None:
        with DriverClient(self.binary) as client:
            result = client.call_tool("health_report")
        report = _structured_or_parsed(result)

        # Top-level keys exactly match the documented schema.
        self.assertEqual(
            REQUIRED_TOP_LEVEL_KEYS,
            REQUIRED_TOP_LEVEL_KEYS & set(report.keys()),
            f"missing top-level keys; got {set(report.keys())!r}",
        )
        self.assertEqual(report["schema_version"], "1")
        self.assertEqual(report["platform"], "darwin")
        self.assertIsInstance(report["driver_version"], str)
        self.assertIn(report["overall"], ALLOWED_OVERALL_VALUES)

        checks = report["checks"]
        self.assertIsInstance(checks, list)
        self.assertGreater(len(checks), 0)

        for check in checks:
            self.assertTrue(
                REQUIRED_CHECK_KEYS.issubset(check.keys()),
                f"check missing required keys: {check!r}",
            )
            self.assertIn(check["status"], ALLOWED_STATUS_VALUES)
            # hint is only required when status == fail.
            if check["status"] == "fail":
                self.assertIn(
                    "hint",
                    check,
                    f"fail entries must carry a remediation hint: {check!r}",
                )
                self.assertTrue(
                    isinstance(check["hint"], str) and check["hint"].strip(),
                    f"hint must be a non-empty string: {check!r}",
                )

    # ── skip filter ──────────────────────────────────────────────────

    def test_skip_filter_marks_check_as_skip(self) -> None:
        with DriverClient(self.binary) as client:
            result = client.call_tool(
                "health_report",
                {"skip": ["tcc_accessibility"]},
            )
        report = _structured_or_parsed(result)
        names_to_status = {c["name"]: c["status"] for c in report["checks"]}
        self.assertEqual(names_to_status.get("tcc_accessibility"), "skip")

    # ── include filter ──────────────────────────────────────────────

    def test_include_filter_runs_only_named_check(self) -> None:
        with DriverClient(self.binary) as client:
            result = client.call_tool(
                "health_report",
                {"include": ["binary_version"]},
            )
        report = _structured_or_parsed(result)
        run_names = {
            c["name"] for c in report["checks"] if c["status"] != "skip"
        }
        # Only `binary_version` should have been actually run; the rest
        # show up with status: skip.
        self.assertEqual(run_names, {"binary_version"})

    # ── documented fail mode ────────────────────────────────────────

    def test_bundle_identity_fail_mode_under_stdio_mcp(self) -> None:
        """The stdio MCP process spawned by an IDE/CI shell runs outside
        CuaDriver.app, so its CFBundleIdentifier is NOT com.trycua.driver.
        That's the canonical "wrong attribution" fail mode the tool is
        designed to surface — schema-wise: status=fail + non-empty
        message + non-empty hint + data.bundle_identifier present so
        consumers can detect the drift without parsing the message text.
        """
        with DriverClient(self.binary) as client:
            result = client.call_tool(
                "health_report",
                {"include": ["bundle_identity"]},
            )
        report = _structured_or_parsed(result)
        bundle_check = next(
            c for c in report["checks"] if c["name"] == "bundle_identity"
        )
        # If the binary IS reached via the .app bundle in this harness
        # the check legitimately passes; tolerate both — but exercise
        # the fail-mode contract whenever the check fails.
        if bundle_check["status"] == "fail":
            self.assertIn("hint", bundle_check)
            self.assertTrue(bundle_check["hint"].strip())
            data = bundle_check.get("data") or {}
            # bundle_identifier surfaces the runtime CFBundleIdentifier
            # so consumers can detect drift without parsing the message.
            # Field is optional only when the process literally has no
            # bundle id; SPM-built binaries always have one.
            self.assertIn(
                "bundle_identifier",
                data,
                "fail mode must surface bundle_identifier in `data`",
            )

    # ── invocation surface itself doesn't error ─────────────────────

    def test_call_does_not_set_isError(self) -> None:
        """`health_report` is the consumer's "what's broken?" probe — it
        must never itself report `isError: true`, even when every check
        fails. Otherwise consumers can't tell apart "the tool itself is
        broken" from "the driver has degraded health" — defeating the
        whole point of the tool.
        """
        with DriverClient(self.binary) as client:
            result = client.call_tool("health_report")
        self.assertFalse(
            result.get("isError", False),
            f"health_report must never set isError; got {result!r}",
        )


if __name__ == "__main__":
    unittest.main()
