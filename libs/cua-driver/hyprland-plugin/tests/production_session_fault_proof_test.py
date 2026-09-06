"""Portable synthetic orchestration checks, never native DPMS/lock certification."""
from copy import deepcopy
from contextlib import nullcontext
import json
import os
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_session_fault_proof as proof


MONITOR = {'id': 0, 'name': 'Virtual-1', 'width': 1280, 'height': 800,
           'x': 0, 'y': 0, 'scale': 1.0, 'transform': 0}
BOUNDS = {'x': 10, 'y': 20, 'width': 800, 'height': 600}
PARTIAL = {'structuredContent': {'effect': 'partial', 'route': 'synthetic_events',
                               'delivery': {'mode': 'background', 'delivered_count': 1}}}
REFUSED = {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'session_unavailable'}}


def identity(pid, exe='/usr/bin/python3'):
    return {'pid': pid, 'uid': 1000, 'starttime': '123', 'exe': exe}


def plan():
    return {'purpose': 'session_fault', 'disposable': True, 'fault': {'kind': 'dpms'},
            'vm': {'machine_id': '1' * 32, 'boot_id': '12345678-1234-1234-1234-123456789abc'},
            'compositor': {**identity(50, '/usr/bin/Hyprland'), 'instance': 'test_1'},
            'foreground': {'pid': 10, 'window_id': 100}, 'primary_point': [20, 20],
            'identities': {'foreground': identity(10), 'target': identity(20, '/usr/bin/soffice.bin')},
            'foreground_fixture': {'sha256': 'a' * 64, 'journal': {'path': '/test/foreground.jsonl',
                'device': 1, 'inode': 2, 'uid': 1000}}, 'monitors': [dict(MONITOR)],
            'agents': [{'app': 'calc', 'name': 'session', 'target': {'pid': 20, 'window_id': 200},
                'bounds': dict(BOUNDS), 'pointer_stage': 'select_range', 'drag': {}}],
            'recovery': {'pointer_stage': 'click_b2'}}


def status(generation=1, held=False):
    return {'configured': True, 'transport': {'ready': True}, 'input': {
        'protocol': 3, 'test_only': False, 'seat_lifetime': 'compositor',
        'upgrade': 'desktop_restart', 'transport_ready': True,
        'lanes': [{'lane': lane, 'epoch': str(lane + 1) * 32, 'desktop_generation': generation,
            'dispatches': 0, 'held_button': 272 if held and lane == 0 else 0,
            'held_keys': 0, 'drag_active': held and lane == 0, 'lease_active': held and lane == 0,
            'pointer_focus': held and lane == 0, 'keyboard_focus': False, 'reserved': held and lane == 0}
            for lane in (0, 1)]}}


ACTIVE = [(0, 'start', 0, 0), (1, 'agent_admitted', 1, 0), (2, 'agent_drag_start', 1, 0),
          (3, 'pointer_button', 1, 1), (4, 'pointer_motion', 1, 0)]
CANCEL = ACTIVE + [(8, 'agent_cancel', 1, 0), (9, 'pointer_button', 1, 0), (10, 'pointer_leave', 1, 0)]


def trace(rows):
    return {'hook': True, 'active': True, 'overflow': False, 'timed_out': False, 'count': len(rows),
            'events': [[i + 1, ms * 1_000_000, kind, 100, 100, lane, value]
                       for i, (ms, kind, lane, value) in enumerate(rows)]}


def fault_record():
    return {'result': 'observed', 'prefix': trace(ACTIVE), 'lane': 1, 'requested_ns': 6_000_000,
            'acknowledged_ns': 7_000_000, 'observed_ns': 12_000_000,
            'gate_status': status(1, held=True), 'after': status(2), 'watchdog_deadline_ns': 20_000_000,
            'monitors_before': [{**MONITOR, 'dpmsStatus': True}],
            'monitors_off': [{**MONITOR, 'dpmsStatus': False}]}


def refusal_record():
    return {'outcome': 'response', 'replayed': False, 'response': deepcopy(REFUSED),
            'before': status(2), 'after': status(2), 'trace_before': trace(CANCEL), 'trace_after': trace(CANCEL),
            'prepared_ns': 1_000_000, 'dispatch_ns': 13_000_000, 'observed_ns': 14_000_000,
            'deadline_ns': 20_000_000, 'monitors_before': [{**MONITOR, 'dpmsStatus': False}],
            'monitors_after': [{**MONITOR, 'dpmsStatus': False}]}


class PlanTests(unittest.TestCase):
    def test_exact_dpms_calc_plan_only(self):
        proof.validate_plan(plan())
        for change in ({'disposable': False}, {'purpose': 'apps'}, {'fault': {'kind': 'lock'}},
                       {'fault': {'kind': 'dpms', 'unlock': True}}, {'monitors': []},
                       {'recovery': {'pointer_stage': 'select_range'}}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.validate_plan({**plan(), **change})
        with self.assertRaisesRegex(AssertionError, 'session-lock'):
            proof.validate_plan({**plan(), 'fault': {'kind': 'lock'}})

    def test_foreground_and_target_process_bindings_are_required(self):
        for field, value in [('pid', 99), ('uid', -1), ('starttime', 'unknown'), ('exe', 'relative')]:
            candidate = plan()
            candidate['identities']['target'][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.validate_plan(candidate)

    def test_unsupported_lock_writes_failed_result_without_native_setup(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'plan.json'
            path.write_text(json.dumps({**plan(), 'fault': {'kind': 'lock'}}))
            args = SimpleNamespace(plan=path, evidence=Path(directory) / 'evidence')
            with patch.object(proof, 'SessionFault') as controller, patch('builtins.print'):
                self.assertEqual(proof.run(args), 1)
            controller.assert_not_called()
            report = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(report['result'], 'failed')
            self.assertIn('session-lock', report['error'])

    def test_monitors_must_have_acknowledgeable_dpms_and_exact_identity(self):
        self.assertEqual(proof.monitor_identity([{**MONITOR, 'dpmsStatus': True}]), [MONITOR])
        for row in (MONITOR, {**MONITOR, 'dpmsStatus': 1}, {**MONITOR, 'dpmsStatus': None},
                    {**MONITOR, 'dpmsStatus': True, 'scale': float('nan')}):
            with self.subTest(row=row), self.assertRaises(AssertionError):
                proof.monitor_identity([row])
        config = {'instance': 'test_1', 'monitors': [MONITOR]}
        for row in ({**MONITOR, 'name': 'New-output', 'dpmsStatus': False},
                    {**MONITOR, 'width': 1920, 'dpmsStatus': False}, {**MONITOR, 'dpmsStatus': True}):
            with patch.object(proof, '_hypr', return_value=json.dumps([row])), self.assertRaises(AssertionError):
                proof.power(config, False)

    def test_host_wrong_boot_or_wrong_compositor_refused(self):
        candidate = plan()
        config = {'disposable': True, 'vm': candidate['vm'], 'instance': 'test_1',
                  'compositor': identity(50, '/usr/bin/Hyprland')}
        for failure in ('host', 'boot', 'not_vm', 'session', 'process', None):
            with self.subTest(failure=failure), \
                 patch.object(proof.platform, 'system', return_value='Darwin' if failure == 'host' else 'Linux'), \
                 patch.object(proof, 'guest_identity', return_value={} if failure == 'boot' else candidate['vm']), \
                 patch.object(proof.subprocess, 'run', return_value=SimpleNamespace(returncode=1 if failure == 'not_vm' else 0)), \
                 patch.dict(os.environ, {'HYPRLAND_INSTANCE_SIGNATURE': 'wrong' if failure == 'session' else 'test_1'}), \
                 patch.object(proof.os, 'getuid', return_value=1000), \
                 patch.object(proof, '_same_compositor', side_effect=AssertionError('changed') if failure == 'process' else None):
                if failure:
                    with self.assertRaises(AssertionError):
                        proof.guard_guest(config)
                else:
                    proof.guard_guest(config)


class OracleTests(unittest.TestCase):
    def test_cancel_requires_real_partial_or_unknown_and_owned_release(self):
        action = {'outcome': 'response', 'response': PARTIAL, 'replayed': False}
        self.assertEqual(proof.verify_cancelled(trace(CANCEL), fault_record(), action)['result'], 'verified')
        self.assertEqual(proof.verify_cancelled(trace(CANCEL), fault_record(),
            {'outcome': 'unknown', 'replayed': False})['outcome']['kind'], 'unknown')
        for rows in (ACTIVE, ACTIVE + [(8, 'pointer_button', 1, 0)],
                     ACTIVE + [(8, 'agent_cancel', 2, 0), (9, 'pointer_button', 1, 0)],
                     ACTIVE + [(8, 'pointer_button', 1, 0), (9, 'agent_cancel', 1, 0)],
                     CANCEL + [(11, 'pointer_motion', 1, 0)], CANCEL + [(11, 'agent_drag_end', 1, 0)]):
            with self.subTest(rows=rows), self.assertRaises(AssertionError):
                proof.verify_cancelled(trace(rows), fault_record(), action)
        with self.assertRaises(AssertionError):
            proof.verify_cancelled(trace(CANCEL), fault_record(), {**action, 'response': REFUSED})

    def test_dpms_has_no_primary_focus_input_or_warp_suppression(self):
        action = {'outcome': 'response', 'response': PARTIAL, 'replayed': False}
        for kind in ('cursor', 'pointer_focus', 'keyboard_focus', 'pointer_leave', 'pointer_button', 'keyboard_key', 'pointer_axis'):
            page = trace(CANCEL + [(11, kind, 0, 0), (12, 'cursor', 0, 0)])
            if kind == 'cursor':
                page['events'][-2][3] += 1  # Warp and return must still fail.
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                proof.verify_cancelled(page, fault_record(), action)
        for key, value in [('hook', False), ('overflow', True), ('timed_out', True), ('count', 0), ('active', False)]:
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_cancelled({**trace(CANCEL), key: value}, fault_record(), action)

    def test_status_is_production_v3_and_lifecycle_revokes_both_lanes(self):
        proof.transition(status(1, held=True), status(2))
        for key, value in [('epoch', 'new'), ('desktop_generation', 1), ('held_button', 272),
                           ('held_keys', 1), ('reserved', True), ('lease_active', True), ('pointer_focus', True)]:
            changed = status(2)
            changed['input']['lanes'][1][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.transition(status(1), changed)
        for key, value in [('test_only', True), ('protocol', 0), ('transport_ready', False)]:
            changed = status(2)
            changed['input'][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.lanes(changed)

    def test_unobserved_power_or_watchdog_expiry_cannot_certify_cancellation(self):
        action = {'outcome': 'response', 'response': PARTIAL, 'replayed': False}
        for change in ({'monitors_off': []}, {'monitors_off': [{**MONITOR, 'dpmsStatus': True}]},
                       {'watchdog_deadline_ns': 11_000_000}, {'requested_ns': 300_000_000}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.verify_cancelled(trace(CANCEL), {**fault_record(), **change}, action)

    def test_refusal_requires_exact_reason_zero_dispatch_and_unavailable_interval(self):
        self.assertEqual(proof.verify_refusal(refusal_record())['no_dispatch'], 'verified')
        for change in ({'outcome': 'unknown'}, {'replayed': True}, {'response': PARTIAL},
                       {'observed_ns': 21_000_000}, {'prepared_ns': -6_000_000_000},
                       {'monitors_after': [{**MONITOR, 'dpmsStatus': True}]}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.verify_refusal({**refusal_record(), **change})
        for key in ('dispatches', 'desktop_generation', 'held_button'):
            changed = refusal_record()
            changed['after']['input']['lanes'][0][key] += 1
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_refusal(changed)
        for kind in ('pointer_enter', 'agent_admitted', 'agent_cancel', 'pointer_button'):
            changed = refusal_record()
            changed['trace_after'] = trace(CANCEL + [(11, kind, 1, 0)])
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                proof.verify_refusal(changed)


class GroundingTests(unittest.TestCase):
    def test_interrupted_state_is_observed_after_restoration_without_input(self):
        observer, guard, save = Mock(), Mock(), Mock()
        spec = plan()['agents'][0]
        action = {'outcome': 'response', 'response': PARTIAL, 'replayed': False}
        restoration = {'result': 'restored', 'emergency': False, 'observed_ns': 10}
        snapshot = {'proof_observation_started_ns': 20, 'proof_image': 'interrupted.png'}
        with patch.object(proof, 'grounded_snapshot', return_value=snapshot) as observed, \
             patch.object(proof.time, 'monotonic_ns', return_value=30):
            record = proof.preserve_interrupted_state(observer, spec, action, restoration, guard, save)
        observed.assert_called_once_with(observer, spec['target'], spec, session=False)
        save.assert_called_once_with('interrupted-state.json', record)
        self.assertIs(record['action'], action)
        self.assertIs(record['snapshot'], snapshot)
        self.assertEqual(record['restoration_observed_ns'], 10)
        self.assertEqual(record['observed_ns'], 30)
        self.assertEqual(guard.call_count, 2)
        observer.tool.assert_not_called()

    def test_interrupted_state_refuses_emergency_wake_and_stale_capture(self):
        for failure in ('emergency', 'not_restored', 'stale_capture', 'replayed'):
            restoration = {'result': 'restored', 'emergency': False, 'observed_ns': 10}
            action = {'replayed': failure == 'replayed'}
            if failure == 'emergency':
                restoration['emergency'] = True
            if failure == 'not_restored':
                restoration['result'] = 'unproven'
            snapshot = {'proof_observation_started_ns': 5 if failure == 'stale_capture' else 20}
            save = Mock()
            with self.subTest(failure=failure), \
                 patch.object(proof, 'grounded_snapshot', return_value=snapshot), \
                 patch.object(proof.time, 'monotonic_ns', return_value=30), \
                 self.assertRaises(AssertionError):
                proof.preserve_interrupted_state(Mock(), plan()['agents'][0], action, restoration, Mock(), save)
            save.assert_not_called()

    def test_one_observation_grounds_both_same_window_pixel_actions_without_input(self):
        clients = [Mock(process=Mock(pid=pid, poll=Mock(return_value=None))) for pid in (100, 101)]
        spec = plan()['agents'][0]
        prepared, probe = {'prepared_ns': 100}, {'prepared_ns': 200}
        def drag(client, received):
            self.assertIs(client, clients[0])
            self.assertIs(received, spec)
            return prepared
        def refusal(grounding, received, stage):
            self.assertIs(grounding, prepared)
            self.assertIs(received, spec)
            self.assertEqual(stage, 'click_b2')
            return probe
        save = Mock()
        with patch.object(proof, 'prepare_drag', side_effect=drag), \
             patch.object(proof, 'prepare_refusal', side_effect=refusal):
            self.assertEqual(proof.prepare_actions(clients, spec, 'click_b2', save), (prepared, probe))
        self.assertEqual([call.args for call in save.call_args_list],
                         [('drag-grounding.json', prepared), ('refusal-grounding.json', probe)])
        for client in clients:
            client.tool.assert_not_called()

    def test_failed_observation_never_dispatches_or_saves_a_complete_pair(self):
        clients = [Mock(process=Mock(pid=pid, poll=Mock(return_value=None))) for pid in (100, 101)]
        save = Mock()
        with patch.object(proof, 'prepare_drag', side_effect=AssertionError('bad snapshot')), \
             patch.object(proof, 'prepare_refusal', return_value={}):
            with self.assertRaisesRegex(AssertionError, 'bad snapshot'):
                proof.prepare_actions(clients, plan()['agents'][0], 'click_b2', save)
        save.assert_not_called()
        for client in clients:
            client.tool.assert_not_called()

    def test_shared_runtime_is_rejected_before_observation(self):
        client = Mock(process=Mock(pid=100, poll=Mock(return_value=None)))
        with patch.object(proof, 'prepare_drag') as drag, patch.object(proof, 'prepare_refusal') as refusal:
            with self.assertRaises(AssertionError):
                proof.prepare_actions([client, client], plan()['agents'][0], 'click_b2', Mock())
        drag.assert_not_called()
        refusal.assert_not_called()

    def test_refusal_keeps_original_observation_time(self):
        for observed_ns in (100, 120):
            spec = plan()['agents'][0]
            snapshot = {**spec['target'], 'window_bounds': spec['bounds'],
                        'proof_image': '/synthetic/agent.png',
                        'proof_observation_started_ns': observed_ns}
            prepared = {'snapshot': snapshot, 'target': spec['target'], 'prepared_ns': observed_ns}
            with patch.object(proof.time, 'monotonic_ns', side_effect=AssertionError('must not reset timestamp')), \
                 patch.object(proof.pointer_grounding, 'read_pixels', return_value='pixels'), \
                 patch.object(proof.pointer_grounding, 'action', return_value=({'x': 1, 'y': 2}, {})):
                result = proof.prepare_refusal(prepared, spec, 'click_b2')
            self.assertEqual(result, {'snapshot': snapshot, 'arguments': {'x': 1, 'y': 2},
                                     'session': 'session-unavailable',
                                     'prepared_ns': observed_ns})

    def test_shared_observation_requires_exact_identity_geometry_and_time(self):
        spec = plan()['agents'][0]
        original = {'snapshot': {**spec['target'], 'window_bounds': spec['bounds'],
                     'proof_observation_started_ns': 100}, 'target': spec['target'], 'prepared_ns': 100}
        for change in ('target', 'pid', 'window_id', 'bounds', 'timestamp', 'missing_timestamp'):
            prepared = deepcopy(original)
            if change == 'target': prepared['target']['pid'] += 1
            elif change in ('pid', 'window_id'): prepared['snapshot'][change] += 1
            elif change == 'bounds': prepared['snapshot']['window_bounds']['x'] += 1
            elif change == 'timestamp': prepared['prepared_ns'] += 1
            else: del prepared['snapshot']['proof_observation_started_ns']
            with self.subTest(change=change), patch.object(proof.pointer_grounding, 'read_pixels') as pixels:
                with self.assertRaises((AssertionError, KeyError)):
                    proof.prepare_refusal(prepared, spec, 'click_b2')
                pixels.assert_not_called()

    def test_stale_or_future_probe_is_retained_without_dispatch(self):
        for prepared_ns in (-proof.MAX_GROUNDING_AGE_NS, 101):
            client, save = Mock(process=Mock(pid=100)), Mock()
            fault = Mock(config={'deadline_ns': 1_000_000_000},
                         unavailable=Mock(return_value=[{**MONITOR, 'dpmsStatus': False}]))
            with patch.object(proof.time, 'monotonic_ns', return_value=100), \
                 patch.object(proof, 'production_status', return_value=status(2)):
                with self.assertRaisesRegex(AssertionError, 'refusal grounding expired'):
                    proof.refuse(client, plan()['agents'][0], {'prepared_ns': prepared_ns},
                                 fault, Mock(collect=Mock(return_value=trace(CANCEL))), Mock(), save)
            client.tool.assert_not_called()
            self.assertEqual(save.call_args.args[0], 'unavailable-action.json')
            self.assertEqual(save.call_args.args[1]['outcome'], 'unknown')
            self.assertFalse(save.call_args.args[1]['replayed'])


class WatchdogTests(unittest.TestCase):
    def test_injection_is_gated_by_fresh_pending_drag_and_acks_exact_power(self):
        def wait_once(predicate, timeout):
            result = predicate()
            if not result:
                raise TimeoutError('DPMS off unsupported')
            return result
        for failure in (None, 'stale', 'done', 'lost_reply', 'no_off'):
            with self.subTest(failure=failure):
                fault = object.__new__(proof.SessionFault)
                fault.config = {'instance': 'test_1', 'deadline_ns': 20_000_000}
                fault.record, fault.mutated = {}, False
                fault.check_targets, fault.live_deadline = Mock(), Mock()
                pending = Mock(done=Mock(return_value=failure == 'done'))
                with patch.object(proof, 'control_lock', return_value=nullcontext()), \
                     patch.object(proof, 'power', side_effect=[[{**MONITOR, 'dpmsStatus': True}],
                                                             [{**MONITOR, 'dpmsStatus': failure == 'no_off'}]]), \
                     patch.object(proof, 'production_status', side_effect=[status(1, held=True), status(2)]), \
                     patch.object(proof, 'poll_active', return_value=(trace(ACTIVE), {1: 2_000_000})), \
                     patch.object(proof.time, 'monotonic_ns', return_value=300_000_000 if failure == 'stale' else 6_000_000), \
                     patch.object(proof, 'wait_for', side_effect=wait_once), \
                     patch.object(proof, '_hypr', return_value='ok',
                                  side_effect=RuntimeError('IPC lost') if failure == 'lost_reply' else None) as dispatch:
                    if failure:
                        with self.assertRaises((AssertionError, RuntimeError, TimeoutError)):
                            fault.inject(Mock(), trace(ACTIVE), pending, Mock())
                        if failure in ('stale', 'done'):
                            dispatch.assert_not_called()
                            self.assertFalse(fault.mutated)
                        else:
                            self.assertTrue(fault.mutated, 'lost reply still requires restoration')
                    else:
                        self.assertEqual(fault.inject(Mock(), trace(ACTIVE), pending, Mock()), 1)
                        self.assertEqual(fault.record['result'], 'observed')
                        dispatch.assert_called_once_with('test_1', 'dispatch', 'hl.dsp.dpms({ action = "disable" })')

    def test_controller_eof_and_timeout_restore_but_cancel_does_not(self):
        for trigger in ('eof', 'timeout', 'cancel', 'failure'):
            with self.subTest(trigger=trigger), tempfile.TemporaryDirectory() as directory:
                reader, writer = os.pipe()
                config = {'deadline_ns': time.monotonic_ns() + (0 if trigger == 'timeout' else 1_000_000_000),
                          'watchdog_path': str(Path(directory) / 'watchdog.json')}
                if trigger == 'cancel':
                    os.write(writer, b'C')
                if trigger in ('eof', 'failure'):
                    os.close(writer)
                    writer = None
                try:
                    with patch.object(proof, 'restore_power', return_value={'result': 'restored', 'emergency': True},
                                      side_effect=RuntimeError('wrong boot') if trigger == 'failure' else None) as restore:
                        proof.watchdog(config, reader)
                        if trigger == 'cancel':
                            restore.assert_not_called()
                            self.assertFalse(Path(config['watchdog_path']).exists())
                        else:
                            restore.assert_called_once_with(config, emergency=True)
                            result = json.loads(Path(config['watchdog_path']).read_text())
                            self.assertEqual(result['result'], 'failed' if trigger == 'failure' else 'restored')
                finally:
                    os.close(reader)
                    if writer is not None:
                        os.close(writer)

    def test_expired_watchdog_marker_or_dead_child_blocks_new_fault(self):
        for failure in (None, 'deadline', 'marker', 'exited'):
            with tempfile.TemporaryDirectory() as directory:
                fault = object.__new__(proof.SessionFault)
                marker = Path(directory) / 'restored'
                fault.config = {'deadline_ns': time.monotonic_ns() + (0 if failure == 'deadline' else 5_000_000_000),
                                'restored_path': str(marker)}
                fault.child = Mock(poll=Mock(return_value=0 if failure == 'exited' else None))
                if failure == 'marker':
                    marker.touch()
                if failure:
                    with self.subTest(failure=failure), self.assertRaises(AssertionError):
                        fault.live_deadline()
                else:
                    fault.live_deadline()

    def test_restore_verifies_monitor_set_before_any_wake_and_marks_terminal(self):
        with tempfile.TemporaryDirectory() as directory:
            lock = Path(directory) / 'lock'
            lock.touch()
            info = lock.stat()
            config = {'lock_path': str(lock), 'lock_identity': [info.st_dev, info.st_ino, info.st_uid],
                      'restored_path': str(Path(directory) / 'restored'), 'instance': 'test_1'}
            with patch.object(proof, 'guard_guest'), patch.object(proof, 'power', side_effect=AssertionError('new output')), \
                 patch.object(proof, '_hypr') as dispatch, self.assertRaisesRegex(AssertionError, 'new output'):
                proof.restore_power(config)
            dispatch.assert_not_called()
            self.assertFalse(Path(config['restored_path']).exists())
            with patch.object(proof, 'guard_guest'), \
                 patch.object(proof, 'power', side_effect=[[{**MONITOR, 'dpmsStatus': False}], [{**MONITOR, 'dpmsStatus': True}]]), \
                 patch.object(proof, 'production_status', return_value=status(3)), \
                 patch.object(proof, '_hypr', return_value='ok') as dispatch:
                self.assertEqual(proof.restore_power(config)['result'], 'restored')
            dispatch.assert_called_once_with('test_1', 'dispatch', 'hl.dsp.dpms({ action = "enable" })')
            self.assertTrue(Path(config['restored_path']).exists())

    def test_failed_restore_leaves_eof_recovery_armed(self):
        fault = object.__new__(proof.SessionFault)
        fault.mutated, fault.config = True, {}
        with patch.object(proof, 'restore_power', side_effect=RuntimeError('IPC lost')), self.assertRaises(RuntimeError):
            fault.restore()
        self.assertTrue(fault.mutated)
        reader, fault.cancel_fd = os.pipe()
        fault.child = None
        try:
            fault.close()
            self.assertEqual(os.read(reader, 1), b'')
        finally:
            os.close(reader)

    def test_successful_restore_disarms_and_reaps_before_app_recovery(self):
        fault = object.__new__(proof.SessionFault)
        fault.mutated, fault.config = True, {}
        reader, fault.cancel_fd = os.pipe()
        child = fault.child = Mock(returncode=0)
        try:
            def restored(config):
                self.assertTrue(fault.mutated)
                child.wait.assert_not_called()
                return {'result': 'restored', 'emergency': False}
            with patch.object(proof, 'restore_power', side_effect=restored):
                self.assertEqual(fault.restore()['result'], 'restored')
            self.assertFalse(fault.mutated)
            self.assertEqual(os.read(reader, 2), b'C')
            self.assertIsNone(fault.cancel_fd)
            self.assertIsNone(fault.child)
            child.wait.assert_called_once_with(timeout=10)
            child.stdout.close.assert_called_once()
            fault.close()
            self.assertEqual(fault.restore(), {'result': 'not_needed'})
            child.wait.assert_called_once()
        finally:
            os.close(reader)


if __name__ == '__main__':
    unittest.main()
