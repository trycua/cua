import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from input_config_toggle import DISABLED, ENABLED, InputConfigToggle


class InputConfigToggleTest(unittest.TestCase):
    def test_toggle_and_restore_verify_compositor_state(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.lua"
            path.write_text(ENABLED)
            toggle = InputConfigToggle(path)
            replies = ["ok", json.dumps({"configured": False, "experiment": {"transport_ready": False}}),
                       "ok", json.dumps({"configured": True, "experiment": {"transport_ready": True}})]
            with patch("input_config_toggle.subprocess.check_output", side_effect=replies) as ctl:
                toggle.disable()
                self.assertEqual(path.read_text(), DISABLED)
                toggle.restore()
                self.assertEqual(path.read_text(), ENABLED)
                toggle.restore()
                self.assertEqual(ctl.call_count, 4)

    def test_unsourced_or_changed_include_cannot_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.lua"
            path.write_text("unrelated configuration")
            with self.assertRaises(ValueError):
                InputConfigToggle(path)
            path.write_text(ENABLED)
            toggle = InputConfigToggle(path)
            with patch("input_config_toggle.subprocess.check_output", side_effect=["ok", "{}"]):
                with self.assertRaises(RuntimeError):
                    toggle.disable()
            self.assertTrue(toggle.changed)
            path.write_text("unexpected edit")
            with self.assertRaises(RuntimeError):
                toggle.restore()

    def test_symlink_is_not_a_test_owned_include(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "target.lua"
            target.write_text(ENABLED)
            link = Path(directory) / "input.lua"
            link.symlink_to(target)
            with self.assertRaises(ValueError):
                InputConfigToggle(link)
