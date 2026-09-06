"""Synthetic orchestration/telemetry tests only; no native desktop is exercised."""
from contextlib import ExitStack
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from production_cancel_proof import (
    MAX_GROUNDING_AGE_NS, PROFILE, active_drags, call_drag, close_owned, grounded_snapshot,
    poll_active, prepare_drag, run,
    terminate_owned, validate_plan, verify_cancellation,
)


BOUNDS = {'x': 0, 'y': 0, 'width': 800, 'height': 600}


def plan():
    return {'purpose': 'cancellation', 'kill_agent': 0,
            'foreground': {'pid': 10, 'window_id': 100}, 'primary_point': [100, 100],
            'agents': [{'app': app, 'name': f'agent-{i}', 'target': {'pid': 20 + i, 'window_id': 200 + i},
                        'bounds': BOUNDS.copy(), 'profile': PROFILE.copy(),
                        'drag': {'from_x': 100, 'from_y': 100, 'to_x': 200, 'to_y': 200, 'duration_ms': 2000}}
                       for i, app in enumerate(('calc', 'inkscape'))]}


def trace(rows, active=True):
    return {'hook': True, 'active': active, 'overflow': False, 'timed_out': False, 'count': len(rows),
            'events': [[i + 1, ms * 1_000_000, kind, 100, 100, lane, value]
                       for i, (ms, kind, lane, value) in enumerate(rows)]}


START = [(0, 'start', 0, 0)]
FIRST = START + [(1, 'agent_admitted', 1, 0), (2, 'agent_drag_start', 1, 0), (2, 'pointer_button', 1, 1)]
OVERLAP = FIRST + [(10, 'agent_admitted', 2, 0), (11, 'agent_drag_start', 2, 0),
                   (11, 'pointer_button', 2, 1), (150, 'pointer_motion', 2, 0)]
FINISH = OVERLAP + [(151, 'agent_cancel', 1, 0), (151, 'pointer_button', 1, 0),
                    (1900, 'pointer_button', 2, 0), (1901, 'agent_drag_end', 2, 0), (2000, 'stop', 0, 0)]
RESPONSE = {'structuredContent': {'effect': 'unverifiable', 'route': 'synthetic_events',
                                   'delivery': {'mode': 'background'}}}


class TelemetryTests(unittest.TestCase):
    def test_complete_overlap_and_own_lane_release(self):
        result = verify_cancellation(trace(FINISH, False), trace(OVERLAP), 1, 2)
        self.assertEqual(result['result'], 'verified')
        self.assertEqual(result['synthetic_cleanup'], 'verified')
        def swap(rows):
            return [(ms, kind, 3 - lane if lane else lane, value) for ms, kind, lane, value in rows]
        self.assertEqual(verify_cancellation(trace(swap(FINISH), False), trace(swap(OVERLAP)), 2, 1)['result'], 'verified')

    def test_missing_truncated_overflowed_and_reordered_telemetry_fail(self):
        for field, value in [('hook', False), ('active', False), ('overflow', True), ('timed_out', True),
                             ('count', 100), ('events', []), ('events', [[1, 0, 'bogus', 0, 0, 0, 0]])]:
            with self.subTest(field=field), self.assertRaises(AssertionError):
                active_drags({**trace(OVERLAP), field: value})
        page = trace(OVERLAP)
        page['events'][3][0] = 100
        with self.assertRaises(AssertionError):
            active_drags(page)

    def test_cancelled_ended_or_unheld_drag_cannot_authorize_kill(self):
        for kind, value in [('agent_cancel', 0), ('agent_drag_end', 0), ('pointer_button', 0),
                            ('agent_admitted', 0), ('agent_action_end', 0), ('keyboard_key', 1)]:
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                active_drags(trace(OVERLAP + [(151, kind, 1, value)]))

    def test_sibling_cancel_missing_release_extra_action_or_primary_disturbance_fails(self):
        mutations = [FINISH[:8] + FINISH[9:],  # missing victim cancellation
                     FINISH[:9] + FINISH[10:],  # missing victim release
                     FINISH[:10] + [(1900, 'agent_cancel', 2, 0)] + FINISH[10:],
                     FINISH[:10] + [(1900, 'agent_drag_start', 1, 0)] + FINISH[10:],
                     FINISH[:10] + [(1900, 'keyboard_key', 1, 1)] + FINISH[10:]]
        for kind in ('keyboard_key', 'pointer_button', 'pointer_focus', 'pointer_axis'):
            mutations.append(FINISH[:-1] + [(1999, kind, 0, 0)] + FINISH[-1:])
        for rows in mutations:
            with self.subTest(rows=rows), self.assertRaises(AssertionError):
                verify_cancellation(trace(rows, False), trace(OVERLAP), 1, 2)
        warped = trace(FINISH, False)
        warped['events'][8][3] = 110
        with self.assertRaises(AssertionError):
            verify_cancellation(warped, trace(OVERLAP), 1, 2)

    def test_poll_requires_fresh_contiguous_overlap_and_pending_calls(self):
        pending = Mock(done=Mock(return_value=False))
        page, active = poll_active(Mock(collect=Mock(return_value=trace(OVERLAP))), trace(FIRST), {1, 2}, [pending])
        self.assertEqual(set(active), {1, 2})
        for done, clocks in [(True, None), (False, [0, 0, 0, 1])]:
            with ExitStack() as stack, self.assertRaises(AssertionError):
                if clocks:
                    stack.enter_context(patch('production_cancel_proof.time.monotonic', side_effect=clocks))
                poll_active(Mock(collect=Mock(return_value=page)), trace(FIRST), {1, 2}, [Mock(done=lambda: done)])
        altered = trace(OVERLAP)
        altered['events'][1][1] += 1
        with self.assertRaisesRegex(AssertionError, 'history'):
            poll_active(Mock(collect=lambda: altered), trace(FIRST), {1, 2}, [pending])
        with patch('production_cancel_proof.time.monotonic', side_effect=[0, 4]), self.assertRaisesRegex(AssertionError, 'bounded'):
            poll_active(Mock(), trace(FIRST), {1, 2}, [pending])

    def test_gate_waits_for_observed_100ms_overlap(self):
        pending = Mock(done=Mock(return_value=False))
        collector = Mock(collect=Mock(side_effect=[trace(OVERLAP[:-1]), trace(OVERLAP)]))
        with patch('production_cancel_proof.time.sleep'):
            page, _ = poll_active(collector, trace(FIRST), {1, 2}, [pending])
        self.assertEqual(collector.collect.call_count, 2)
        self.assertEqual(page, trace(OVERLAP))


def client(pid):
    process = Mock(pid=pid, poll=Mock(return_value=None))
    process.kill.side_effect = lambda: setattr(process.poll, 'return_value', -9)
    return Mock(process=process, failed=False)


class OwnershipTests(unittest.TestCase):
    def test_pointer_plan_requires_derived_drag_and_exact_app_stage(self):
        candidate = plan()
        for spec, stage in zip(candidate['agents'], ('select_range', 'move_rectangle')):
            spec.update(drag={}, pointer_stage=stage)
        validate_plan(candidate)
        for change in ({'pointer_stage': 'click_a1'}, {'drag': plan()['agents'][0]['drag']},
                       {'pointer_stage': 'move_rectangle'}):
            invalid = json.loads(json.dumps(candidate))
            invalid['agents'][0].update(change)
            with self.assertRaises(AssertionError):
                validate_plan(invalid)

    def test_pointer_preparation_uses_exact_image_and_preserves_oracle(self):
        spec = {**plan()['agents'][0], 'drag': {}, 'pointer_stage': 'select_range'}
        before = {'proof_image': 'fresh.png', 'window_bounds': BOUNDS}
        arguments = {**plan()['agents'][0]['drag'], 'steps': 30}
        oracle = {'selection': 'A1:B3'}
        with patch('production_cancel_proof.grounded_snapshot', return_value=before) as snapshot, \
             patch('production_cancel_proof.pointer_grounding.read_pixels', return_value='pixels') as pixels, \
             patch('production_cancel_proof.pointer_grounding.action', return_value=(arguments, oracle)) as action:
            result = prepare_drag(Mock(), spec)
        snapshot.assert_called_once()
        pixels.assert_called_once_with('fresh.png')
        action.assert_called_once_with(before, 'pixels', 'calc', 'select_range')
        self.assertEqual(result['arguments'], arguments)
        self.assertEqual(result['oracle'], oracle)
        self.assertEqual(result['snapshot'], before)

    def test_expired_or_retargeted_grounding_never_dispatches(self):
        spec, mcp = plan()['agents'][0], Mock()
        with patch('production_cancel_proof.grounded_snapshot', return_value={}), \
             patch('production_cancel_proof.time.monotonic_ns', return_value=100):
            prepared = prepare_drag(mcp, spec)
        for change, now in [({}, 101 + MAX_GROUNDING_AGE_NS), ({}, 99),
                            ({'target': {'pid': 99, 'window_id': 99}}, 100),
                            ({'session': 'different'}, 100)]:
            with patch('production_cancel_proof.time.monotonic_ns', return_value=now), self.assertRaises(AssertionError):
                call_drag(mcp, spec, {**prepared, **change})
        mcp.tool.assert_not_called()

    def test_prepared_drag_does_not_snapshot_during_sibling_gesture(self):
        spec, mcp = plan()['agents'][0], Mock()
        mcp.tool.return_value = RESPONSE
        with patch('production_cancel_proof.grounded_snapshot') as snapshot, \
             patch('production_cancel_proof.time.monotonic_ns', return_value=100):
            prepared = prepare_drag(mcp, spec)
            result = call_drag(mcp, spec, prepared)
        snapshot.assert_called_once()
        self.assertEqual(result['outcome'], 'response')
        mcp.tool.assert_called_once()

    def test_pointer_snapshot_requires_own_exact_png(self):
        spec = {**plan()['agents'][0], 'drag': {}, 'pointer_stage': 'select_range'}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'image.png').touch()
            content = {'window_bounds': BOUNDS, 'screenshot_width': 800, 'screenshot_height': 600}
            for image_file, valid in [('image.png', True), ('missing.png', False), ('../escape.png', False)]:
                mcp = Mock(directory=root, tool=Mock(side_effect=[
                    {'structuredContent': {'windows': [spec['target']]}},
                    {'structuredContent': content, 'content': [{'type': 'image', 'image_file': image_file}]}]))
                if valid:
                    result = grounded_snapshot(mcp, spec['target'], spec)
                    self.assertEqual(result['proof_image'], str(root / 'image.png'))
                    self.assertNotIn('max_elements', mcp.tool.call_args.args[1])
                else:
                    with self.assertRaises((AssertionError, FileNotFoundError)):
                        grounded_snapshot(mcp, spec['target'], spec)

    def test_cleanup_reaps_child_even_when_close_raises(self):
        owned = client(100)
        owned.close.side_effect = RuntimeError('close failed')
        with self.assertRaisesRegex(RuntimeError, 'close failed'):
            close_owned(owned)
        owned.process.kill.assert_called_once_with()
        owned.process.wait.assert_called_once_with(timeout=3)

    def test_kill_one_exact_owned_process_and_poison_without_replay(self):
        victim, sibling = client(100), client(101)
        terminate_owned(victim, sibling, [Mock(done=lambda: False)])
        self.assertTrue(victim.failed)
        victim.process.kill.assert_called_once_with()
        victim.process.wait.assert_called_once_with(timeout=3)
        sibling.process.kill.assert_not_called()
        victim.tool.assert_not_called()

    def test_shared_exited_or_completed_process_cannot_be_killed(self):
        for same, exited, done in [(True, False, False), (False, True, False), (False, False, True)]:
            victim, sibling = client(100), client(100 if same else 101)
            if exited:
                victim.process.poll.return_value = 0
            with self.assertRaises(AssertionError):
                terminate_owned(victim, sibling, [Mock(done=lambda: done)])
            victim.process.kill.assert_not_called()

    def test_unknown_drag_is_called_once_with_fresh_grounding(self):
        mcp, spec = Mock(), plan()['agents'][0]
        mcp.tool.side_effect = TimeoutError('lost reply')
        with patch('production_cancel_proof.grounded_snapshot') as snapshot:
            result = call_drag(mcp, spec)
        snapshot.assert_called_once_with(mcp, spec['target'], spec)
        self.assertEqual(result['outcome'], 'unknown')
        self.assertFalse(result['replayed'])
        mcp.tool.assert_called_once_with('drag', {**spec['drag'], **spec['target'],
                                                'session': spec['name'], 'delivery_mode': 'background'})

    def test_plan_rejects_unsafe_targets_profiles_and_unbounded_drags(self):
        validate_plan(plan())
        for change in ({'profile': {'mode': 'standard'}}, {'drag': {**plan()['agents'][0]['drag'], 'pid': 5}},
                       {'drag': {**plan()['agents'][0]['drag'], 'duration_ms': 2001}},
                       {'drag': {**plan()['agents'][0]['drag'], 'from_x': float('nan')}},
                       {'target': {'pid': 10, 'window_id': 200}}):
            candidate = plan()
            candidate['agents'][0].update(change)
            with self.assertRaises(AssertionError):
                validate_plan(candidate)

    def test_snapshot_rejects_stale_identity_geometry_and_off_image_coordinates(self):
        spec = plan()['agents'][0]
        good = {'structuredContent': {'window_bounds': BOUNDS, 'screenshot_width': 800, 'screenshot_height': 600}}
        for windows, content in [([], good), ([{'pid': 20, 'window_id': 999}], good),
                                 ([spec['target']], {'structuredContent': {**good['structuredContent'], 'window_bounds': {}}}),
                                 ([spec['target']], {'structuredContent': {**good['structuredContent'], 'screenshot_width': 50}})]:
            mcp = Mock(tool=Mock(side_effect=[{'structuredContent': {'windows': windows}}, content]))
            with self.assertRaises(AssertionError):
                grounded_snapshot(mcp, spec['target'], spec)


class RunnerTests(unittest.TestCase):
    def test_success_and_failures_reap_all_owned_children_without_replay(self):
        for failure in (None, 'pointer', 'pointer_effect', 'trace', 'unknown_sibling',
                        'successful_victim', 'close', 'snapshot', 'grab'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                root = Path(directory)
                source_plan = root / 'plan.json'
                candidate = plan()
                if failure in ('pointer', 'pointer_effect'):
                    for spec, stage in zip(candidate['agents'], ('select_range', 'move_rectangle')):
                        spec.update(drag={}, pointer_stage=stage)
                source_plan.write_text(json.dumps(candidate))
                args = SimpleNamespace(plan=source_plan, evidence=root / 'evidence', driver=root / 'driver',
                                       trace_socket=root / 'cua-input-v3.sock', primary_grab=root / 'primary-grab',
                                       foreground_journal=root / 'journal')
                agents, observer = [client(100), client(101)], client(102)
                for owned in [*agents, observer]:
                    owned.close.side_effect = lambda c=owned: setattr(c.process.poll, 'return_value', 0)
                observer.tool.return_value = {'structuredContent': {'screen_width': 1000, 'screen_height': 800}}
                for agent in agents:
                    agent.tool.return_value = {'structuredContent': {}}
                held = [True]
                grab = Mock(stdout=Mock(), poll=Mock(return_value=0))
                trace_client = Mock(hello={'protocol': 3})
                stopped = trace(FINISH, False)
                if failure == 'trace':
                    stopped['overflow'] = True
                trace_client.collect.side_effect = [trace(START), trace(FIRST), trace(OVERLAP), stopped]
                outcomes = [{'outcome': 'unknown', 'replayed': False},
                            {'outcome': 'response', 'response': RESPONSE, 'replayed': False}]
                if failure == 'unknown_sibling':
                    outcomes[1] = outcomes[0]
                if failure == 'successful_victim':
                    outcomes[0] = outcomes[1]
                futures = [Mock(done=Mock(return_value=False), result=Mock(return_value=o)) for o in outcomes]
                for future in futures:
                    def finish(*args, f=future, **kwargs):
                        f.done.return_value = True
                        return f.result.return_value
                    future.result.side_effect = finish
                pool = Mock(submit=Mock(side_effect=futures))
                snapshot = Mock(return_value={'window_bounds': BOUNDS, 'proof_image': 'fresh.png'})
                if failure == 'snapshot':
                    snapshot.side_effect = AssertionError('stale geometry')
                if failure == 'close':
                    agents[0].close.side_effect = RuntimeError('cleanup failure')
                replacements = {
                    'provenance': Mock(return_value={'files': {}}), 'DirectMCP': Mock(side_effect=[*agents, observer]),
                    'grounded_snapshot': snapshot, 'subprocess.Popen': Mock(return_value=grab),
                    'primary_acknowledgement': Mock(side_effect=AssertionError('missing HELD')) if failure == 'grab' else Mock(return_value='HELD\n'),
                    'wait_for': lambda fn: self.assertTrue(fn()),
                    'state': lambda path: {'held': held[0], 'clicks': 0, 'keys': 0, 'scroll': 0},
                    'wm': lambda: {'pid': 10, 'cursor': {'x': 100, 'y': 100}}, 'Trace': Mock(return_value=trace_client),
                    'ThreadPoolExecutor': Mock(return_value=pool), 'stop_process': lambda proc: held.__setitem__(0, False),
                    'pointer_grounding.read_pixels': Mock(return_value='pixels'),
                    'pointer_grounding.action': Mock(return_value=(plan()['agents'][0]['drag'], {'stage': 'drag'})),
                    'pointer_grounding.verify': Mock(side_effect=AssertionError('sibling did not move'))
                        if failure == 'pointer_effect' else Mock(return_value={'verified': True}),
                }
                for name, value in replacements.items():
                    stack.enter_context(patch('production_cancel_proof.' + name, value))
                self.assertEqual(run(args), 0 if failure in (None, 'pointer') else 1)
                result = json.loads((args.evidence / 'result.json').read_text())
                self.assertEqual(result['reacquisition'], 'unproven')
                if failure in ('pointer', 'pointer_effect'):
                    replacements['pointer_grounding.verify'].assert_called_once()
                    self.assertEqual(replacements['pointer_grounding.action'].call_count, 2)
                    self.assertTrue((args.evidence / 'agent-0-drag-grounding.json').is_file())
                    self.assertTrue((args.evidence / 'agent-1-drag-grounding.json').is_file())
                if failure == 'pointer':
                    self.assertEqual(result['sibling_app_effect'], {'verified': True})
                agents[0].close.assert_called_once()
                if failure != 'snapshot':
                    agents[1].close.assert_called_once()
                    observer.close.assert_called_once()
                    self.assertFalse(held[0])
                if failure not in ('snapshot', 'grab'):
                    self.assertEqual(pool.submit.call_count, 2)
                    agents[0].process.kill.assert_called_once()
                    agents[1].process.kill.assert_not_called()
                    trace_client.close.assert_called_once()
                    self.assertEqual(trace_client.exchange.call_args_list[0].args, ('TRACE_START',))
                    self.assertEqual(trace_client.exchange.call_args_list[-1].args, ('TRACE_STOP',))

    def test_missing_trace_stops_before_native_process_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source_plan = root / 'plan.json'
            source_plan.write_text(json.dumps(plan()))
            with patch('production_cancel_proof.provenance') as origin, patch('production_cancel_proof.DirectMCP') as launch:
                self.assertEqual(run(SimpleNamespace(plan=source_plan, evidence=root / 'evidence', trace_socket=None)), 1)
                origin.assert_not_called()
                launch.assert_not_called()


if __name__ == '__main__':
    unittest.main()
