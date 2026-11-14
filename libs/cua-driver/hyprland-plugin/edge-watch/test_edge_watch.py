"""Unit tests for the Omarchy edge watch helpers (no network)."""

import copy
import json
from pathlib import Path
import sys
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import edge_watch as ew  # noqa: E402

QUALIFIED = json.loads(ew.QUALIFIED.read_text())


def snapshot(hyprland="0.56.2-4", plugin="0.33.2-4"):
    return {"channels": {"omarchy-edge": {
        "packages": {"hyprland": {"version": hyprland}},
        "omarchy_plugin": {"version": plugin}}}, "errors": {}}


DB = {"omarchy-dev": {"version": "4.0.0.r7437.g7f4b1dd-1"},
      "omarchy-settings-dev": {"version": "4.0.0.r7437.g7f4b1dd-1", "filename": "s.pkg.tar.zst"}}

LUA = """
-- hl.config({ misc = { commented_out = true } })
hl.config({
  general = { gaps_in = 5, col = { active_border = "red" } },
  misc = {
    disable_hyprland_logo = true,
    bell_sound = "x = {",
  },
})
hl.bind("SUPER + Q", function() end)
"""


class CheckTest(unittest.TestCase):
    def test_unchanged(self):
        r = ew.check(snapshot(), QUALIFIED, "", DB)
        self.assertFalse(r["changed"])
        self.assertFalse(r["post_update"])

    def test_new_hyprland_and_config_package(self):
        db = copy.deepcopy(DB)
        db["omarchy-settings-dev"]["version"] = "4.0.0.r7562.g4907fcd-1"
        r = ew.check(snapshot("0.57.0-3"), QUALIFIED, "", db)
        self.assertTrue(r["changed"])
        self.assertEqual(len(r["reasons"]), 2)
        self.assertEqual(r["pin"], "0.56.2-4")

    def test_unreadable_repository_is_not_a_change(self):
        self.assertFalse(ew.check(snapshot(plugin=""), QUALIFIED, "", {})["changed"])

    def test_post_update_when_omarchy_ships_our_release(self):
        r = ew.check(snapshot(plugin="0.38.0-1"), QUALIFIED, "0.38.0", DB)
        self.assertTrue(r["changed"] and r["post_update"])
        self.assertFalse(ew.check(snapshot(plugin="0.33.2-4"), QUALIFIED, "0.38.0", DB)["post_update"])


class LintTest(unittest.TestCase):
    def test_config_keys_ignore_comments_and_strings(self):
        self.assertEqual(ew.config_keys(LUA), [
            "general:gaps_in", "general:col:active_border",
            "misc:disable_hyprland_logo", "misc:bell_sound"])

    def test_unknown_key_is_flagged(self):
        known = ew.binary_keys(b"\0misc:disable_hyprland_logo\0general:gaps_in\0general:col.active_border\0")
        self.assertEqual(ew.unknown_keys({"looknfeel.lua": LUA}, known), [("looknfeel.lua", "misc:bell_sound")])

    def test_sections_the_binary_does_not_know_are_left_alone(self):
        self.assertEqual(ew.unknown_keys({"a.lua": "hl.config({ other = { x = 1 } })"}, {"misc:vrr"}), [])


class BumpTest(unittest.TestCase):
    def test_bump_moves_pin_and_records_versions(self):
        watch = ew.check(snapshot("0.57.0-3", "0.38.0-1"), QUALIFIED, "0.38.0", DB)
        out = ew.bump(copy.deepcopy(QUALIFIED), watch, "https://example.test/run", "2026-11-03", True)
        self.assertEqual(out["plugin_pin"]["hyprland"], "0.57.0-3")
        self.assertEqual(out["last_qualified"]["omarchy_plugin"], "0.38.0-1")
        self.assertEqual(out["stopgaps"], [])
        self.assertFalse(ew.check(snapshot("0.57.0-3", "0.38.0-1"), out, "0.38.0", DB)["changed"])

    def test_report_dry_run_names_one_issue(self):
        calls = []

        def run(cmd, **kw):
            calls.append(cmd)

            class Done:
                stdout = '{"jobs": [], "artifacts": []}' if "api" in cmd else ""
                returncode = 0
            return Done()

        self.assertEqual(ew.report("o/r", "1", run=run), "opened issue")
        self.assertTrue(any("create" in c and ew.ISSUE_TITLE in c for c in calls))


if __name__ == "__main__":
    unittest.main()
