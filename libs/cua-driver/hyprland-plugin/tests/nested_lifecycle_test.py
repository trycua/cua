"""Control-flow tests; native lifecycle evidence comes from nested_lifecycle.py."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import nested_lifecycle as lifecycle


class LifecycleTest(unittest.TestCase):
    def test_failed_launch_closes_log(self):
        with tempfile.TemporaryDirectory() as root:
            compositor = lifecycle.NestedCompositor(
                Path(root) / "child", "Hyprland", Path("plugin.so")
            )
            with patch.object(subprocess, "Popen", side_effect=FileNotFoundError):
                with self.assertRaises(FileNotFoundError):
                    compositor.start()
            self.assertTrue(compositor.log.closed)
            self.assertIsNone(compositor.process)

    def test_ctl_requires_owned_instance(self):
        with tempfile.TemporaryDirectory() as root:
            compositor = lifecycle.NestedCompositor(
                Path(root) / "child", "Hyprland", Path("plugin.so")
            )
            with self.assertRaisesRegex(RuntimeError, "no instance"):
                compositor.ctl("reload")

    def test_stop_without_process_is_safe(self):
        with tempfile.TemporaryDirectory() as root:
            compositor = lifecycle.NestedCompositor(
                Path(root) / "child", "Hyprland", Path("plugin.so")
            )
            compositor.stop()

    def test_forced_shutdown_is_failure_and_reaps_owned_process(self):
        with tempfile.TemporaryDirectory() as root:
            compositor = lifecycle.NestedCompositor(
                Path(root) / "child", "Hyprland", Path("plugin.so")
            )
            process = Mock()
            process.wait.side_effect = [subprocess.TimeoutExpired("Hyprland", 15), 0]
            log = Mock()
            compositor.process, compositor.log = process, log
            with self.assertRaisesRegex(RuntimeError, "forced termination"):
                compositor.stop()
            process.terminate.assert_called_once()
            process.kill.assert_called_once()
            self.assertEqual(process.wait.call_count, 2)
            log.close.assert_called_once()
            self.assertIsNone(compositor.process)

    @patch.dict(os.environ, {"HYPRLAND_INSTANCE_SIGNATURE": "parent", "WAYLAND_DISPLAY": "parent"})
    @patch.object(lifecycle, "command", return_value='{"version":"test"}')
    @patch.object(lifecycle, "NestedCompositor")
    def test_start_failure_stops_both_owned_children(self, factory, _command):
        first, second = Mock(), Mock()
        factory.side_effect = [first, second]
        second.start.side_effect = RuntimeError("start failed")
        with self.assertRaisesRegex(RuntimeError, "start failed"):
            lifecycle.run(Path("plugin.so"), Path("logs"), "Hyprland")
        first.stop.assert_called_once()
        second.stop.assert_called_once()

    @patch.dict(os.environ, {"HYPRLAND_INSTANCE_SIGNATURE": "parent", "WAYLAND_DISPLAY": "parent"})
    @patch.object(lifecycle, "command", return_value='{"version":"test"}')
    @patch.object(lifecycle, "NestedCompositor")
    def test_cleanup_failure_still_stops_other_child(self, factory, _command):
        first, second = Mock(), Mock()
        factory.side_effect = [first, second]
        first.start.side_effect = RuntimeError("start failed")
        second.stop.side_effect = RuntimeError("cleanup failed")
        with self.assertRaisesRegex(RuntimeError, "cleanup failed"):
            lifecycle.run(Path("plugin.so"), Path("logs"), "Hyprland")
        first.stop.assert_called_once()

    @patch.dict(os.environ, {}, clear=True)
    def test_no_parent_fails_before_launch(self):
        with patch.object(lifecycle, "NestedCompositor") as factory:
            with self.assertRaisesRegex(RuntimeError, "parent session"):
                lifecycle.run(Path("plugin.so"), Path("logs"), "Hyprland")
            factory.assert_not_called()


if __name__ == "__main__":
    unittest.main()
