"""Safety/recovery contracts; no compositor or host desktop is touched."""
import json
from pathlib import Path
import signal
import tempfile
import unittest
from unittest.mock import Mock, patch

import desktop_faults as faults


class DesktopFaultTest(unittest.TestCase):
    def controller(self, directory):
        value = faults.FaultController.__new__(faults.FaultController)
        value.instance = 'selected-instance'
        value.target = {'pid': 42, 'address': '0xab'}
        value.compositor = {'pid': 10, 'starttime': '77', 'uid': 1000, 'exe': '/usr/bin/Hyprland'}
        value.fixture = {'pid': 42, 'starttime': '88', 'uid': 1000, 'exe': '/usr/bin/python3'}
        value.fixture_fd = 7
        value.evidence = Path(directory)
        value.move_to, value.resize_to = (110, 130), (680, 560)
        value.hold_seconds = 12
        value.kind = value.before = value.lock_process = value.watchdog = None
        value.cancel_fd = value.lock_fd = value.restoration = None
        value.lock_state, value.lock_buffer = 'unknown', b''
        value.lock_events, value.lock_ack_ns = [], None
        value.mutated = False
        return value

    def state(self, **target):
        return {'target': {'pid': 42, 'address': '0xab', 'at': [70, 100],
                           'size': [600, 500], 'floating': True, **target},
                'monitors': [{'name': 'virtual-1', 'dpmsStatus': True}],
                'lock': 'unknown', 'observed_ns': 1}

    def test_host_and_missing_confirmation_rejected_before_commands(self):
        with patch('desktop_faults.platform.system', return_value='Darwin'), \
             patch('desktop_faults.subprocess.run') as run:
            for disposable in (False, True):
                with self.assertRaises(ValueError):
                    faults.FaultController(compositor_pid=10, instance='test', target={},
                                           disposable=disposable, evidence=Path('/unused'))
            run.assert_not_called()

    def test_bare_metal_rejected_before_process_lookup(self):
        with patch('desktop_faults.platform.system', return_value='Linux'), \
             patch('desktop_faults.subprocess.run', return_value=Mock(returncode=1)), \
             patch('desktop_faults._identity') as identity:
            with self.assertRaisesRegex(ValueError, 'virtual machine'):
                faults.FaultController(compositor_pid=10, instance='test', target={},
                                       disposable=True, evidence=Path('/unused'))
            identity.assert_not_called()

    def test_stale_compositor_identity_rejected_before_hyprctl(self):
        with patch('desktop_faults._identity', return_value={'starttime': 'replacement'}), \
             patch('desktop_faults.subprocess.check_output') as command:
            with self.assertRaisesRegex(RuntimeError, 'identity changed'):
                faults._same_compositor({'pid': 10, 'starttime': 'original'}, 'instance')
            command.assert_not_called()

    def test_instance_must_match_pid(self):
        original = {'pid': 10, 'starttime': '77'}
        with patch('desktop_faults._identity', return_value=original), \
             patch('desktop_faults.subprocess.check_output', return_value='[{"instance":"other","pid":10}]'):
            with self.assertRaisesRegex(RuntimeError, 'instance'):
                faults._same_compositor(original, 'selected')

    def test_address_reuse_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            with patch('desktop_faults._same_compositor'), \
                 patch('desktop_faults._hypr', return_value='[{"address":"0xab","pid":43}]'):
                with self.assertRaisesRegex(RuntimeError, 'ownership'):
                    value.snapshot()

    def test_fixture_pidfd_exit_prevents_retargeting(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            with patch('desktop_faults.select.select', return_value=([7], [], [])), \
                 patch('desktop_faults._identity') as identity:
                with self.assertRaisesRegex(ValueError, 'exited'):
                    value._validate_fixture()
                identity.assert_not_called()

    def test_foreground_fixture_is_not_a_fault_target(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            fixture = Path(faults.__file__).resolve().parents[2] / 'tests/fixtures/apps/linux/isolated-input/main.py'
            argv = f'python3\0{fixture}\0--actor\0Foreground\0--journal\0test.json\0'.encode()
            with patch('desktop_faults.select.select', return_value=([], [], [])), \
                 patch('desktop_faults._identity', return_value=value.fixture), \
                 patch('desktop_faults.os.getuid', return_value=1000), \
                 patch('desktop_faults.Path.read_bytes', return_value=argv):
                with self.assertRaisesRegex(ValueError, 'Background'):
                    value._validate_fixture()

    def test_destroy_signals_only_bound_pidfd_and_records_disappearance(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            absent = {**self.state(), 'target': None}
            with patch.object(value, '_validate_fixture'), \
                 patch.object(value, 'snapshot', side_effect=[self.state(), absent, absent]), \
                 patch('desktop_faults.select.select', return_value=([7], [], [])), \
                 patch('desktop_faults.signal.pidfd_send_signal', create=True) as send:
                record = value.apply('destroy')
            send.assert_called_once_with(7, signal.SIGTERM)
            self.assertTrue(record['acknowledged'])
            self.assertIsNone(record['after']['target'])
            self.assertLessEqual(record['fault_ns'], record['observed_ns'])
            self.assertEqual(json.loads((Path(directory) / 'fault.json').read_text()), record)

    def test_move_requires_nontrivial_floating_target(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            with patch.object(value, '_validate_fixture'), \
                 patch.object(value, 'snapshot', return_value=self.state(floating=False)), \
                 patch.object(value, '_dispatch') as dispatch:
                with self.assertRaisesRegex(ValueError, 'already-floating'):
                    value.apply('move')
                value.rollback()
            dispatch.assert_not_called()

    def test_move_ack_and_restore_exact_selected_address(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            original, moved = self.state(), self.state(at=[110, 130])
            with patch.object(value, '_validate_fixture'), \
                 patch.object(value, 'snapshot', side_effect=[original, moved, moved, original, original]), \
                 patch.object(value, '_dispatch') as dispatch:
                value.apply('move')
                restored = value.rollback()
                self.assertIs(value.rollback(), restored)
            self.assertEqual(dispatch.call_args_list[0].args,
                             ('movewindowpixel', 'exact 110 130,address:0xab'))
            self.assertEqual(dispatch.call_args_list[1].args,
                             ('movewindowpixel', 'exact 70 100,address:0xab'))
            self.assertTrue(restored['restored'])

    def test_dpms_rejects_initial_off_without_forcing_on_in_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            state = self.state()
            state['monitors'][0]['dpmsStatus'] = False
            with patch.object(value, '_validate_fixture'), \
                 patch.object(value, 'snapshot', return_value=state), \
                 patch.object(value, '_dispatch') as dispatch, \
                 patch.object(value, '_arm') as arm:
                with self.assertRaisesRegex(ValueError, 'acknowledged on'):
                    value.apply('dpms')
                value.rollback()
            arm.assert_not_called()
            dispatch.assert_not_called()

    def test_dpms_watchdog_arms_before_mutation_and_survives_failed_ack(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            order = []
            with patch.object(value, '_validate_fixture'), \
                 patch.object(value, 'snapshot', return_value=self.state()), \
                 patch.object(value, '_arm', side_effect=lambda kind: order.append('armed')), \
                 patch.object(value, '_dispatch', side_effect=lambda *args: order.append(args)), \
                 patch('desktop_faults._wait', side_effect=TimeoutError('no ack')):
                with self.assertRaises(TimeoutError):
                    value.apply('dpms')
            self.assertEqual(order, ['armed', ('dpms', 'off')])
            self.assertTrue(value.mutated)
            record = json.loads((Path(directory) / 'fault.json').read_text())
            self.assertFalse(record['acknowledged'])
            self.assertEqual(record['error'], 'no ack')

    def test_failed_restoration_never_cancels_watchdog(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            value.kind, value.mutated, value.cancel_fd = 'dpms', True, 8
            with patch.object(value, '_dispatch', side_effect=RuntimeError('unavailable')), \
                 patch('desktop_faults.os.write') as write:
                with self.assertRaises(RuntimeError):
                    value.rollback()
            write.assert_not_called()
            self.assertIsNone(value.restoration)
            self.assertFalse(json.loads((Path(directory) / 'restoration.json').read_text())['restored'])

    def test_lock_recovery_uses_graceful_signal_not_termination(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            value.kind, value.mutated, value.lock_fd = 'lock', True, 9
            value.lock_process = Mock()
            value.lock_process.poll.return_value = None
            value.lock_process.wait.return_value = 0
            with patch.object(value, '_lock_event', return_value={'event': 'unlocked'}) as acknowledgment, \
                 patch.object(value, 'snapshot', return_value=self.state()), \
                 patch('desktop_faults.signal.pidfd_send_signal', create=True) as send:
                self.assertTrue(value.rollback()['restored'])
            send.assert_called_once_with(9, signal.SIGUSR1)
            acknowledgment.assert_called_once_with('unlocked')
            value.lock_process.terminate.assert_not_called()
            value.lock_process.kill.assert_not_called()

    def test_watchdog_controller_death_restores_dpms_with_readback(self):
        with tempfile.TemporaryDirectory() as directory:
            config = {'kind': 'dpms', 'seconds': 12, 'compositor': {'pid': 10},
                      'instance': 'selected', 'record': str(Path(directory) / 'watchdog.json')}
            with patch('desktop_faults.select.select', return_value=([8], [], [])), \
                 patch('desktop_faults.os.read', return_value=b''), \
                 patch('desktop_faults._same_compositor'), \
                 patch('desktop_faults._hypr', side_effect=['ok', '[{"dpmsStatus":true}]']) as hypr:
                faults._watchdog(config, 8, -1)
            self.assertEqual(hypr.call_args_list[0].args, ('selected', 'dispatch', 'dpms', 'on'))
            self.assertTrue(json.loads(Path(config['record']).read_text())['restored'])

    def test_watchdog_cancel_does_not_mutate_desktop(self):
        with patch('desktop_faults.select.select', return_value=([8], [], [])), \
             patch('desktop_faults.os.read', return_value=b'C'), \
             patch('desktop_faults._same_compositor') as verify:
            faults._watchdog({'seconds': 12}, 8, -1)
        verify.assert_not_called()

    def test_dpms_monitor_set_change_is_not_restoration(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            value.before = self.state()
            changed = self.state()
            changed['monitors'][0]['name'] = 'replacement'
            with patch.object(value, 'snapshot', return_value=changed):
                self.assertFalse(value._dpms(True))

    def test_arbitrary_dispatch_is_rejected_before_compositor_access(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            with patch('desktop_faults._same_compositor') as identity:
                for command, argument in [('exec', 'anything'), ('dpms', 'toggle'),
                                          ('movewindowpixel', 'exact 1 2,address:0xcd')]:
                    with self.assertRaises(ValueError):
                        value._dispatch(command, argument)
            identity.assert_not_called()

    def test_buffered_unlock_acknowledgment_is_not_lost(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            value.lock_process = Mock()
            value.lock_buffer = (b'{"event":"locked","observed_ns":12}\n'
                                 b'{"event":"unlocked","observed_ns":15}\n')
            self.assertEqual(value._lock_event('locked')['observed_ns'], 12)
            self.assertEqual(value._lock_event('unlocked')['observed_ns'], 15)
            self.assertEqual(value.lock_state, 'unlocked')

    def test_resize_restores_exact_size(self):
        with tempfile.TemporaryDirectory() as directory:
            value = self.controller(directory)
            original, resized = self.state(), self.state(size=[680, 560])
            with patch.object(value, '_validate_fixture'), \
                 patch.object(value, 'snapshot', side_effect=[original, resized, resized, original, original]), \
                 patch.object(value, '_dispatch') as dispatch:
                value.apply('resize')
                self.assertTrue(value.rollback()['restored'])
            self.assertEqual(dispatch.call_args_list[1].args,
                             ('resizewindowpixel', 'exact 600 500,address:0xab'))


if __name__ == '__main__':
    unittest.main()
