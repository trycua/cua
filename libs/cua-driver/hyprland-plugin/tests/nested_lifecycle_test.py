"""Control-flow tests; native lifecycle evidence comes from nested_lifecycle.py."""

import os
import json
import io
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, Mock, patch

import nested_lifecycle as lifecycle


class LifecycleTest(unittest.TestCase):
    def test_command_keeps_ipc_failure_labels_without_raw_output_or_arguments(self):
        failure = subprocess.CalledProcessError(
            6, ["hyprctl", "-i", "private-instance"],
            output="Hyprland IPC didn't respond in time\nCouldn't read (6)\nprivate-value")
        with patch.object(subprocess, "check_output", side_effect=failure) as call:
            with self.assertRaises(RuntimeError) as raised:
                lifecycle.command("hyprctl", "-i", "private-instance", "-j", "version")
        message = str(raised.exception)
        self.assertIn('"exit_code": 6', message)
        self.assertIn('"ipc_read_timeout": true', message)
        self.assertIn('"ipc_read_failure": true', message)
        self.assertNotIn("private-instance", message)
        self.assertNotIn("private-value", message)
        self.assertEqual(call.call_count, 1)

    def test_command_does_not_mislabel_another_read_error_as_timeout(self):
        failure = subprocess.CalledProcessError(6, ["hyprctl"], output="Couldn't read (6)")
        with patch.object(subprocess, "check_output", side_effect=failure):
            with self.assertRaises(RuntimeError) as raised:
                lifecycle.command("hyprctl", "-j", "version")
        self.assertIn('"ipc_read_timeout": false', str(raised.exception))

    def test_command_success_is_unchanged(self):
        with patch.object(subprocess, "check_output", return_value="value\n"):
            self.assertEqual(lifecycle.command("hyprctl", "version"), "value")

    @patch.dict(os.environ, {"HYPRLAND_INSTANCE_SIGNATURE": "parent", "WAYLAND_DISPLAY": "parent"})
    def test_parent_failure_stays_failed_after_diagnostic_and_cleanup(self):
        failure = RuntimeError("lifecycle command failed")
        for reply, expected in ((failure, RuntimeError), ('{"version":"changed"}', AssertionError)):
            for process_error, alive in ((ProcessLookupError, False), (PermissionError, None)):
                first, second = Mock(process=None), Mock(process=None)
                metadata = lambda name: {"transport": {"socket": name}, "compositor_epoch": name}
                first.start.return_value = metadata("first")
                second.start.side_effect = [metadata("second"), metadata("replacement")]
                connection = MagicMock()
                connection.__enter__.return_value.recv.return_value = b""
                diagnostics = io.StringIO()
                with patch.object(lifecycle, "NestedCompositor", side_effect=[first, second]), \
                     patch.object(lifecycle, "command", side_effect=[
                         '{"version":"test"}', '[{"instance":"parent","pid":123}]', reply]) as command, \
                     patch.object(lifecycle.live, "connect", return_value=connection), \
                     patch.object(lifecycle.live, "exchange", return_value=(4, b"")), \
                     patch.object(lifecycle.os, "kill", side_effect=process_error) as check_pid, \
                     patch.object(lifecycle.sys, "stderr", diagnostics):
                    with self.assertRaises(expected) as raised:
                        lifecycle.run(Path("plugin.so"), Path("logs"), "Hyprland")
                if reply is failure:
                    self.assertIs(raised.exception, failure)
                self.assertEqual(command.call_count, 3)
                check_pid.assert_called_once_with(123, 0)
                first.stop.assert_called_once()
                evidence = json.loads(diagnostics.getvalue())
                self.assertEqual(evidence["parent_process_exists"], alive)
                self.assertTrue(evidence["owned_processes_reaped"])

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
    @patch.object(lifecycle, "command", side_effect=['{"version":"test"}', '[{"instance":"parent","pid":123}]'])
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
    @patch.object(lifecycle, "command", side_effect=['{"version":"test"}', '[{"instance":"parent","pid":123}]'])
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
