"""Portable independent-observer contracts; no native qualification is claimed."""
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch
from types import SimpleNamespace

from primary_observer import (MAX_BYTES, PrimaryObserver, analyze, journal_rows,
                              primary_wire_state, sync_barrier, verify_negative_control, wire_rows)
from production_realapp_proof import run
from production_realapp_proof_test import plan


def wire(*lines):
    return ''.join('[12345.000] {Default Queue} ' + line + '\n' for line in lines).encode()


SYNC = wire(' -> wl_display#1.sync(new id wl_callback#20)', 'wl_callback#20.done(1)',
            ' -> wl_display#1.sync(new id wl_callback#21)', 'wl_callback#21.done(2)')
BASE = wire('wl_pointer#5.enter(1, wl_surface#10, 300.00000000, 300.00000000)',
            'wl_keyboard#6.enter(2, wl_surface#10, array[0])',
            'wl_keyboard#6.modifiers(3, 0, 0, 0, 0)', 'wl_pointer#5.button(4, 100, 272, 1)')
CANARY = wire('wl_pointer#5.motion(101, 340.00000000, 330.00000000)',
              'wl_pointer#5.motion(102, 300.00000000, 300.00000000)')


def observation(events=b''):
    state = {'clicks': 0, 'keys': '', 'scroll': 0, 'motion': 0, 'held': True,
             'buttons': [1], 'keys_down': [], 'window_active': True, 'canvas_focus': True}
    def row(seq, kind, when, **values):
        return {'seq': seq, 'instance': 'fixture', 'kind': kind, 'time': when, **values}
    data = BASE + SYNC + events + SYNC
    begin = row(2, 'sync', 2_000_000_000, **state, nonce='a' * 32,
                wire_start=len(BASE), wire_end=len(BASE + SYNC))
    end = row(4, 'sync', 2_200_000_000, **state, nonce='b' * 32,
              wire_start=len(BASE + SYNC + events), wire_end=len(data))
    rows = [row(1, 'ready', 1_900_000_000, native_wayland=True), begin,
            row(3, 'state', 2_100_000_000, **state), end]
    before = {'identity': {'pid': 10}, 'marker': begin}
    after = {'identity': {'pid': 10}, 'marker': end}
    return before, after, rows, data, [(2_020_000_000, 2_080_000_000)], {'cursor': [300, 300]}, {'cursor': [300, 300]}


class ObserverAnalysisTests(unittest.TestCase):
    def test_native_clock_timestamps_preserve_wire_event_fields(self):
        data = (b'[02:42:49.666527] {Default Queue}  -> xdg_wm_base#42.pong(1337)\n'
                b'[02:42:51.166558] {Default Queue} xdg_wm_base#42.ping(1337)\n')
        self.assertEqual(wire_rows(data), [
            {'out': True, 'interface': 'xdg_wm_base', 'object': 42, 'event': 'pong', 'arguments': '1337'},
            {'out': False, 'interface': 'xdg_wm_base', 'object': 42, 'event': 'ping', 'arguments': '1337'},
        ])

    def test_native_clock_boundaries_and_legacy_numeric_timestamps(self):
        for timestamp in (b'00:00:00.0', b'23:59:59.999999', b'19:09:09.123',
                          b'0.0', b'12345.000', b' 123456789.123456'):
            with self.subTest(timestamp=timestamp):
                native_sync = SYNC.replace(b'12345.000', timestamp)
                self.assertEqual(wire_rows(native_sync), wire_rows(SYNC))
                sync_barrier(native_sync)
                self.assertEqual(primary_wire_state(BASE.replace(b'12345.000', timestamp)),
                                 primary_wire_state(BASE))
                with self.assertRaisesRegex(AssertionError, 'missing complete'):
                    sync_barrier(native_sync.replace(b'wl_callback#21.done', b'wl_callback#22.done'))

    def test_malformed_timestamps_fail_without_skipping_records(self):
        for timestamp in (b'24:00:00.000000', b'99:00:00.000000', b'00:60:00.000000',
                          b'00:00:60.000000', b'2:42:49.666527', b'02:2:49.666527',
                          b'02:42:9.666527', b'02:42:49', b'02:42:49.', b'02:42:49.x',
                          b'02:42:49.123Z', b'-02:42:49.123', b'02:42:49.123 ',
                          b'12345', b'12345.', b'.000', b'-12345.000'):
            with self.subTest(timestamp=timestamp):
                bad = CANARY.replace(b'12345.000', timestamp, 1)
                with self.assertRaisesRegex(AssertionError, 'unparseable Wayland wire record'):
                    wire_rows(SYNC + bad + SYNC)

    def test_normal_interval_passes_without_claiming_compositor_attribution(self):
        result = analyze(*observation())
        self.assertEqual(result['result'], 'passed')
        self.assertEqual(result['primary']['position'], [300, 300])
        self.assertEqual(result['primary']['held_button'], 272)
        self.assertFalse(result['compositor_attribution'])
        self.assertTrue(result['complete'])

    def test_warp_and_return_fails_identical_detector_even_if_gtk_coalesces_it(self):
        values = observation(CANARY)
        # Journal and endpoint counters intentionally remain identical.
        result = analyze(*values)
        self.assertEqual(result['result'], 'failed')
        self.assertTrue(verify_negative_control(result)['verified'])
        self.assertEqual(result['motions'][-1]['position'], result['primary']['position'])
        with self.assertRaisesRegex(AssertionError, 'did not fail'):
            verify_negative_control(analyze(*observation()))

    def test_focus_grab_keys_buttons_axis_and_any_motion_are_not_endpoint_evidence(self):
        events = (
            'wl_pointer#5.axis(101, 0, 10.0)', 'wl_pointer#5.axis_discrete(0, 1)',
            'wl_pointer#5.axis_value120(0, 120)', 'wl_pointer#5.axis_source(0)',
            'wl_pointer#5.button(5, 101, 272, 0)', 'wl_pointer#5.leave(5, wl_surface#10)',
            'wl_pointer#5.enter(5, wl_surface#10, 300.0, 300.0)',
            'wl_keyboard#6.key(5, 101, 30, 1)', 'wl_keyboard#6.leave(5, wl_surface#10)',
            'wl_keyboard#6.modifiers(5, 1, 0, 0, 0)', 'wl_seat#4.capabilities(0)',
            'zwp_relative_pointer_v1#8.relative_motion(0, 100, 2.0, 0.0, 2.0, 0.0)',
        )
        for event in events:
            with self.subTest(event=event):
                result = analyze(*observation(wire(event)))
                self.assertEqual(result['result'], 'failed')
                with self.assertRaises(AssertionError):
                    verify_negative_control(result)
        for kind in ('focus-change', 'grab-broken', 'leave-notify', 'button-release', 'key-release', 'scroll'):
            values = observation()
            values[2][2]['kind'] = kind
            self.assertEqual(analyze(*values)['result'], 'failed')

    def test_control_must_return_on_same_pointer_and_fail_only_for_motion(self):
        for events in (wire('wl_pointer#5.motion(101, 340.0, 330.0)'),
                       CANARY + wire('wl_keyboard#6.key(5, 101, 30, 1)'),
                       CANARY.replace(b'wl_pointer#5', b'wl_pointer#7')):
            with self.assertRaises(AssertionError):
                verify_negative_control(analyze(*observation(events)))
        values = observation(CANARY)
        values[-1]['cursor'] = [301, 300]
        with self.assertRaisesRegex(AssertionError, 'unrelated'):
            verify_negative_control(analyze(*values))

    def test_full_interval_fresh_heartbeats_and_sync_callbacks_are_mandatory(self):
        for mutation in (
            lambda v: v[1]['identity'].update(pid=11),
            lambda v: v[1]['marker'].update(nonce='a' * 32),
            lambda v: v[1]['marker'].update(time=4_000_000_000),
            lambda v: v[4].append((1_999_999_999, 2_100_000_000)),
            lambda v: v[4].append((2_100_000_000, 2_300_000_000)),
            lambda v: v[4].clear(),
            lambda v: v[0]['marker'].update(held=False),
            lambda v: v[0]['marker'].update(canvas_focus=False),
            lambda v: v[0]['marker'].update(wire_end=len(BASE)),
        ):
            values = observation()
            mutation(values)
            with self.assertRaises(AssertionError):
                analyze(*values)
        for data in (b'', SYNC.replace(b'wl_callback#21.done(2)\n', b''),
                     SYNC.replace(b'wl_callback#21.done', b'wl_callback#22.done')):
            with self.assertRaises(AssertionError):
                sync_barrier(data)

    def test_baseline_requires_one_native_surface_with_proven_pointer_grab_and_idle_keyboard(self):
        for data in (BASE.replace(b'272, 1', b'272, 0'),
                     BASE.replace(b'array[0]', b'array[4]'),
                     BASE.replace(b'modifiers(3, 0, 0, 0, 0)', b'modifiers(3, 1, 0, 0, 0)'),
                     BASE.replace(b'wl_keyboard#6.enter(2, wl_surface#10', b'wl_keyboard#6.enter(2, wl_surface#11'),
                     BASE + wire('wl_pointer#7.enter(8, wl_surface#10, 1.0, 1.0)')):
            with self.assertRaises(AssertionError):
                primary_wire_state(data)

    def test_raw_evidence_cannot_be_partial_malformed_reordered_or_from_another_producer(self):
        rows = observation()[2]
        data = b''.join((json.dumps(row) + '\n').encode() for row in rows)
        self.assertEqual(journal_rows(data), rows)
        for bad in (data[:-1], data.replace(b'"seq": 3', b'"seq": 4'),
                    data.replace(b'2100000000', b'1000000000'),
                    data.replace(b'"instance": "fixture"', b'"instance": "other"', 1)):
            with self.assertRaises(AssertionError):
                journal_rows(bad)
        for bad in (CANARY[:-1], CANARY + b'logger dropped events\n', b'x' * (MAX_BYTES + 1)):
            with self.assertRaises(AssertionError):
                wire_rows(bad)


class ObserverFileTests(unittest.TestCase):
    def test_fixture_help_is_portable_and_optimized_execution_is_refused(self):
        fixture = Path(__file__).with_name('primary_observer_fixture.py')
        help_text = subprocess.check_output([sys.executable, str(fixture), '--help'], text=True, timeout=5)
        self.assertIn('--control', help_text)
        self.assertIn('--wire', help_text)
        optimized = subprocess.run([sys.executable, '-O', str(fixture), '--help'], text=True,
                                   capture_output=True, timeout=5)
        self.assertNotEqual(optimized.returncode, 0)
        self.assertIn('assertions must be enabled', optimized.stderr)

    def test_sync_nonce_peer_source_and_freshness_are_all_bound(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / 'fixture.py'
            source.write_text('synthetic fixture')
            for fault in (None, 'peer', 'uid', 'nonce', 'stale', 'future', 'source', 'truncated', 'process'):
                with self.subTest(fault=fault):
                    observer = object.__new__(PrimaryObserver)
                    observer.control = Mock(lstat=Mock(return_value=SimpleNamespace(st_mode=stat.S_IFSOCK, st_uid=os.getuid())))
                    observer.foreground, observer.source = {'pid': 10}, source
                    observer.process_start = 'original'
                    observer._process_identity = Mock(return_value='replacement' if fault == 'process' else 'original')
                    client = Mock()
                    client.__enter__ = Mock(return_value=client)
                    client.__exit__ = Mock(return_value=False)
                    client.getsockopt.return_value = struct.pack('3i', 11 if fault == 'peer' else 10,
                                                                os.getuid() + (fault == 'uid'), 0)
                    def response(size):
                        nonce = json.loads(client.sendall.call_args.args[0])['nonce']
                        packet = {'identity': {'pid': 10, 'uid': os.getuid(), 'native_wayland': True,
                                  'instance': 'f' * 32,
                                  'source_sha256': '0' * 64 if fault == 'source' else hashlib.sha256(source.read_bytes()).hexdigest()},
                                  'marker': {'instance': 'f' * 32, 'nonce': 'x' * 32 if fault == 'nonce' else nonce,
                                  'time': 999 if fault == 'stale' else 3000 if fault == 'future' else 1500}}
                        return json.dumps(packet).encode(), [], socket.MSG_TRUNC if fault == 'truncated' else 0, None
                    client.recvmsg.side_effect = response
                    with patch('primary_observer.socket.socket', return_value=client), \
                            patch('primary_observer.socket.SO_PEERCRED', 17, create=True), \
                            patch('primary_observer.time.monotonic_ns', side_effect=[1000, 2000]):
                        if fault is None:
                            self.assertEqual(observer._sync()['controller_interval'], [1000, 2000])
                        else:
                            with self.assertRaises(AssertionError):
                                observer._sync()

    def test_inode_prefix_size_and_recording_limits_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            path = root / 'journal'
            path.write_bytes(b'old\n')
            info = path.stat()
            packet = {'identity': {'journal': {'path': str(path), 'device': info.st_dev, 'inode': info.st_ino}}}
            observer = object.__new__(PrimaryObserver)
            observer.journal, observer.descriptors, observer.prefixes = path, {}, {}
            try:
                self.assertEqual(observer._read('journal', packet, 4), b'old\n')
                observer.prefixes['journal'] = b'old\n'
                path.write_bytes(b'new\n')
                with self.assertRaisesRegex(AssertionError, 'history changed'):
                    observer._read('journal', packet, 4)
                path.write_bytes(b'o')
                with self.assertRaisesRegex(AssertionError, 'truncated'):
                    observer._read('journal', packet, 4)
                path.rename(root / 'original')
                path.write_bytes(b'old\n')
                with self.assertRaisesRegex(AssertionError, 'replaced'):
                    observer._read('journal', packet, 4)
            finally:
                observer.close()


class ObserverGateTests(unittest.TestCase):
    def test_production_app_and_control_intervals_close_before_primary_release(self):
        for purpose, detection in (('apps', 'passed'), ('apps', 'failed'),
                                   ('negative_control', 'failed'), ('negative_control', 'passed')):
            with self.subTest(purpose=purpose, detection=detection), tempfile.TemporaryDirectory() as temporary, ExitStack() as stack:
                root = Path(temporary)
                candidate = plan()
                candidate['purpose'] = purpose
                for i, spec in enumerate(candidate['agents']):
                    spec.update(name=f'agent-{i}', profile={'mode': 'standard'},
                                bounds={'x': 0, 'y': 0, 'width': 600, 'height': 600})
                candidate['phases'] = ([{'negative_control': True}] if purpose == 'negative_control' else
                                       [{'agent': i, 'tool': 'press_key', 'arguments': {'key': 'a'}} for i in range(2)])
                for i, oracle in enumerate(candidate['outputs']):
                    output = root / f'output-{i}'
                    output.write_bytes(b'baseline')
                    oracle['path'] = str(output)
                path = root / 'plan.json'
                path.write_text(json.dumps(candidate))
                args = SimpleNamespace(plan=path, evidence=root / 'evidence', artifact_role='production', trace_socket=None,
                                       primary_observer=root / 'control.sock', foreground_journal=root / 'journal',
                                       primary_grab=root / 'primary-grab', driver=root / 'driver', record_video=False)
                agents = [Mock(process=Mock(pid=101 + i, poll=Mock(return_value=None))) for i in range(2)]
                recorder = Mock()
                def tool(name, arguments):
                    if name == 'get_window_state':
                        return {'structuredContent': {'window_bounds': candidate['agents'][0]['bounds'], 'screenshot_width': 600}}
                    if name == 'get_desktop_state':
                        return {'structuredContent': {'screen_width': 800, 'screen_height': 800}}
                    if name == 'press_key':
                        return {'structuredContent': {'route': 'synthetic_events', 'effect': 'unverifiable',
                                                      'delivery': {'mode': 'background'}}}
                    return {'structuredContent': {}}
                for mcp in agents + [recorder]:
                    mcp.tool.side_effect = tool
                held, began = [True], []
                grab = Mock(poll=Mock(return_value=None))
                observer = Mock()
                observer.start.side_effect = lambda primary: began.append(time.monotonic_ns())
                result = analyze(*observation(CANARY if detection == 'failed' else b''))
                def finish(intervals, primary):
                    self.assertTrue(all(agent.close.called for agent in agents))
                    self.assertFalse(grab.terminate.called)
                    self.assertEqual(len(intervals), 2 if purpose == 'apps' else 1)
                    self.assertTrue(all(began[0] <= first < last <= time.monotonic_ns() for first, last in intervals))
                    return result
                observer.finish.side_effect = finish
                replacements = {'provenance': Mock(return_value={}), 'PrimaryObserver': Mock(return_value=observer),
                    'DirectMCP': Mock(side_effect=agents + [recorder]), 'subprocess.Popen': Mock(return_value=grab),
                    'subprocess.run': Mock(), 'primary_acknowledgement': Mock(return_value='HELD\n'),
                    'verify_output': Mock(return_value={'verified': True}),
                    'wait_for': lambda predicate: self.assertTrue(predicate()),
                    'state': lambda path: {'held': held[0], 'clicks': 0, 'keys': 0, 'scroll': 0},
                    'wm': lambda: {'pid': 10, 'address': '0x64', 'workspace': 1, 'cursor': {'x': 100, 'y': 200}},
                    'stop_process': lambda process: held.__setitem__(0, False)}
                for name, replacement in replacements.items():
                    stack.enter_context(patch('production_realapp_proof.' + name, replacement))
                should_pass = (purpose == 'apps') == (detection == 'passed')
                self.assertEqual(run(args), 0 if should_pass else 1)
                observer.start.assert_called_once()
                observer.finish.assert_called_once()
                observer.close.assert_called_once()
                report = json.loads((args.evidence / 'result.json').read_text())
                self.assertEqual(report['continuous_isolation'], 'unproven')
                self.assertEqual(report['synthetic_cleanup'], 'unproven')
                self.assertEqual(report['independent_primary_isolation']['result'], detection)
                if purpose == 'negative_control' and should_pass:
                    self.assertTrue(report['negative_control_detected'])
                    self.assertEqual(report['scope'], 'production-package-primary-control')
                self.assertFalse(held[0])

    def test_explicit_production_requires_observer_before_any_driver_process(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / 'plan.json'
            path.write_text(json.dumps(plan()))
            args = SimpleNamespace(plan=path, evidence=root / 'evidence', artifact_role='production', trace_socket=None)
            with patch('production_realapp_proof.provenance') as origin, patch('production_realapp_proof.DirectMCP') as spawn:
                self.assertEqual(run(args), 1)
                origin.assert_not_called()
                spawn.assert_not_called()
            result = json.loads((args.evidence / 'result.json').read_text())
            self.assertIn('requires the independent primary observer', result['error'])
            self.assertEqual(result['continuous_isolation'], 'unproven')


if __name__ == '__main__':
    unittest.main()
