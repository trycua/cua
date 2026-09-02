"""Tests for the movement-spec promotion tool."""

from __future__ import annotations

import json
import subprocess
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

TOOL = Path(__file__).resolve().parent / "promote_motion.py"


def write(path: Path, payload: dict) -> None:
    path.write_text(json.dumps(payload, indent=2) + "\n")


def spec_with(shared_overrides: dict | None = None) -> dict:
    shared = {
        "start_handle": 0.3,
        "end_handle": 0.3,
        "arc_size": 0.25,
        "arc_flow": 0.0,
        "spring": 0.72,
        "glide_duration_ms": 0.0,
        "dwell_after_click_ms": 80.0,
        "idle_hide_ms": 20000.0,
        "press_duration_ms": 120.0,
        "peak_speed": 900.0,
        "min_start_speed": 300.0,
        "min_end_speed": 200.0,
        "turn_radius": 80.0,
        "click_offset": 16.0,
    }
    shared.update(shared_overrides or {})
    return {
        "schema": "cua.cursor-motion/1",
        "shared": shared,
        "spring_settle": {
            "macos": {"mode": "fixed", "k": 400.0, "c": 17.0, "overshoot": 0.8},
            "windows_linux": {
                "mode": "derived",
                "k_per_spring": 400.0,
                "c_per_spring": 20.0,
                "overshoot": 0.5,
            },
        },
    }


class PromoteMotionTests(unittest.TestCase):
    def run_tool(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, str(TOOL), *args],
            capture_output=True,
            text=True,
        )

    def test_promotes_override_values_into_the_spec(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            override = root / "motion-override.json"
            spec = root / "motion.default.json"
            write(override, {"schema": "cua.cursor-gallery-motion-override/1",
                             "peak_speed": 640.0, "turn_radius": 110.0})
            original = spec_with()
            write(spec, original)

            result = self.run_tool("--override", str(override), "--spec", str(spec))
            self.assertEqual(result.returncode, 0, result.stderr)
            promoted = json.loads(spec.read_text())
            self.assertEqual(promoted["shared"]["peak_speed"], 640.0)
            self.assertEqual(promoted["shared"]["turn_radius"], 110.0)
            # Untouched values keep the spec defaults.
            self.assertEqual(promoted["shared"]["spring"], 0.72)
            self.assertEqual(promoted["spring_settle"], original["spring_settle"])

    def test_promotion_diff_is_value_only(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            override = root / "motion-override.json"
            spec = root / "motion.default.json"
            write(override, {"schema": "cua.cursor-gallery-motion-override/1",
                             "min_end_speed": 240.0})
            write(spec, spec_with())

            before = spec.read_text()
            self.run_tool("--override", str(override), "--spec", str(spec))
            after = spec.read_text()

            before_lines = {line.strip() for line in before.splitlines()}
            after_lines = {line.strip() for line in after.splitlines()}
            changed = before_lines - after_lines
            added = after_lines - before_lines
            self.assertEqual(changed, {'"min_end_speed": 200.0,'})
            self.assertEqual(added, {'"min_end_speed": 240.0,'})

    def test_validate_only_never_writes(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            override = root / "motion-override.json"
            spec = root / "motion.default.json"
            write(override, {"schema": "cua.cursor-gallery-motion-override/1",
                             "spring": 0.66})
            write(spec, spec_with())
            before = spec.read_text()

            result = self.run_tool("--override", str(override), "--spec", str(spec),
                                   "--validate-only")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("shared.spring: 0.72 -> 0.66", result.stdout)
            self.assertEqual(spec.read_text(), before)

    def test_rejects_bad_overrides(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            spec = root / "motion.default.json"
            write(spec, spec_with())

            empty = root / "empty.json"
            write(empty, {"schema": "cua.cursor-gallery-motion-override/1"})
            result = self.run_tool("--override", str(empty), "--spec", str(spec))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("no movement values", result.stderr)

            wrong_schema = root / "wrong.json"
            write(wrong_schema, {"schema": "cua.other/1", "peak_speed": 5.0})
            result = self.run_tool("--override", str(wrong_schema), "--spec", str(spec))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("schema", result.stderr)

            # Non-promotable field sneaking through is refused by the
            # promotion whitelist first...
            hostile = root / "hostile.json"
            write(hostile, {"schema": "cua.cursor-gallery-motion-override/1",
                            "idle_hide_ms": 5.0})
            result = self.run_tool("--override", str(hostile), "--spec", str(spec))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("non-promotable", result.stderr)
            # ...and a whitelisted field with a hostile value reaches the
            # shared dev-server sanitizer, which rejects it.
            sneaky = root / "sneaky.json"
            write(sneaky, {"schema": "cua.cursor-gallery-motion-override/1",
                           "peak_speed": "fast"})
            result = self.run_tool("--override", str(sneaky), "--spec", str(spec))
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("override rejected", result.stderr)
            self.assertEqual(spec.read_text(), json.dumps(spec_with(), indent=2) + "\n")

    def test_hand_edited_out_of_range_values_are_bounded_before_promotion(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            override = root / "motion-override.json"
            spec = root / "motion.default.json"
            # A hand-edited file carrying values outside every bound.
            write(override, {"schema": "cua.cursor-gallery-motion-override/1",
                             "peak_speed": 99999.0,
                             "min_start_speed": -50.0,
                             "min_end_speed": 0.0,
                             "turn_radius": 5000.0,
                             "spring": 4.0,
                             "glide_duration_ms": 90000.0})
            write(spec, spec_with())

            result = self.run_tool("--override", str(override), "--spec", str(spec))
            self.assertEqual(result.returncode, 0, result.stderr)
            promoted = json.loads(spec.read_text())
            self.assertEqual(promoted["shared"]["peak_speed"], 5000.0)
            self.assertEqual(promoted["shared"]["min_start_speed"], 1.0)
            self.assertEqual(promoted["shared"]["min_end_speed"], 1.0)
            self.assertEqual(promoted["shared"]["turn_radius"], 1000.0)
            self.assertEqual(promoted["shared"]["spring"], 1.0)
            self.assertEqual(promoted["shared"]["glide_duration_ms"], 5000.0)

            # Cross-field: floors capped to the overridden peak.
            capped = root / "capped.json"
            write(capped, {"schema": "cua.cursor-gallery-motion-override/1",
                           "peak_speed": 120.0,
                           "min_start_speed": 400.0,
                           "min_end_speed": 350.0})
            write(spec, spec_with())
            result = self.run_tool("--override", str(capped), "--spec", str(spec))
            self.assertEqual(result.returncode, 0, result.stderr)
            promoted = json.loads(spec.read_text())
            self.assertEqual(promoted["shared"]["min_start_speed"], 120.0)
            self.assertEqual(promoted["shared"]["min_end_speed"], 120.0)

    def test_no_change_leaves_file_untouched(self) -> None:
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            override = root / "motion-override.json"
            spec = root / "motion.default.json"
            write(override, {"schema": "cua.cursor-gallery-motion-override/1",
                             "peak_speed": 900.0})
            write(spec, spec_with())
            before = spec.read_text()

            result = self.run_tool("--override", str(override), "--spec", str(spec))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("nothing written", result.stdout)
            self.assertEqual(spec.read_text(), before)


if __name__ == "__main__":
    unittest.main()
