"""Portable synthetic tests; these do not exercise or certify a native desktop."""
from contextlib import ExitStack
from copy import deepcopy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_geometry_fault_proof as proof


BOUNDS = {'x': 10, 'y': 20, 'width': 800, 'height': 600}
PARTIAL = {'structuredContent': {'effect': 'partial', 'route': 'synthetic_events',
                               'delivery': {'mode': 'background', 'delivered_count': 1}}}
DELIVERED = {'structuredContent': {'effect': 'unverifiable', 'route': 'synthetic_events',
                                 'delivery': {'mode': 'background'}}}


def plan(kind='move', app='calc'):
    return {'purpose': 'geometry_fault', 'disposable': True, 'compositor': {'pid': 50, 'instance': 'test_1'},
            'foreground': {'pid': 10, 'window_id': 100}, 'primary_point': [20, 20],
            'agents': [{'app': app, 'name': 'geometry', 'target': {'pid': 20, 'window_id': 200},
                        'bounds': dict(BOUNDS), 'pointer_stage': proof.POINTER_STAGES[app], 'drag': {}}],
            'fault': {'kind': kind, 'to': [30, 40] if kind == 'move' else [820, 620]},
            'recovery': {'pointer_stage': 'click_b2' if app == 'calc' else 'scroll_down'}}


def trace(rows, active=True):
    return {'hook': True, 'active': active, 'overflow': False, 'timed_out': False, 'count': len(rows),
            'events': [[i + 1, ms * 1_000_000, kind, 100, 100, lane, value]
                       for i, (ms, kind, lane, value) in enumerate(rows)]}


ACTIVE = [(0, 'start', 0, 0), (1, 'agent_admitted', 1, 0), (2, 'agent_drag_start', 1, 0),
          (3, 'pointer_button', 1, 1), (4, 'pointer_motion', 1, 0)]
CANCEL = ACTIVE + [(8, 'agent_cancel', 1, 0), (9, 'pointer_button', 1, 0), (10, 'pointer_leave', 1, 0)]


def record():
    return {'result': 'observed', 'lane': 1, 'prefix': trace(ACTIVE), 'gate_ns': 5_000_000,
            'requested_ns': 6_000_000, 'acknowledged_ns': 7_000_000,
            'after': {'bounds': {**BOUNDS, 'x': 30, 'y': 40}, 'observed_ns': 11_000_000},
            'before_bounds': dict(BOUNDS), 'expected_bounds': {**BOUNDS, 'x': 30, 'y': 40}}


def action(response=PARTIAL):
    return {'outcome': 'response', 'response': deepcopy(response), 'replayed': False}


def client(pid, alive=True):
    process = Mock(pid=pid, poll=Mock(return_value=None if alive else 0))
    process.kill.side_effect = lambda: setattr(process.poll, 'return_value', -9)
    return Mock(process=process)


class OracleTests(unittest.TestCase):
    def test_partial_and_unknown_preserve_delivery_uncertainty(self):
        for observed in (action(), {'outcome': 'unknown', 'error': 'lost reply', 'replayed': False},
                         action({'structuredContent': {'effect': 'unverifiable', 'route': 'synthetic_events',
                                                       'delivery': {'mode': 'unknown'}}})):
            self.assertEqual(proof.verify_fault(trace(CANCEL), record(), observed)['result'], 'verified')
        for response in (DELIVERED, {'isError': True, 'structuredContent': {
                'effect': 'refused', 'reason': 'cancelled'}}, {**PARTIAL, 'structuredContent': {
                **PARTIAL['structuredContent'], 'delivery': {'mode': 'background', 'delivered_count': 0}}}):
            with self.subTest(response=response), self.assertRaises(AssertionError):
                proof.verify_fault(trace(CANCEL), record(), action(response))
        with self.assertRaises(AssertionError):
            proof.fault_outcome({**action(), 'replayed': True})

    def test_incomplete_and_discontinuous_telemetry_cannot_pass(self):
        for key, value in [('hook', False), ('active', False), ('overflow', True), ('timed_out', True), ('count', 0)]:
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_fault({**trace(CANCEL), key: value}, record(), action())
        changed = trace(CANCEL)
        changed['events'][1][1] += 1
        with self.assertRaisesRegex(AssertionError, 'history'):
            proof.verify_fault(changed, record(), action())

    def test_primary_warp_and_input_leak_fail_even_with_equal_endpoints(self):
        for kind, lane, value in [('cursor', 0, 0), ('pointer_focus', 0, 0), ('pointer_button', 0, 0),
                                  ('keyboard_key', 0, 1), ('pointer_axis', 0, 0)]:
            rows = trace(CANCEL + [(12, kind, lane, value), (13, 'pointer_leave', 1, 0)])
            if kind == 'cursor':
                rows['events'][-2][3] += 1
                rows['events'][-1][2] = 'cursor'  # Warp back: endpoint-only checks would miss it.
                rows['events'][-1][5] = 0
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                proof.verify_fault(rows, record(), action())

    def test_own_lane_cancellation_release_and_no_continuation_required(self):
        mutations = [ACTIVE, CANCEL[:5] + CANCEL[6:], CANCEL[:6] + CANCEL[7:],
                     ACTIVE + [(8, 'agent_cancel', 2, 0), (9, 'pointer_button', 1, 0)],
                     ACTIVE + [(5, 'agent_cancel', 1, 0), (9, 'pointer_button', 1, 0)],
                     ACTIVE + [(8, 'pointer_button', 1, 0), (9, 'agent_cancel', 1, 0)]]
        for kind in ('agent_drag_end', 'agent_drag_start', 'agent_admitted', 'agent_action_end',
                     'pointer_motion', 'pointer_enter', 'keyboard_key', 'pointer_axis'):
            mutations.append(CANCEL + [(12, kind, 1, 0)])
        for rows in mutations:
            with self.subTest(rows=rows), self.assertRaises(AssertionError):
                proof.verify_fault(trace(rows), record(), action())

    def test_unchanged_geometry_stale_gate_and_invalid_timestamps_fail(self):
        for field, value in [('result', 'unproven'), ('requested_ns', 300_000_000),
                             ('acknowledged_ns', 4_000_000), ('gate_ns', 7_000_000),
                             ('after', {'bounds': BOUNDS, 'observed_ns': 11_000_000}),
                             ('before_bounds', record()['expected_bounds'])]:
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.verify_fault(trace(CANCEL), {**record(), field: value}, action())


class OwnershipTests(unittest.TestCase):
    def test_resize_keeps_center_not_top_left_and_inverse_restores_exact_frame(self):
        before = {'x': 983, 'y': 576, 'width': 480, 'height': 480}
        smaller = {'x': 987, 'y': 580, 'width': 472, 'height': 472}
        fault = {'kind': 'resize', 'to': [472, 472]}
        self.assertEqual(proof.expected_geometry(before, fault), smaller)
        self.assertEqual(proof.expected_geometry(smaller, {'kind': 'resize', 'to': [480, 480]}), before)
        self.assertEqual(proof.expected_geometry(BOUNDS, plan('resize')['fault']),
                         {'x': 0, 'y': 10, 'width': 820, 'height': 620})
        self.assertEqual(proof.expected_geometry(BOUNDS, plan()['fault']),
                         {**BOUNDS, 'x': 30, 'y': 40})

    def test_resize_with_fractional_center_shift_is_rejected_before_dispatch(self):
        for size in ([819, 620], [820, 619]):
            candidate = plan('resize')
            candidate['fault']['to'] = size
            with self.subTest(size=size), self.assertRaisesRegex(AssertionError, 'even'):
                proof.validate_plan(candidate)

    def test_plan_confines_targets_geometry_and_new_action(self):
        for kind in ('move', 'resize'):
            for app in ('calc', 'inkscape'):
                candidate = plan(kind, app)
                proof.validate_plan(candidate)
                bad = deepcopy(candidate)
                bad['agents'][0]['drag'] = {'pid': 999}
                with self.assertRaises(AssertionError):
                    proof.validate_plan(bad)
        for update in ({'disposable': False}, {'agents': []}, {'fault': {'kind': 'close', 'to': [1, 2]}},
                       {'fault': {'kind': 'move', 'to': [10, 20]}},
                       {'fault': {'kind': 'move', 'to': [9999, 20]}},
                       {'recovery': {'pointer_stage': 'select_range'}},
                       {'compositor': {'pid': 50, 'instance': 'x;bad'}}):
            with self.subTest(update=update), self.assertRaises(AssertionError):
                proof.validate_plan({**plan(), **update})

    def controller(self, kind='move'):
        fault = object.__new__(proof.GeometryFault)
        candidate = plan(kind)
        fault.spec, fault.fault = candidate['agents'][0], candidate['fault']
        fault.instance = 'test_1'
        fault.owner = {'pid': 20, 'uid': 1000, 'starttime': '1', 'exe': '/usr/bin/app'}
        fault.compositor = {'pid': 50, 'uid': 1000, 'starttime': '2', 'exe': '/usr/bin/Hyprland'}
        fault.expected = proof.expected_geometry(BOUNDS, candidate['fault'])
        fault.mutated, fault.record = False, {'result': 'unproven'}
        return fault

    def test_snapshot_revalidates_exact_pid_address_native_floating_and_synthetic_title(self):
        window = {'pid': 20, 'address': '0xc8', 'floating': True, 'xwayland': False,
                  'title': 'cua-smoke-calc.ods', 'at': [10, 20], 'size': [800, 600]}
        for change in (None, {'pid': 21}, {'address': '0xc9'}, {'floating': False},
                       {'xwayland': True}, {'title': 'unowned.ods'}):
            fault = self.controller()
            fault.hypr = Mock(side_effect=[json.dumps([{'pid': 50, 'instance': 'test_1'}]),
                                           json.dumps([{**window, **(change or {})}])])
            with patch.object(proof, 'process_identity', side_effect=[fault.compositor, fault.owner]):
                if change:
                    with self.assertRaises(AssertionError):
                        fault.snapshot()
                else:
                    self.assertEqual(fault.snapshot()['bounds'], BOUNDS)
        fault = self.controller()
        with patch.object(proof, 'process_identity', return_value={}), self.assertRaises(AssertionError):
            fault.snapshot()

    def test_exact_dispatch_has_no_focus_or_input_operation(self):
        for kind in ('move', 'resize'):
            fault = self.controller(kind)
            fault.hypr = Mock(return_value='ok')
            fault.dispatch(fault.fault['to'])
            x, y = fault.fault['to']
            fault.hypr.assert_called_once_with('dispatch',
                f'hl.dsp.window.{kind}({{ window = "address:0xc8", x = {x}, y = {y}, relative = false }})')

    def test_injection_requires_active_pending_and_fresh_owned_geometry(self):
        for failure in (None, 'done', 'stale', 'changed', 'reply_lost', 'unchanged'):
            fault = self.controller()
            before = {'bounds': dict(BOUNDS), 'observed_ns': 1}
            after = {'bounds': fault.expected, 'observed_ns': 10}
            fault.snapshot = Mock(side_effect=[before, {**before, 'bounds': fault.expected} if failure == 'changed' else before, after])
            fault.dispatch = Mock(side_effect=TimeoutError('lost IPC reply') if failure == 'reply_lost' else None)
            pending = Mock(done=Mock(return_value=failure == 'done'))
            with ExitStack() as stack:
                stack.enter_context(patch.object(proof, 'poll_active', return_value=(trace(ACTIVE), {1: 2})))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[5, 300_000_006 if failure == 'stale' else 6, 7]))
                stack.enter_context(patch.object(proof, 'wait_for', side_effect=AssertionError('unchanged') if failure == 'unchanged' else lambda check, timeout: check()))
                if failure:
                    with self.assertRaises((AssertionError, TimeoutError)):
                        fault.inject(Mock(), trace(ACTIVE[:1]), pending, Mock())
                else:
                    _, lane = fault.inject(Mock(), trace(ACTIVE[:1]), pending, Mock())
                    self.assertEqual(lane, 1)
                    self.assertEqual(fault.record['result'], 'observed')
                self.assertEqual(fault.dispatch.call_count, int(failure not in ('done', 'stale', 'changed')))
                if failure == 'reply_lost':
                    self.assertTrue(fault.mutated, 'unknown dispatch must retain cleanup obligation')

    def test_restore_only_owned_expected_geometry_and_preserve_identity_failure(self):
        for kind in ('move', 'resize'):
            fault = self.controller(kind)
            fault.snapshot, fault.dispatch = Mock(), Mock()
            self.assertEqual(fault.restore(), {'result': 'not_needed'})
            fault.snapshot.assert_not_called()
            for current in (fault.expected, BOUNDS, {**BOUNDS, 'x': 99}):
                fault.mutated = True
                fault.snapshot = Mock(side_effect=[{'bounds': current}, {'bounds': BOUNDS}])
                fault.dispatch = Mock()
                with patch.object(proof, 'wait_for', side_effect=lambda check, timeout: check()):
                    if current not in (fault.expected, BOUNDS):
                        with self.assertRaisesRegex(AssertionError, 'unowned geometry'):
                            fault.restore()
                        fault.dispatch.assert_not_called()
                    else:
                        self.assertEqual(fault.restore()['result'], 'restored')
                        self.assertEqual(fault.dispatch.call_count, int(current != BOUNDS))


class RecoveryTests(unittest.TestCase):
    def test_fresh_runtime_fresh_grounding_single_new_action_and_unknown_never_replayed(self):
        for app in ('calc', 'inkscape'):
            for failure in (None, 'slow_discovery', 'alive', 'reused', 'stale', 'unknown', 'guard', 'effect'):
                with self.subTest(app=app, failure=failure), ExitStack() as stack:
                    spec = plan(app=app)['agents'][0]
                    stage = plan(app=app)['recovery']['pointer_stage']
                    tool = 'click' if app == 'calc' else 'scroll'
                    fresh, observer, victim = client(101), client(102), client(100, failure == 'alive')
                    if failure == 'reused':
                        fresh.process.pid = 100
                    fresh.tool.side_effect = [{}, TimeoutError('lost reply') if failure == 'unknown' else DELIVERED]
                    before, after = {'proof_image': 'before.png'}, {'proof_image': 'after.png'}
                    dispatch_ns = 101
                    if failure == 'slow_discovery':
                        before['proof_observation_started_ns'] = proof.MAX_GROUNDING_AGE_NS + 200
                        dispatch_ns = proof.MAX_GROUNDING_AGE_NS + 301
                    stack.enter_context(patch.object(proof, 'grounded_snapshot', side_effect=[before, after]))
                    stack.enter_context(patch.object(proof, 'app_process_identity'))
                    stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[100, proof.MAX_GROUNDING_AGE_NS + 101 if failure == 'stale' else dispatch_ns]))
                    stack.enter_context(patch.object(proof.pointer_grounding, 'read_pixels', return_value='pixels'))
                    stack.enter_context(patch.object(proof.pointer_grounding, 'action', return_value=({'x': 20, 'y': 30}, {'stage': stage})))
                    stack.enter_context(patch.object(proof.pointer_grounding, 'verify', side_effect=AssertionError('effect') if failure == 'effect' else None))
                    stack.enter_context(patch.object(proof, 'verify_recovery_trace', return_value={'result': 'verified'}))
                    result, save = {}, Mock()
                    guard = Mock(side_effect=AssertionError('primary expired') if failure == 'guard' else None)
                    if failure not in (None, 'slow_discovery'):
                        with self.assertRaises(AssertionError):
                            proof.recover(fresh, observer, victim, spec, stage, Mock(), trace(CANCEL), 1, guard, save, result)
                    else:
                        proof.recover(fresh, observer, victim, spec, stage, Mock(), trace(CANCEL), 1, guard, save, result)
                        self.assertEqual(result['result'], 'verified')
                        self.assertEqual(result['action']['dispatch_ns'], dispatch_ns)
                        grounding = next(call.args[1] for call in save.call_args_list
                                         if call.args[0] == 'recovery-grounding.json')
                        self.assertEqual(grounding['prepared_ns'],
                                         before.get('proof_observation_started_ns', 100))
                    inputs = [call for call in fresh.tool.call_args_list if call.args[0] != 'start_session']
                    self.assertEqual(len(inputs), int(failure not in ('alive', 'reused', 'stale', 'guard')))
                    if inputs:
                        self.assertEqual(inputs[0].args[0], tool)
                    if failure == 'unknown':
                        self.assertEqual(result['action']['outcome'], 'unknown')
                        self.assertFalse(result['action']['replayed'])
                    victim.tool.assert_not_called()

    def test_invalid_plan_failure_preserves_cleanup_and_fails(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            plan_path = directory / 'plan.json'
            plan_path.write_text(json.dumps({**plan(), 'disposable': False}))
            args = SimpleNamespace(evidence=directory / 'evidence', plan=plan_path)
            with patch.object(proof, 'GeometryFault') as constructor:
                self.assertEqual(proof.run(args), 1)
            constructor.assert_not_called()
            result = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(result['result'], 'failed')
            self.assertEqual(json.loads((args.evidence / 'cleanup.json').read_text()), {'errors': []})

    def test_orchestration_always_preserves_trace_reaps_owned_runtimes_and_restores(self):
        for failure in (None, 'inject', 'recovery', 'restore', 'expired'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root, ExitStack() as stack:
                directory = Path(root)
                plan_path = directory / 'plan.json'
                plan_path.write_text(json.dumps(plan()))
                args = SimpleNamespace(evidence=directory / 'evidence', plan=plan_path, driver=Path('/driver'),
                                       primary_grab=Path('/grab'), foreground_journal=Path('/journal'),
                                       trace_socket=Path('/cua-input-v3.sock'))
                fault = Mock(record=record(), expected=record()['expected_bounds'])
                fault.inject.side_effect = AssertionError('injection failed') if failure == 'inject' else None
                fault.inject.return_value = (trace(ACTIVE), 1)
                fault.restore.side_effect = AssertionError('restoration refused') if failure == 'restore' else None
                fault.restore.return_value = {'result': 'restored'}
                stack.enter_context(patch.object(proof, 'GeometryFault', return_value=fault))
                stack.enter_context(patch.object(proof, 'provenance', return_value={'files': {}}))
                agent, observer, fresh = client(101), client(102), client(103)
                observer.tool.return_value = {'structuredContent': {'screen_width': 1600, 'screen_height': 900}}
                agent.tool.return_value = {}
                stack.enter_context(patch.object(proof, 'DirectMCP', side_effect=[agent, observer, fresh]))
                stack.enter_context(patch.object(proof, 'grounded_snapshot', return_value={'window_bounds': BOUNDS}))
                stack.enter_context(patch.object(proof, 'prepare_drag', return_value={'grounding': 'fresh'}))
                stack.enter_context(patch.object(proof, 'call_drag', return_value=action()))
                grab = Mock(poll=Mock(return_value=None))
                grab.terminate.side_effect = lambda: setattr(grab.poll, 'return_value', 0)
                stack.enter_context(patch.object(proof.subprocess, 'Popen', return_value=grab))
                stack.enter_context(patch.object(proof, 'stop_process'))
                stack.enter_context(patch.object(proof, 'primary_acknowledgement', return_value='HELD\n'))
                stack.enter_context(patch.object(proof, 'wait_for', return_value=True))
                stack.enter_context(patch.object(proof, 'wm', return_value={'pid': 10}))
                stack.enter_context(patch.object(proof, 'state', return_value={'held': True, 'clicks': 0, 'keys': 0, 'scroll': 0}))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', return_value=0))
                stopped = False
                def exchange(command):
                    nonlocal stopped
                    if command == 'TRACE_STOP':
                        stopped = True
                def guard(*_args):
                    if stopped and failure == 'expired':
                        raise AssertionError('primary deadline expired')
                stack.enter_context(patch.object(proof, 'require_primary_active', side_effect=guard))
                recovery = trace(CANCEL + [(11, 'agent_admitted', 1, 0), (12, 'pointer_button', 1, 1),
                                           (13, 'pointer_button', 1, 0), (14, 'agent_action_end', 1, 0)])
                final = proof.stopped_prefix(recovery if failure not in ('inject', 'recovery') else trace(CANCEL))
                pages = [trace(ACTIVE[:1])]
                if failure != 'inject':
                    pages += [trace(CANCEL), trace(CANCEL)]
                pages += [final]
                tracer = Mock(hello={'protocol': 3}, exchange=Mock(side_effect=exchange), collect=Mock(side_effect=pages))
                stack.enter_context(patch.object(proof, 'Trace', return_value=tracer))
                def recover(*args):
                    if failure == 'recovery':
                        raise AssertionError('recovery failed')
                    args[-1]['result'] = 'verified'
                    return recovery
                stack.enter_context(patch.object(proof, 'recover', side_effect=recover))
                def close(client):
                    client.process.poll.return_value = 0
                closer = stack.enter_context(patch.object(proof, 'close_owned', side_effect=close))
                stack.enter_context(patch('builtins.print'))
                self.assertEqual(proof.run(args), int(failure is not None))
                report = json.loads((args.evidence / 'result.json').read_text())
                self.assertEqual(report['result'], 'failed' if failure else 'passed')
                self.assertFalse(report['physical_hardware'])
                self.assertTrue((args.evidence / 'fault.json').exists())
                self.assertTrue((args.evidence / 'trace.json').exists())
                fault.restore.assert_called_once()
                tracer.close.assert_called_once()
                self.assertEqual(agent.process.poll(), 0)
                self.assertEqual(observer.process.poll(), 0)
                if failure != 'inject':
                    self.assertEqual(fresh.process.poll(), 0)
                self.assertGreaterEqual(closer.call_count, 2)
                if failure in ('restore', 'expired'):
                    self.assertTrue(json.loads((args.evidence / 'cleanup.json').read_text())['errors'])


if __name__ == '__main__':
    unittest.main()
