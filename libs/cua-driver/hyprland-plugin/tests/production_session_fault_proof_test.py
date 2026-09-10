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
    def test_constructor_accepts_inert_baseline_but_not_reserved_input(self):
        for reserved in (False, True):
            with self.subTest(reserved=reserved), tempfile.TemporaryDirectory() as root:
                before = status()
                before['input']['lanes'][1].update(pointer_focus=True, reserved=reserved)
                args = SimpleNamespace(evidence=Path(root))
                with patch.object(proof.SessionFault, 'check_targets'), patch.object(proof, 'power'), \
                     patch.object(proof, 'production_status', return_value=before):
                    if reserved:
                        with self.assertRaisesRegex(AssertionError, 'authority'):
                            proof.SessionFault(plan(), args)
                    else:
                        fault = proof.SessionFault(plan(), args)
                        self.assertFalse(fault.mutated)
                        self.assertIsNone(fault.child)
                self.assertEqual(json.loads((args.evidence / 'pre-fault-status.json').read_text()), before)

    def test_passive_recovery_does_not_relax_transition_or_held_state_checks(self):
        passive = status(1)
        passive['input']['lanes'][0]['pointer_focus'] = True
        with self.assertRaises(AssertionError):
            proof.lanes(passive, cleared=True)
        proof.lanes(passive, cleared=True, allow_passive=True)
        for field, value in (('held_button', 272), ('held_keys', 1), ('drag_active', True),
                             ('lease_active', True), ('keyboard_focus', True), ('pointer_focus', 1)):
            altered = deepcopy(passive)
            altered['input']['lanes'][0][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.lanes(altered, cleared=True, allow_passive=True)

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
                                     'prepared_ns': observed_ns, 'tool': 'click'})

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


def retained_status(generation=2, lane=1):
    value = status(generation)
    value['input']['lanes'][lane - 1]['pointer_focus'] = True
    return value


def claimed_refusal(interrupted_lane=1, claimed_lane=0):
    value = refusal_record()
    value.update(pointer_cleanup='retained_inert', lane=interrupted_lane,
                 before=retained_status(2, interrupted_lane), after=retained_status(2, interrupted_lane),
                 after_close=retained_status(2, interrupted_lane), close_started_ns=15_000_000,
                 reaped_ns=15_500_000, exit_code=0, closed_ns=16_000_000,
                 monitors_after_close=[{**MONITOR, 'dpmsStatus': False}])
    value['response']['structuredContent']['lane'] = claimed_lane
    value['after']['input']['lanes'][claimed_lane]['reserved'] = True
    for key in ('trace_before', 'trace_after', 'trace_after_close'):
        value[key] = trace(CANCEL[:-1])
    return value


def motion_gate(record, lane=1):
    """A 13px surface-local movement, with the same held lane across status."""
    page = trace([(0, 'start', 0, 0), (1, 'agent_admitted', lane, 0),
                  (2, 'agent_drag_start', lane, 0), (2.5, 'pointer_enter', lane, 0),
                  (3, 'pointer_button', lane, 1), (4, 'pointer_motion', lane, 0)])
    for row in page['events']:
        row[1] = int(row[1])
        if row[2] in ('pointer_enter', 'pointer_motion'):
            row.extend([10 if row[2] == 'pointer_enter' else 23, 20])
    gate = status(1, held=True)
    if lane == 2:
        gate['input']['lanes'][0], gate['input']['lanes'][1] = gate['input']['lanes'][1], gate['input']['lanes'][0]
        for index, row in enumerate(gate['input']['lanes']):
            row['lane'], row['epoch'] = index, str(index + 1) * 32
    record.update(pointer_cleanup='retained_inert', min_motion_px=12,
                  prefix=page, gate_first=deepcopy(page), lane=lane,
                  status_started_ns=5_000_000, gate_status=gate, after=retained_status(2, lane))
    boundary = deepcopy(page)
    boundary['events'] += [[7, 8_000_000, 'agent_cancel', 100, 100, lane, 0],
                           [8, 9_000_000, 'pointer_button', 100, 100, lane, 0]]
    boundary['count'] = len(boundary['events'])
    return boundary


class RetainedPointerTests(unittest.TestCase):
    def test_fresh_claim_is_capacity_only_and_must_match_response_lane(self):
        for interrupted in (1, 2):
            for claimed in (0, 1):
                value = claimed_refusal(interrupted, claimed)
                self.assertEqual(proof.verify_refusal(value)['result'], 'verified')
                for failure in ('foreign_reservation', 'old_reservation', 'lease_active', 'held_button',
                                'held_keys', 'drag_active', 'keyboard_focus', 'dispatches',
                                'desktop_generation', 'epoch', 'pointer_focus', 'response_lane', 'missing_lane'):
                    bad = deepcopy(value)
                    if failure == 'foreign_reservation':
                        bad['after']['input']['lanes'][1 - claimed]['reserved'] = True
                    elif failure == 'old_reservation':
                        bad['before']['input']['lanes'][claimed]['reserved'] = True
                    elif failure == 'response_lane':
                        bad['response']['structuredContent']['lane'] = 1 - claimed
                    elif failure == 'missing_lane':
                        del bad['response']['structuredContent']['lane']
                    else:
                        row = bad['after']['input']['lanes'][interrupted - 1]
                        row[failure] = ('changed' if failure == 'epoch' else
                                        False if failure == 'pointer_focus' else
                                        row[failure] + 1 if type(row[failure]) is int else True)
                    with self.subTest(interrupted=interrupted, claimed=claimed, failure=failure), self.assertRaises(AssertionError):
                        proof.verify_refusal(bad)

    def test_probe_close_requires_all_capacity_released_while_still_off_and_quiet(self):
        value = claimed_refusal()
        self.assertEqual(proof.verify_refusal_close(value)['reservation_released'], True)
        for failure in ('reserved', 'lease_active', 'held_keys', 'held_button', 'keyboard_focus',
                        'drag_active', 'dispatches', 'desktop_generation', 'epoch', 'pointer_focus',
                        'leave', 'motion', 'enter', 'history', 'deadline', 'clock', 'power', 'unreaped'):
            bad = deepcopy(value)
            if failure in ('leave', 'motion', 'enter'):
                bad['trace_after_close'] = trace(CANCEL[:-1] + [(15, 'pointer_' + failure, 1, 0)])
            elif failure == 'history':
                bad['trace_after_close']['events'][-1][3] += 1
            elif failure == 'deadline':
                bad['closed_ns'] = bad['deadline_ns']
            elif failure == 'clock':
                bad['close_started_ns'] = bad['observed_ns'] - 1
            elif failure == 'power':
                bad['monitors_after_close'][0]['dpmsStatus'] = True
            elif failure == 'unreaped':
                bad['exit_code'] = None
            else:
                row = bad['after_close']['input']['lanes'][0]
                row[failure] = ('changed' if failure == 'epoch' else
                                False if failure == 'pointer_focus' else
                                row[failure] + 1 if type(row[failure]) is int else True)
            with self.subTest(failure=failure), self.assertRaises(AssertionError):
                proof.verify_refusal_close(bad)

    def test_retained_refusal_reaps_probe_and_reads_back_before_returning_to_restore(self):
        for failure in (None, 'reservation_survived', 'synthetic_close', 'runtime_live'):
            value = claimed_refusal()
            after_close = deepcopy(value['after_close'])
            if failure == 'reservation_survived':
                after_close['input']['lanes'][0]['reserved'] = True
            trace_after_close = value['trace_after_close']
            if failure == 'synthetic_close':
                trace_after_close = trace(CANCEL[:-1] + [(15, 'pointer_leave', 1, 0)])
            client = Mock(process=Mock(pid=100, poll=Mock(return_value=None if failure == 'runtime_live' else 0)),
                          tool=Mock(return_value=value['response']))
            fault = Mock(config={'pointer_cleanup': 'retained_inert', 'deadline_ns': 20_000_000},
                         record={'lane': 1, 'after': retained_status()},
                         unavailable=Mock(return_value=[{**MONITOR, 'dpmsStatus': False}]))
            events = []
            states = iter([value['before'], value['after'], after_close])
            def status_read(*args):
                events.append('status')
                return next(states)
            def close(runtime):
                self.assertIs(runtime, client)
                events.append('close')
            save = Mock()
            trace_client = Mock(collect=Mock(side_effect=[value['trace_before'], value['trace_after'], trace_after_close]))
            with self.subTest(failure=failure), patch.object(proof, 'production_status', side_effect=status_read), \
                 patch.object(proof, 'close_owned', side_effect=close), \
                 patch.object(proof.time, 'monotonic_ns', side_effect=[13_000_000, 14_000_000, 15_000_000, 15_500_000, 16_000_000]):
                args = (client, plan()['agents'][0], {'prepared_ns': 1_000_000, 'arguments': {}, 'session': 'probe'},
                        fault, trace_client, Mock(), save)
                if failure:
                    with self.assertRaises(AssertionError):
                        proof.refuse(*args)
                else:
                    result = proof.refuse(*args)
                    self.assertEqual(result['close_verification']['result'], 'verified')
                    self.assertEqual(events, ['status', 'status', 'close', 'status'])
            client.tool.assert_called_once()
            self.assertEqual(events.count('close'), 1)
            self.assertEqual(save.call_args.args[0], 'unavailable-action.json')
            fault.restore.assert_not_called()

    def test_cleanup_and_motion_are_independent_opt_ins(self):
        record = fault_record()
        boundary = motion_gate(record)
        del record['min_motion_px']
        proof.verify_cancelled(boundary, record, {'outcome': 'response', 'replayed': False, 'response': PARTIAL})
        record['min_motion_px'] = 12
        del record['pointer_cleanup']
        record['after'] = status(2)
        proof.verify_cancelled(boundary, record, {'outcome': 'response', 'replayed': False, 'response': PARTIAL})

    def test_optional_policies_and_finite_motion_do_not_relax_plan_scope(self):
        value = plan()
        value['fault'].update(pointer_cleanup='retained_inert', min_motion_px=12.5)
        proof.validate_plan(value)
        for change in ({'pointer_cleanup': 'anything'}, {'extra': True},
                       *({'min_motion_px': v} for v in (None, True, 0, -1, float('inf'), float('nan'), '12'))):
            bad = deepcopy(value)
            bad['fault'].update(change)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.validate_plan(bad)

    def test_both_lanes_keep_inert_presence_only_when_explicitly_selected(self):
        for lane in (1, 2):
            record = fault_record()
            boundary = motion_gate(record, lane)
            result = proof.verify_cancelled(boundary, record, {'outcome': 'response', 'replayed': False, 'response': PARTIAL})
            self.assertEqual(result['result'], 'verified')
            with self.assertRaises(AssertionError):
                proof.transition(record['gate_status'], record['after'])
            for key, value in (('held_button', 272), ('held_keys', 1), ('drag_active', True),
                               ('lease_active', True), ('keyboard_focus', True), ('reserved', True),
                               ('pointer_focus', False), ('dispatches', 1), ('epoch', 'changed'),
                               ('desktop_generation', 1)):
                bad = deepcopy(record)
                bad['after']['input']['lanes'][lane - 1][key] = value
                with self.subTest(lane=lane, key=key), self.assertRaises(AssertionError):
                    proof.transition(bad['gate_status'], bad['after'], 'retained_inert', lane)

    def test_held_gate_rejects_insufficient_stale_retargeted_and_changed_lane_evidence(self):
        record = fault_record()
        motion_gate(record)
        proof.verify_held_gate(record)
        for failure in ('insufficient', 'coordinates', 'stale_status', 'stale_trace',
                        'first_after_status', 'lane', 'leave', 'enter', 'released', 'unheld', 'history'):
            bad = deepcopy(record)
            if failure == 'insufficient':
                bad['min_motion_px'] = 14
            elif failure == 'coordinates':
                bad['prefix']['events'][-1] = bad['prefix']['events'][-1][:7]
            elif failure == 'stale_status':
                bad['status_started_ns'] = -300_000_000
            elif failure == 'stale_trace':
                bad['requested_ns'] = 300_000_000
            elif failure == 'first_after_status':
                bad['status_started_ns'] = 3_000_000
            elif failure == 'lane':
                bad['lane'] = 2
            elif failure in ('leave', 'enter', 'released'):
                kind = {'leave': 'pointer_leave', 'enter': 'pointer_enter', 'released': 'pointer_button'}[failure]
                bad['prefix']['events'].append([7, 5_000_000, kind, 100, 100, 1, 0])
                bad['prefix']['count'] += 1
            elif failure == 'unheld':
                bad['gate_status'] = status(1)
            else:
                bad['gate_first']['events'][-1][7] += 1
            with self.subTest(failure=failure), self.assertRaises(AssertionError):
                proof.verify_held_gate(bad)

    def test_cancel_refuses_leave_reentry_motion_and_cross_lane_cleanup(self):
        record = fault_record()
        boundary = motion_gate(record)
        action = {'outcome': 'response', 'replayed': False, 'response': PARTIAL}
        for kind, lane in (('pointer_leave', 1), ('pointer_enter', 1), ('pointer_motion', 1), ('pointer_leave', 2)):
            bad = deepcopy(boundary)
            bad['events'].append([9, 10_000_000, kind, 100, 100, lane, 0])
            bad['count'] += 1
            with self.subTest(kind=kind, lane=lane), self.assertRaises(AssertionError):
                proof.verify_cancelled(bad, record, action)

    def test_refusal_and_restoration_require_unchanged_presence_and_no_events(self):
        value = claimed_refusal()
        proof.verify_refusal(value)
        proof.verify_refusal_close(value)
        proof.transition(value['after_close'], retained_status(3), 'retained_inert', 1)
        proof.verify_stable_inert(value['after_close'], retained_status(), 1)
        for key, change in (('epoch', 'replaced'), ('desktop_generation', 3), ('reserved', True)):
            bad = retained_status()
            bad['input']['lanes'][0][key] = change
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_stable_inert(value['after_close'], bad, 1)
        for key, field in (('before', 'reserved'), ('after', 'held_keys'),
                           ('after', 'dispatches'), ('after', 'pointer_focus')):
            bad = deepcopy(value)
            bad[key]['input']['lanes'][0][field] = False if field == 'pointer_focus' else (1 if field in ('held_keys', 'dispatches') else True)
            with self.subTest(key=key, field=field), self.assertRaises(AssertionError):
                proof.verify_refusal(bad)
        before = trace(CANCEL[:-1])
        proof.verify_inert_interval(before, before)
        for kind in ('pointer_leave', 'pointer_enter', 'pointer_motion', 'agent_admitted'):
            after = trace(CANCEL[:-1] + [(15, kind, 1, 0)])
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                proof.verify_inert_interval(before, after)

    def test_dpms_motion_gate_brackets_status_before_one_fault_dispatch(self):
        for failure in (None, 'insufficient', 'changed_lane', 'generation', 'stale_status', 'timeout'):
            fixture = object.__new__(proof.SessionFault)
            fixture.config = {'instance': 'test', 'deadline_ns': 20_000_000,
                              'pointer_cleanup': 'retained_inert', 'min_motion_px': 12}
            fixture.record = {'before': status(1), 'pointer_cleanup': 'retained_inert', 'min_motion_px': 12}
            fixture.mutated = False
            fixture.check_targets = fixture.live_deadline = Mock()
            record = fault_record()
            motion_gate(record)
            first, page = deepcopy(record['prefix']), deepcopy(record['prefix'])
            gate = record['gate_status']
            if failure == 'insufficient':
                first['events'][-1][7] = page['events'][-1][7] = 15
            if failure == 'changed_lane':
                for row in page['events'][1:]:
                    row[5] = 2
            if failure == 'generation':
                gate['input']['lanes'][0]['desktop_generation'] += 1
            calls = []
            def poll(*args, **kwargs):
                calls.append('trace')
                selected = first if calls.count('trace') == 1 else page
                return selected, proof.active_drags(selected)
            def read(*args):
                calls.append('status')
                return gate if calls.count('status') == 1 else retained_status()
            with self.subTest(failure=failure), \
                 patch.object(proof, 'control_lock', return_value=nullcontext()), \
                 patch.object(proof, 'power', side_effect=[[{**MONITOR, 'dpmsStatus': True}], [{**MONITOR, 'dpmsStatus': False}]]), \
                 patch.object(proof, 'poll_fault_active', side_effect=poll), \
                 patch.object(proof, 'production_status', side_effect=read), \
                 patch.object(proof.time, 'monotonic', side_effect=[0, 3.1] if failure == 'timeout' else None, return_value=0), \
                 patch.object(proof.time, 'monotonic_ns', side_effect=[-300_000_000 if failure == 'stale_status' else 5_000_000, 6_000_000, 7_000_000, 12_000_000]), \
                 patch.object(proof, 'wait_for', side_effect=lambda fn, timeout: fn()), \
                 patch.object(proof, '_hypr', return_value='ok') as dispatch:
                if failure:
                    with self.assertRaises(AssertionError):
                        fixture.inject(Mock(), trace([ACTIVE[0]]), Mock(done=Mock(return_value=False)), Mock())
                    dispatch.assert_not_called()
                else:
                    self.assertEqual(fixture.inject(Mock(), trace([ACTIVE[0]]), Mock(done=Mock(return_value=False)), Mock()), 1)
                    dispatch.assert_called_once()
                    self.assertEqual(calls[:3], ['trace', 'status', 'trace'])


if __name__ == '__main__':
    unittest.main()
