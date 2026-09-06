"""Synthetic orchestration/telemetry tests only; no native desktop is exercised."""
from contextlib import ExitStack
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import subprocess
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from production_cancel_proof import (
    GROUNDING_DISPATCH_RESERVE_NS, MAX_GROUNDING_AGE_NS, PROFILE,
    active_drags, call_drag, close_owned, grounded_snapshot,
    pair_has_dispatch_budget, poll_active, prepare_drag, prepare_drags, recover_once, run, stopped_prefix,
    terminate_owned, validate_plan, verify_cancellation, verify_fresh_observation,
    verify_recovery_cleanup, verify_recovery_trace,
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


def wire_trace(rows, active=True):
    """Both lanes start at the same point; only lane two reaches the end."""
    result = trace(rows, active)
    events = []
    for row in result['events']:
        if row[2] == 'agent_drag_start':
            events.append([0, row[1], 'pointer_motion', *row[3:6], 0, 100, 100])
        if row[2] == 'pointer_button' and row[1] == 1900 * 1_000_000:
            events.append([0, row[1], 'pointer_motion', *row[3:6], 0, 200, 200])
        events.append(row + ([110, 110] if row[2] == 'pointer_motion' else []))
    for sequence, row in enumerate(events, 1):
        row[0] = sequence
    return {**result, 'events': events, 'count': len(events)}


def recovery_rows(tool='click', lane=1):
    inputs = [(2010, 'pointer_button', lane, 1), (2011, 'pointer_button', lane, 0)] if tool == 'click' else [
        (2010, 'pointer_axis', lane, 0)]
    return FINISH[:-1] + [(2001, 'agent_admitted', lane, 0), *inputs, (2012, 'agent_action_end', lane, 0)]


class TelemetryTests(unittest.TestCase):
    def test_active_drags_accepts_surface_coordinates_without_losing_lane_state(self):
        self.assertEqual(active_drags(wire_trace(OVERLAP)), active_drags(trace(OVERLAP)))
        self.assertEqual(verify_cancellation(wire_trace(FINISH, False), wire_trace(OVERLAP), 1, 2)
                         ['result'], 'verified')

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
    process.terminate.side_effect = lambda: setattr(process.poll, 'return_value', -15)
    return Mock(process=process, failed=False)


class RecoveryTests(unittest.TestCase):
    def test_completed_recovery_allows_only_seat_cleanup(self):
        rows = recovery_rows()
        cleanup = [(2020 + i, kind, lane, 0) for i, (kind, lane) in enumerate(
            [('agent_cancel', 1), ('pointer_leave', 1), ('keyboard_leave', 1),
             ('agent_cancel', 2), ('pointer_leave', 2), ('keyboard_leave', 2)])]
        prefix = trace(rows)
        for tail in ([], cleanup):
            stopped = trace(rows + tail + [(2030, 'stop', 0, 0)], False)
            self.assertEqual(verify_recovery_cleanup(prefix, stopped)['result'], 'passed')
        for kind in ('agent_admitted', 'agent_action_end', 'agent_drag_start', 'agent_drag_end',
                     'pointer_motion', 'pointer_button', 'pointer_axis', 'keyboard_key'):
            for lane in (1, 2):
                with self.subTest(kind=kind, lane=lane), self.assertRaises(AssertionError):
                    verify_recovery_cleanup(prefix,
                        trace(rows + cleanup + [(2028, kind, lane, 0), (2030, 'stop', 0, 0)], False))
        for field, value in (('active', True), ('hook', False), ('overflow', True),
                             ('timed_out', True), ('count', 0)):
            with self.subTest(field=field), self.assertRaises(AssertionError):
                verify_recovery_cleanup(prefix, {**trace(rows + [(2030, 'stop', 0, 0)], False), field: value})
        changed = trace(rows + cleanup + [(2030, 'stop', 0, 0)], False)
        changed['events'][1][1] += 1
        with self.assertRaisesRegex(AssertionError, 'history'):
            verify_recovery_cleanup(prefix, changed)

    def test_recovery_plan_requires_new_supported_action_and_both_oracles(self):
        for victim, stage in ((0, 'click_b2'), (1, 'scroll_down')):
            candidate = plan()
            candidate.update(kill_agent=victim, recovery={'pointer_stage': stage})
            with self.assertRaisesRegex(AssertionError, 'both app-effect'):
                validate_plan(candidate)
            for spec, drag_stage in zip(candidate['agents'], ('select_range', 'move_rectangle')):
                spec.update(drag={}, pointer_stage=drag_stage)
            validate_plan(candidate)
            for bad in ({'pointer_stage': 'move_rectangle'}, {'pointer_stage': 'select_range'},
                        {'pointer_stage': stage, 'arguments': {'x': 10}}, None):
                with self.subTest(victim=victim, bad=bad), self.assertRaises(AssertionError):
                    validate_plan({**candidate, 'recovery': bad})

    def test_phase_traces_require_one_fresh_action_on_released_lane(self):
        for lane in (1, 2):
            for tool in ('click', 'scroll'):
                rows = recovery_rows(tool, lane)
                if lane == 2:
                    rows[:len(FINISH) - 1] = [
                        (ms, kind, 3 - old if old else old, value) for ms, kind, old, value in FINISH[:-1]]
                boundary, after = trace(rows[:len(FINISH) - 1]), trace(rows)
                self.assertEqual(verify_recovery_trace(boundary, after, lane, tool)['result'], 'verified')
                self.assertEqual(verify_cancellation(stopped_prefix(boundary),
                    trace(rows[:len(OVERLAP)]), lane, 3 - lane)['result'], 'verified')
                for extra in ((2013, 'agent_admitted', lane, 0), (2013, 'agent_cancel', lane, 0),
                              (2013, 'agent_action_end', lane, 0), (2013, 'agent_drag_start', lane, 0),
                              (2013, 'keyboard_key', lane, 1), (2013, 'pointer_button', lane, 1),
                              (2013, 'pointer_motion', 3 - lane, 0), (2013, 'pointer_axis', 0, 0)):
                    with self.subTest(extra=extra), self.assertRaises(AssertionError):
                        verify_recovery_trace(boundary, trace(rows + [extra]), lane, tool)
                for missing in (-1, len(FINISH) - 1, len(FINISH)):
                    damaged = list(rows)
                    damaged.pop(missing)
                    with self.assertRaises(AssertionError):
                        verify_recovery_trace(boundary, trace(damaged), lane, tool)
                for key in ('hook', 'active'):
                    with self.assertRaises(AssertionError):
                        verify_recovery_trace(boundary, {**after, key: False}, lane, tool)
                changed = trace(rows)
                changed['events'][1][1] += 1
                with self.assertRaisesRegex(AssertionError, 'history'):
                    verify_recovery_trace(boundary, changed, lane, tool)

    def test_fresh_runtime_snapshot_single_action_and_observer_effect(self):
        for app, stage, tool in (('calc', 'click_b2', 'click'), ('inkscape', 'scroll_down', 'scroll')):
            for failure in (None, 'slow_discovery', 'alive', 'reused', 'stale', 'identity', 'snapshot', 'unknown', 'denied', 'effect', 'trace', 'primary_before', 'primary_after'):
                with self.subTest(app=app, failure=failure), ExitStack() as stack:
                    victim, sibling, observer, fresh = [client(pid) for pid in (100, 101, 102, 103)]
                    victim.failed = True
                    victim.process.poll.return_value = None if failure == 'alive' else -9
                    if failure == 'reused':
                        fresh.process.pid = victim.process.pid
                    response = RESPONSE if failure != 'denied' else {'isError': True, 'structuredContent': {
                        'status': 'refused', 'refusal': {'code': 'permission_denied'}}}
                    fresh.tool.side_effect = [{}, TimeoutError('lost reply') if failure == 'unknown' else response]
                    spec = next(spec for spec in plan()['agents'] if spec['app'] == app)
                    before, after = {'proof_image': 'before.png'}, {'proof_image': 'after.png'}
                    dispatch_ns = 101
                    if failure == 'slow_discovery':
                        before['proof_observation_started_ns'] = MAX_GROUNDING_AGE_NS + 200
                        dispatch_ns = MAX_GROUNDING_AGE_NS + 301
                    snapshot = stack.enter_context(patch('production_cancel_proof.grounded_snapshot',
                        side_effect=AssertionError('geometry changed') if failure == 'snapshot' else [before, after]))
                    identity = stack.enter_context(patch('production_cancel_proof.app_process_identity',
                        side_effect=AssertionError('app changed') if failure == 'identity' else None,
                        return_value={'pid': spec['target']['pid']}))
                    stack.enter_context(patch('production_cancel_proof.time.monotonic_ns',
                        side_effect=[100, MAX_GROUNDING_AGE_NS + 101 if failure == 'stale' else dispatch_ns]))
                    stack.enter_context(patch('production_cancel_proof.pointer_grounding.read_pixels', return_value='pixels'))
                    arguments, oracle = {'x': 20, 'y': 30}, {'app': app, 'stage': stage}
                    action = stack.enter_context(patch('production_cancel_proof.pointer_grounding.action',
                        return_value=(arguments, oracle)))
                    verify = stack.enter_context(patch('production_cancel_proof.pointer_grounding.verify',
                        side_effect=AssertionError('no app effect') if failure == 'effect' else None,
                        return_value={'verified': True}))
                    page = trace(recovery_rows(tool))
                    if failure == 'trace':
                        page['overflow'] = True
                    trace_client, save, result = Mock(collect=Mock(return_value=page)), Mock(), {}
                    guard = Mock(side_effect=AssertionError('primary lifetime ended') if failure == 'primary_before'
                                 else [None, AssertionError('primary lifetime ended')] if failure == 'primary_after' else None)
                    def attempt():
                        return recover_once(fresh, observer, victim, sibling, spec, stage, trace_client,
                                            trace(FINISH[:-1]), 1, save, result, guard)
                    if failure not in (None, 'slow_discovery'):
                        with self.assertRaises((AssertionError, TimeoutError)):
                            attempt()
                    else:
                        self.assertEqual(attempt(), page)
                        self.assertEqual(result['result'], 'verified')
                        self.assertEqual(result['policy']['startup_profile'], PROFILE)
                        self.assertNotEqual(result['runtime_pid'], result['victim_pid'])
                        self.assertFalse(result['replayed'])
                        self.assertEqual(result['action']['dispatch_ns'], dispatch_ns)
                        self.assertEqual(result['grounding']['prepared_ns'],
                                         before.get('proof_observation_started_ns', 100))
                        identity.assert_called_once_with(app, spec['target']['pid'])
                        action.assert_called_once_with(before, 'pixels', app, stage)
                        verify.assert_called_once_with(after, 'pixels', oracle)
                        self.assertIs(snapshot.call_args_list[0].args[0], fresh)
                        self.assertIs(snapshot.call_args_list[1].args[0], observer)
                    inputs = [call for call in fresh.tool.call_args_list if call.args[0] != 'start_session']
                    self.assertEqual(len(inputs), 0 if failure in ('alive', 'reused', 'stale', 'identity', 'snapshot', 'primary_before') else 1)
                    if inputs:
                        self.assertEqual(inputs[0].args, (tool, {**arguments, **spec['target'],
                            'session': spec['name'] + '-recovery', 'delivery_mode': 'background'}))
                    if failure == 'unknown':
                        self.assertEqual(result['action']['outcome'], 'unknown')
                        self.assertFalse(result['action']['replayed'])
                        self.assertEqual([call.args[0] for call in save.call_args_list],
                            ['recovery-grounding.json', 'recovery-action.json', 'recovery-after.json'])
                        self.assertIs(snapshot.call_args_list[-1].args[0], observer)
                    victim.tool.assert_not_called()
                    sibling.tool.assert_not_called()


class OwnershipTests(unittest.TestCase):
    def test_both_grounding_calls_overlap_without_app_input(self):
        clients = [client(100), client(101)]
        specs = plan()['agents']
        rendezvous = threading.Barrier(2, timeout=2)
        save = Mock()
        def observe(owned, spec):
            rendezvous.wait()
            return {'target': spec['target'], 'session': spec['name'], 'prepared_ns': 100}
        with patch('production_cancel_proof.prepare_drag', side_effect=observe), \
             patch('production_cancel_proof.time.monotonic_ns', return_value=200):
            result = prepare_drags(clients, specs, save)
        self.assertEqual([item['target'] for item in result], [spec['target'] for spec in specs])
        self.assertEqual([call.args[0] for call in save.call_args_list],
                         ['agent-0-drag-grounding-attempt-1.json', 'agent-0-drag-grounding.json',
                          'agent-1-drag-grounding-attempt-1.json', 'agent-1-drag-grounding.json',
                          'drag-grounding-attempt-1.json'])
        for owned in clients:
            owned.tool.assert_not_called()

    def test_aging_pair_refreshes_both_observations_before_any_input(self):
        clients, specs, retained = [client(100), client(101)], plan()['agents'], {}
        counts = [0, 0]
        now = 10_000_000_000
        def observe(owned, spec):
            index = specs.index(spec)
            counts[index] += 1
            age = (4_995_561_185, 4_995_070_539)[index] if counts[index] == 1 else 2_000_000_000
            return {'target': spec['target'], 'session': spec['name'], 'prepared_ns': now - age,
                    'snapshot': {'fresh_attempt': counts[index]}}
        with patch('production_cancel_proof.prepare_drag', side_effect=observe), \
             patch('production_cancel_proof.time.monotonic_ns', return_value=now):
            result = prepare_drags(clients, specs, lambda name, value: retained.update({name: value}))
        self.assertEqual(counts, [2, 2])
        for index, item in enumerate(result):
            self.assertEqual(item['snapshot']['fresh_attempt'], 2)
            self.assertIs(retained[f'agent-{index}-drag-grounding.json'], item)
            self.assertEqual(retained[f'agent-{index}-drag-grounding-attempt-1.json']['snapshot']['fresh_attempt'], 1)
        self.assertFalse(retained['drag-grounding-attempt-1.json']['ready'])
        self.assertTrue(retained['drag-grounding-attempt-2.json']['ready'])
        for owned in clients:
            owned.tool.assert_not_called()

    def test_pair_freshness_boundary_and_attempt_cap(self):
        limit = MAX_GROUNDING_AGE_NS - GROUNDING_DISPATCH_RESERVE_NS
        for age, attempts, error in ((limit, 1, None), (limit + 1, 2, 'insufficient dispatch time'),
                                     (MAX_GROUNDING_AGE_NS + 1, 2, 'insufficient dispatch time'),
                                     (-1, 1, 'in the future')):
            with self.subTest(age=age):
                clients, save = [client(100), client(101)], Mock()
                with patch('production_cancel_proof.prepare_drag', return_value={'prepared_ns': 100}) as observe, \
                     patch('production_cancel_proof.time.monotonic_ns', return_value=100 + age):
                    if error:
                        with self.assertRaisesRegex(AssertionError, error):
                            prepare_drags(clients, plan()['agents'], save)
                    else:
                        prepare_drags(clients, plan()['agents'], save)
                self.assertEqual(observe.call_count, 2 * attempts)
                for owned in clients:
                    owned.tool.assert_not_called()

    def test_grounding_retention_time_counts_toward_pair_freshness(self):
        clients, now = [client(100), client(101)], [100]
        def save(name, item):
            if name == 'agent-1-drag-grounding.json':
                now[0] += MAX_GROUNDING_AGE_NS
        with patch('production_cancel_proof.prepare_drag', side_effect=lambda *args: {'prepared_ns': now[0]}) as observe, \
             patch('production_cancel_proof.time.monotonic_ns', side_effect=lambda: now[0]):
            with self.assertRaisesRegex(AssertionError, 'insufficient dispatch time; no input sent'):
                prepare_drags(clients, plan()['agents'], save)
        self.assertEqual(observe.call_count, 4)
        for owned in clients:
            owned.tool.assert_not_called()

    def test_pair_requires_each_item_to_have_budget(self):
        now = 10_000_000_000
        for stale in (0, 1):
            prepared = [{'prepared_ns': now}, {'prepared_ns': now}]
            prepared[stale]['prepared_ns'] = now - MAX_GROUNDING_AGE_NS + GROUNDING_DISPATCH_RESERVE_NS - 1
            with patch('production_cancel_proof.time.monotonic_ns', return_value=now):
                self.assertFalse(pair_has_dispatch_budget(prepared))
            self.assertEqual(prepared[stale]['timing']['pair_gate_ns'], now)

    def test_failed_parallel_grounding_never_dispatches(self):
        clients = [client(100), client(101)]
        save = Mock()
        with patch('production_cancel_proof.prepare_drag', side_effect=AssertionError('bad image')) as observe:
            with self.assertRaisesRegex(AssertionError, 'bad image'):
                prepare_drags(clients, plan()['agents'], save)
        self.assertEqual(observe.call_count, 2)
        save.assert_not_called()
        for owned in clients:
            owned.tool.assert_not_called()

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

    def test_freshness_starts_before_snapshot_and_records_each_phase(self):
        spec, mcp = plan()['agents'][0], Mock()
        mcp.tool.return_value = RESPONSE
        before = {'proof_observation_started_ns': 200}
        with patch('production_cancel_proof.grounded_snapshot', return_value=before), \
             patch('production_cancel_proof.time.monotonic_ns', side_effect=[100, 300, 400, 500]):
            prepared = prepare_drag(mcp, spec)
            call_drag(mcp, spec, prepared)
        self.assertEqual(prepared['prepared_ns'], 200)
        self.assertEqual(prepared['timing'], {
            'preparation_started_ns': 100, 'observation_started_ns': 200,
            'observation_finished_ns': 300, 'grounding_finished_ns': 400,
            'dispatch_attempt_ns': 500, 'grounding_age_ns': 300})

    def test_slow_snapshot_or_guard_expires_without_input_and_retains_age(self):
        spec, mcp = plan()['agents'][0], Mock()
        for slow_phase in ('snapshot', 'guard'):
            with self.subTest(slow_phase=slow_phase):
                now = [100]
                def snapshot(*args):
                    if slow_phase == 'snapshot':
                        now[0] += MAX_GROUNDING_AGE_NS + 1
                    return {'proof_observation_started_ns': 100}
                def guard():
                    if slow_phase == 'guard':
                        now[0] += MAX_GROUNDING_AGE_NS + 1
                with patch('production_cancel_proof.grounded_snapshot', side_effect=snapshot), \
                     patch('production_cancel_proof.time.monotonic_ns', side_effect=lambda: now[0]):
                    prepared = prepare_drag(mcp, spec)
                    with self.assertRaisesRegex(AssertionError, 'snapshot expired; no input sent'):
                        call_drag(mcp, spec, prepared, guard)
                self.assertEqual(prepared['timing']['grounding_age_ns'], MAX_GROUNDING_AGE_NS + 1)
        mcp.tool.assert_not_called()

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
        with patch('production_cancel_proof.grounded_snapshot', return_value={}) as snapshot, \
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
            content = {'snapshot_id': 's00000001', 'window_bounds': BOUNDS, 'screenshot_width': 800, 'screenshot_height': 600}
            for image_file, valid in [('image.png', True), ('missing.png', False), ('../escape.png', False)]:
                mcp = Mock(directory=root, process=Mock(pid=101, poll=Mock(return_value=None)), tool=Mock(side_effect=[
                    {'structuredContent': {'windows': [spec['target']]}},
                    {'structuredContent': content, 'content': [{'type': 'image', 'image_file': image_file}]}]))
                if valid:
                    result = grounded_snapshot(mcp, spec['target'], spec)
                    self.assertEqual(result['proof_image'], str(root / 'image.png'))
                    self.assertNotIn('max_elements', mcp.tool.call_args.args[1])
                else:
                    with self.assertRaises((AssertionError, FileNotFoundError)):
                        grounded_snapshot(mcp, spec['target'], spec)

    def test_inkscape_pointer_snapshot_bounds_walk_without_limiting_depth(self):
        spec = {**plan()['agents'][1], 'drag': {}, 'pointer_stage': 'move_rectangle'}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            now = [100]
            snapshots = [0]
            def tool(name, arguments):
                if name == 'list_windows':
                    now[0] = 200
                    return {'structuredContent': {'windows': [spec['target']]}}
                now[0] = 300
                snapshots[0] += 1
                image_file = f'image-{snapshots[0]}.png'
                (root / image_file).touch()
                return {'structuredContent': {'snapshot_id': f's{snapshots[0]:08x}', 'window_bounds': BOUNDS, 'screenshot_width': 800,
                                              'screenshot_height': 600},
                        'content': [{'type': 'image', 'image_file': image_file}]}
            mcp = Mock(directory=root, process=Mock(pid=101, poll=Mock(return_value=None)), tool=Mock(side_effect=tool))
            with patch('production_cancel_proof.time.monotonic_ns', side_effect=lambda: now[0]):
                result = grounded_snapshot(mcp, spec['target'], spec)
            self.assertEqual(result['proof_observation_started_ns'], 200)
            self.assertEqual(result['proof_observation_finished_ns'], 300)
            self.assertEqual(result['proof_runtime'], {'pid': 101, 'directory': str(root.resolve())})
            self.assertEqual(mcp.tool.call_args.args, ('get_window_state', {
                **spec['target'], 'session': spec['name'], 'max_elements': 2500}))
            # A bounded walk is not permission to omit the existing oracle.
            with patch('production_cancel_proof.pointer_grounding.read_pixels', return_value='pixels'):
                with self.assertRaisesRegex(RuntimeError, 'snapshot has no semantic elements'):
                    prepare_drag(mcp, spec)
            self.assertTrue(all(call.args[0] in ('list_windows', 'get_window_state')
                                for call in mcp.tool.call_args_list))

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

    def test_sigterm_is_preselected_and_never_falls_back_to_kill(self):
        candidate = {**plan(), 'termination_signal': 'SIGTERM'}
        validate_plan(candidate)
        victim, sibling = client(100), client(101)
        result = terminate_owned(victim, sibling, [Mock(done=lambda: False)], 'SIGTERM')
        self.assertTrue(victim.failed)
        self.assertEqual(result['signal'], 'SIGTERM')
        self.assertEqual(result['pid'], 100)
        victim.process.terminate.assert_called_once_with()
        victim.process.kill.assert_not_called()
        sibling.process.terminate.assert_not_called()
        sibling.process.kill.assert_not_called()

        victim = client(100)
        victim.process.terminate.side_effect = None
        victim.process.wait.side_effect = subprocess.TimeoutExpired('owned-driver', 3)
        with self.assertRaises(subprocess.TimeoutExpired):
            terminate_owned(victim, sibling, [Mock(done=lambda: False)], 'SIGTERM')
        self.assertTrue(victim.failed)
        victim.process.terminate.assert_called_once_with()
        victim.process.kill.assert_not_called()

    def test_unknown_signal_is_refused_before_process_mutation(self):
        for signal in ('SIGINT', 'SIGSTOP', '', None, 9):
            victim, sibling = client(100), client(101)
            with self.assertRaises(AssertionError):
                validate_plan({**plan(), 'termination_signal': signal})
            with self.assertRaises(AssertionError):
                terminate_owned(victim, sibling, [Mock(done=lambda: False)], signal)
            self.assertFalse(victim.failed)
            victim.process.kill.assert_not_called()
            victim.process.terminate.assert_not_called()

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
        with patch('production_cancel_proof.grounded_snapshot', return_value={}) as snapshot:
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
        good = {'structuredContent': {'snapshot_id': 's00000001', 'window_bounds': BOUNDS, 'screenshot_width': 800, 'screenshot_height': 600}}
        for windows, content in [([], good), ([{'pid': 20, 'window_id': 999}], good),
                                 ([spec['target']], {'structuredContent': {**good['structuredContent'], 'window_bounds': {}}}),
                                 ([spec['target']], {'structuredContent': {**good['structuredContent'], 'screenshot_width': 50}})]:
            mcp = Mock(directory=Path.cwd(), process=Mock(pid=101, poll=Mock(return_value=None)),
                       tool=Mock(side_effect=[{'structuredContent': {'windows': windows}}, content]))
            with self.assertRaises(AssertionError):
                grounded_snapshot(mcp, spec['target'], spec)


class ObservationTests(unittest.TestCase):
    def test_counter_identity_and_original_chronology(self):
        with tempfile.TemporaryDirectory() as directory:
            observer = Mock(directory=Path(directory), process=Mock(pid=102, poll=Mock(return_value=None)))
            runtime = {'pid': 102, 'directory': str(Path(directory).resolve())}
            before = {'snapshot_id': 's00000001', 'proof_runtime': {**runtime, 'pid': 101},
                'proof_image': str(Path(directory) / 'before.png'),
                'proof_observation_started_ns': 10, 'proof_observation_finished_ns': 20}
            after = {**before, 'proof_runtime': runtime, 'proof_image': str(Path(directory) / 'after.png'),
                'proof_observation_started_ns': 40, 'proof_observation_finished_ns': 50}
            for failure in (None, 'cached', 'same_artifact', 'artifact_alias', 'same_runtime_counter',
                            'wrong_runtime', 'dead', 'before_return', 'future', 'invalid_time'):
                with self.subTest(failure=failure), patch('production_cancel_proof.time.monotonic_ns', return_value=60):
                    old, new = dict(before), dict(after)
                    observer.process.poll.return_value = 0 if failure == 'dead' else None
                    if failure == 'cached':
                        new.update(proof_observation_started_ns=10, proof_observation_finished_ns=20)
                    elif failure in ('same_artifact', 'artifact_alias'):
                        new['proof_image'] = old['proof_image'] if failure == 'same_artifact' else str(Path(directory) / 'unused' / '..' / 'before.png')
                    elif failure == 'same_runtime_counter':
                        old['proof_runtime'] = runtime
                    elif failure == 'wrong_runtime':
                        new['proof_runtime'] = old['proof_runtime']
                    elif failure == 'before_return':
                        new['proof_observation_started_ns'] = 29
                    elif failure == 'future':
                        new['proof_observation_finished_ns'] = 61
                    elif failure == 'invalid_time':
                        new['proof_observation_started_ns'] = True
                    if failure:
                        with self.assertRaises(AssertionError):
                            verify_fresh_observation(old, new, observer, after_ns=30)
                    else:
                        verify_fresh_observation(old, new, observer, after_ns=30)
                        self.assertEqual(new['proof_observation_started_ns'], 40)

    def test_observation_rejects_runtime_exit_during_snapshot(self):
        spec = plan()['agents'][0]
        for polls in ([0], [None, 0]):
            mcp = Mock(directory=Path.cwd(), process=Mock(pid=101, poll=Mock(side_effect=polls)),
                tool=Mock(side_effect=[{'structuredContent': {'windows': [spec['target']]}},
                    {'structuredContent': {'window_bounds': BOUNDS, 'screenshot_width': 800, 'screenshot_height': 600}}]))
            with self.subTest(polls=polls), self.assertRaisesRegex(AssertionError, 'process exited'):
                grounded_snapshot(mcp, spec['target'], spec)
            self.assertEqual(mcp.tool.call_count, len(polls))

    def test_runtime_cache_cannot_restamp_reused_counter_or_image(self):
        spec = {**plan()['agents'][0], 'drag': {}, 'pointer_stage': 'select_range'}
        for failure in (None, 'counter', 'image'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'first.png').touch()
                (root / 'second.png').touch()
                def response(snapshot_id, image_file):
                    return {'structuredContent': {'snapshot_id': snapshot_id, 'window_bounds': BOUNDS,
                        'screenshot_width': 800, 'screenshot_height': 600},
                        'content': [{'type': 'image', 'image_file': image_file}]}
                windows = {'structuredContent': {'windows': [spec['target']]}}
                first = response('s00000001', 'first.png')
                second = response('s00000001' if failure == 'counter' else 's00000002',
                                  'first.png' if failure == 'image' else 'second.png')
                mcp = Mock(directory=root, process=Mock(pid=101, poll=Mock(return_value=None)),
                    tool=Mock(side_effect=[windows, first, windows, second]))
                with patch('production_cancel_proof.time.monotonic_ns', side_effect=[10, 20, 40, 50]):
                    old = grounded_snapshot(mcp, spec['target'], spec)
                    if failure:
                        with self.assertRaisesRegex(AssertionError, 'reused'):
                            grounded_snapshot(mcp, spec['target'], spec)
                    else:
                        new = grounded_snapshot(mcp, spec['target'], spec)
                        self.assertEqual(new['proof_observation_started_ns'], 40)
                self.assertEqual(old['proof_observation_started_ns'], 10)


class RunnerTests(unittest.TestCase):
    def test_success_and_failures_reap_all_owned_children_without_replay(self):
        for failure in (None, 'sigterm', 'pointer', 'pointer_partial', 'pointer_effect', 'pointer_endpoint', 'pointer_anchor',
                        'pointer_coordinates', 'trace', 'unknown_sibling',
                        'successful_victim', 'close', 'snapshot', 'grab', 'primary_before', 'primary_after', 'dispatch_budget',
                        'prepare_budget',
                        'recovery_calc', 'recovery_inkscape', 'recovery_effect', 'recovery_close', 'recovery_trace'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                root = Path(directory)
                source_plan = root / 'plan.json'
                candidate = plan()
                if failure == 'sigterm':
                    candidate['termination_signal'] = 'SIGTERM'
                recovery = failure is not None and failure.startswith('recovery_')
                pointer = failure is not None and failure.startswith('pointer')
                if pointer or recovery:
                    for spec, stage in zip(candidate['agents'], ('select_range', 'move_rectangle')):
                        spec.update(drag={}, pointer_stage=stage)
                if recovery:
                    candidate['kill_agent'] = 1 if failure == 'recovery_inkscape' else 0
                    candidate['recovery'] = {'pointer_stage': 'scroll_down' if candidate['kill_agent'] else 'click_b2'}
                victim = candidate['kill_agent']
                source_plan.write_text(json.dumps(candidate))
                args = SimpleNamespace(plan=source_plan, evidence=root / 'evidence', driver=root / 'driver',
                                       trace_socket=root / 'cua-input-v3.sock', primary_grab=root / 'primary-grab',
                                       foreground_journal=root / 'journal')
                agents, observer = [client(100), client(101)], client(102)
                fresh = client(103)
                fresh.tool.side_effect = [{}, RESPONSE]
                for owned in [*agents, observer, fresh]:
                    owned.close.side_effect = lambda c=owned: setattr(c.process.poll, 'return_value', 0)
                if failure == 'recovery_close':
                    fresh.close.side_effect = RuntimeError('recovery close failed')
                observer.tool.return_value = {'structuredContent': {'screen_width': 1000, 'screen_height': 800}}
                for agent in agents:
                    agent.tool.return_value = {'structuredContent': {}}
                held = [True]
                grab = Mock(stdout=Mock(), poll=Mock(return_value=None))
                if failure == 'primary_before':
                    grab.poll.return_value = 0
                trace_client = Mock(hello={'protocol': 3})
                make_trace = wire_trace if pointer or recovery else trace
                stopped = make_trace(FINISH, False)
                if failure in ('pointer_endpoint', 'pointer_coordinates'):
                    endpoint = next(row for row in stopped['events']
                                    if row[2] == 'pointer_motion' and row[7:9] == [200, 200])
                    if failure == 'pointer_endpoint':
                        endpoint[7:9] = [199, 199]
                    else:
                        del endpoint[7:9]
                if failure == 'trace':
                    stopped['overflow'] = True
                trace_client.collect.side_effect = [make_trace(START), make_trace(FIRST), make_trace(OVERLAP), stopped]
                if recovery:
                    rows = recovery_rows('scroll' if victim else 'click')
                    complete = make_trace(rows + [(2015, 'agent_cancel', 1, 0), (2016, 'pointer_leave', 1, 0),
                        (2017, 'agent_cancel', 2, 0), (2018, 'keyboard_leave', 2, 0), (2020, 'stop', 0, 0)], False)
                    if failure == 'recovery_trace':
                        complete['overflow'] = True
                    trace_client.collect.side_effect = [make_trace(START), make_trace(FIRST), make_trace(OVERLAP),
                        make_trace(FINISH[:-1]), *([] if failure == 'recovery_effect' else [make_trace(rows)]), complete]
                outcomes = [{'outcome': 'unknown', 'replayed': False},
                            {'outcome': 'response', 'response': RESPONSE, 'replayed': False}]
                if failure == 'unknown_sibling':
                    outcomes[1] = outcomes[0]
                if failure == 'successful_victim':
                    outcomes[0] = outcomes[1]
                if failure == 'pointer_partial':
                    outcomes[0] = {'outcome': 'response', 'response': {'structuredContent': {
                        **RESPONSE['structuredContent'], 'effect': 'partial',
                        'delivery': {'mode': 'background', 'delivered_count': 3}}}, 'replayed': False}
                futures = [Mock(done=Mock(return_value=False), result=Mock(return_value=o)) for o in outcomes]
                for future in futures:
                    def finish(*args, f=future, **kwargs):
                        f.done.return_value = True
                        return f.result.return_value
                    future.result.side_effect = finish
                pool = Mock(submit=Mock(side_effect=futures))
                proof_image = root / 'fresh.png'
                proof_image.write_bytes(b'synthetic-test-image')
                snapshot = Mock(return_value={'window_bounds': BOUNDS, 'proof_image': str(proof_image)})
                if failure == 'prepare_budget':
                    snapshot.return_value['proof_observation_started_ns'] = 1
                if failure == 'snapshot':
                    snapshot.side_effect = AssertionError('stale geometry')
                if failure == 'primary_after':
                    def primary_exits_after_action(*args, **kwargs):
                        if kwargs.get('session') is False:
                            grab.poll.return_value = 0
                        return snapshot.return_value
                    snapshot.side_effect = primary_exits_after_action
                if failure == 'close':
                    agents[0].close.side_effect = RuntimeError('cleanup failure')
                sibling_effect = {'verified': True, 'app': candidate['agents'][1 - victim]['app'],
                                  'observed_delta': [80, 80] if failure == 'pointer_anchor' else [90, 90]}
                replacements = {
                    'provenance': Mock(return_value={'files': {}}), 'DirectMCP': Mock(side_effect=[*agents, observer, fresh]),
                    'app_process_identity': Mock(return_value={'pid': candidate['agents'][victim]['target']['pid']}),
                    'grounded_snapshot': snapshot, 'subprocess.Popen': Mock(return_value=grab),
                    'primary_acknowledgement': Mock(side_effect=AssertionError('missing HELD')) if failure == 'grab' else Mock(return_value='HELD\n'),
                    'wait_for': lambda fn: self.assertTrue(fn()),
                    'state': lambda path: {'held': held[0], 'clicks': 0, 'keys': 0, 'scroll': 0},
                    'wm': lambda: {'pid': 10, 'cursor': {'x': 100, 'y': 100}}, 'Trace': Mock(return_value=trace_client),
                    'ThreadPoolExecutor': Mock(return_value=pool), 'stop_process': lambda proc: held.__setitem__(0, False),
                    'pointer_grounding.read_pixels': Mock(return_value='pixels'),
                    'pointer_grounding.action': Mock(side_effect=lambda before, pixels, app, stage:
                        (plan()['agents'][0]['drag'] if stage in ('select_range', 'move_rectangle') else {'x': 20, 'y': 30},
                         {'app': app, 'stage': stage})),
                    'pointer_grounding.verify': Mock(side_effect=AssertionError('sibling did not move'))
                        if failure == 'pointer_effect' else Mock(side_effect=[sibling_effect, AssertionError('recovery no effect')])
                        if failure == 'recovery_effect' else Mock(return_value=sibling_effect),
                    'print': Mock(),
                }
                # The real concurrent preparation helper has its own barrier
                # tests. Keep this runner's fake executor scoped to actions.
                def observations(owned, specs, save):
                    prepared = [prepare_drag(c, s) for c, s in zip(owned, specs)]
                    for i, item in enumerate(prepared):
                        if failure == 'dispatch_budget':
                            item['prepared_ns'] -= MAX_GROUNDING_AGE_NS
                        save(f'agent-{i}-drag-grounding.json', item)
                    return prepared
                if failure == 'prepare_budget':
                    # Exercise the real bounded preparation loop, including
                    # its two read-only executors and failure cleanup.
                    executor_count = [0]
                    def executor(*args, **kwargs):
                        executor_count[0] += 1
                        return pool if executor_count[0] == 1 else ThreadPoolExecutor(*args, **kwargs)
                    replacements['ThreadPoolExecutor'] = executor
                    replacements['prepare_drag'] = Mock(wraps=prepare_drag)
                else:
                    replacements['prepare_drags'] = observations
                for name, value in replacements.items():
                    stack.enter_context(patch('production_cancel_proof.' + name, value))
                self.assertEqual(run(args), 0 if failure in (None, 'sigterm', 'pointer', 'pointer_partial', 'recovery_calc', 'recovery_inkscape') else 1)
                result = json.loads((args.evidence / 'result.json').read_text())
                if failure == 'sigterm':
                    self.assertEqual(result['termination']['signal'], 'SIGTERM')
                    agents[victim].process.terminate.assert_called_once_with()
                    agents[victim].process.kill.assert_not_called()
                if not recovery:
                    self.assertEqual(result['reacquisition'], 'unproven')
                else:
                    self.assertEqual(result['reacquisition']['result'], 'unproven' if failure == 'recovery_effect' else 'verified')
                    self.assertEqual(result['cancellation']['victim_lane'], 1)
                    self.assertEqual(result['termination']['pid'], agents[victim].process.pid)
                    self.assertEqual(result['saved_app_effects'], 'unproven')
                    self.assertEqual(result['interrupted_state']['saved_document_effect'], 'unproven')
                    self.assertEqual(proof_image.read_bytes(), b'synthetic-test-image')
                    self.assertTrue((args.evidence / 'interrupted-state.json').is_file())
                    self.assertTrue((args.evidence / 'recovery-grounding.json').is_file())
                    self.assertTrue((args.evidence / 'recovery-after.json').is_file())
                    self.assertEqual(fresh.tool.call_count, 2)
                    fresh.close.assert_called_once()
                    self.assertIsNotNone(fresh.process.poll())
                if failure in ('pointer', 'pointer_effect'):
                    replacements['pointer_grounding.verify'].assert_called_once()
                    self.assertEqual(replacements['pointer_grounding.action'].call_count, 2)
                    self.assertTrue((args.evidence / 'agent-0-drag-grounding.json').is_file())
                    self.assertTrue((args.evidence / 'agent-1-drag-grounding.json').is_file())
                if failure in ('pointer', 'pointer_partial') or recovery:
                    self.assertEqual(result['sibling_app_effect'], sibling_effect)
                    if failure != 'recovery_trace':
                        self.assertTrue(result['sibling_pointer_delivery']['verified'])
                        self.assertEqual(result['sibling_pointer_delivery']['lane'], 2)
                if failure == 'pointer_partial':
                    self.assertEqual(result['actions'][str(victim)], outcomes[0])
                    self.assertEqual(result['saved_app_effects'], 'unproven')
                if pointer and failure not in ('pointer', 'pointer_partial'):
                    self.assertEqual(result['sibling_pointer_delivery'], 'unproven')
                    self.assertEqual(result['actions'][str(victim)]['outcome'], 'unknown')
                    self.assertFalse(result['actions'][str(victim)]['replayed'])
                    if failure != 'pointer_effect':
                        self.assertEqual(result['sibling_app_effect'], sibling_effect)
                        self.assertIn('finish_trace', (args.evidence / 'cleanup.json').read_text())
                agents[0].close.assert_called_once()
                if failure != 'snapshot':
                    agents[1].close.assert_called_once()
                    observer.close.assert_called_once()
                    self.assertFalse(held[0])
                if failure == 'primary_before':
                    pool.submit.assert_not_called()
                    agents[victim].process.kill.assert_not_called()
                    self.assertIn('primary', result['error'])
                if failure == 'primary_after':
                    self.assertIn('primary', result['error'])
                if failure in ('dispatch_budget', 'prepare_budget'):
                    pool.submit.assert_not_called()
                    self.assertNotIn('termination', result)
                    self.assertIn('no input sent', result['error'])
                    for i, owned in enumerate(agents):
                        owned.process.kill.assert_not_called()
                        owned.process.terminate.assert_not_called()
                        saved = json.loads((args.evidence / f'agent-{i}-drag-grounding.json').read_text())
                        self.assertGreaterEqual(saved['timing']['pair_grounding_age_ns'], MAX_GROUNDING_AGE_NS)
                    trace_client.close.assert_called_once()
                    if failure == 'prepare_budget':
                        self.assertEqual(replacements['prepare_drag'].call_count, 4)
                        self.assertEqual(executor_count[0], 3)
                        for attempt in (1, 2):
                            saved_attempt = json.loads((args.evidence / f'drag-grounding-attempt-{attempt}.json').read_text())
                            self.assertFalse(saved_attempt['ready'])
                            self.assertFalse(saved_attempt['input_attempted'])
                        self.assertFalse((args.evidence / 'drag-grounding-attempt-3.json').exists())
                if failure not in ('snapshot', 'grab', 'primary_before', 'dispatch_budget', 'prepare_budget'):
                    self.assertEqual(pool.submit.call_count, 2)
                    if failure == 'sigterm':
                        agents[victim].process.terminate.assert_called_once()
                    else:
                        agents[victim].process.kill.assert_called_once()
                    agents[1 - victim].process.kill.assert_not_called()
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
