"""Portable failure-oracle tests only; never native lock certification."""
from concurrent.futures import Future
from contextlib import ExitStack
from copy import deepcopy
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_active_lock_proof as proof
import production_lock_refusal_proof as settled
from production_lock_refusal_proof_test import plan as lock_plan
from production_session_fault_proof_test import ACTIVE, CANCEL, PARTIAL, status, trace


def plan():
    return {**lock_plan(), 'purpose': 'active_lock',
            'recovery': {'pointer_stages': ['click_a1', 'click_b2']}}


def action():
    response = deepcopy(PARTIAL)
    response['structuredContent']['reason'] = 'desktop_changed'
    return {'outcome': 'response', 'replayed': False, 'response': response,
            'dispatch_ns': 500_000, 'observed_ns': 11_000_000}


def record():
    return {'result': 'acknowledged', 'before': status(1),
        'ready': {'event': 'ready', 'observed_ns': 1},
        'prefix': trace(ACTIVE), 'lane': 1, 'status_started_ns': 5_000_000,
        'requested_ns': 6_000_000, 'ack': {'event': 'locked', 'observed_ns': 7_000_000},
        'observed_ns': 12_000_000, 'deadline_ns': 20_000_000,
        'gate_status': status(1, held=True), 'after': status(2)}


class OracleTests(unittest.TestCase):
    def test_recovery_grounding_uses_original_snapshot_time_not_wrapper_clock(self):
        client = Mock()
        spec = plan()['agents'][0]
        snapshot = {'proof_observation_started_ns': 10, 'proof_image': 'synthetic.png'}
        with patch.object(proof, 'grounded_snapshot', return_value=snapshot) as observe, \
             patch.object(proof, 'recovery_stage', return_value='click_b2'), \
             patch.object(proof.time, 'monotonic_ns', return_value=50), \
             patch.object(proof.pointer_grounding, 'read_pixels', return_value='pixels'), \
             patch.object(proof.pointer_grounding, 'action', return_value=({'x': 1}, {'cell': 'B2'})):
            result = proof.prepare_recovery(client, spec, ['click_a1', 'click_b2'])
        observe.assert_called_once_with(client, spec['target'], spec)
        self.assertEqual(result, {'snapshot': snapshot, 'arguments': {'x': 1},
            'oracle': {'cell': 'B2'}, 'stage': 'click_b2', 'prepared_ns': 10})
        client.tool.assert_not_called()

    def test_recovery_grounding_rejects_missing_invalid_or_future_snapshot_time(self):
        for timestamp in (None, True, 0, -1, 51, 10.0):
            snapshot = {'proof_image': 'synthetic.png'}
            if timestamp is not None:
                snapshot['proof_observation_started_ns'] = timestamp
            with self.subTest(timestamp=timestamp), \
                 patch.object(proof, 'grounded_snapshot', return_value=snapshot), \
                 patch.object(proof.time, 'monotonic_ns', return_value=50), \
                 patch.object(proof.pointer_grounding, 'action') as action, \
                 self.assertRaises((AssertionError, KeyError)):
                proof.prepare_recovery(Mock(), plan()['agents'][0], ['click_a1', 'click_b2'])
            action.assert_not_called()

    def test_recovery_stage_is_chosen_from_new_selection_before_input(self):
        snapshot = {'snapshot_id': 'fresh-after-unlock'}
        for selected_b2, expected in ((True, 'click_a1'), (False, 'click_b2')):
            with patch.object(proof.pointer_grounding, 'rows', return_value=['fresh']) as rows, \
                 patch.object(proof.pointer_grounding, 'calc_formula_selection', return_value=selected_b2) as selection:
                self.assertEqual(proof.recovery_stage(snapshot), expected)
            rows.assert_called_once_with(snapshot)
            selection.assert_called_once_with(snapshot, ['fresh'], 'B2')

    def test_plan_requires_reviewed_disposable_calc_lock_and_fresh_recovery(self):
        proof.validate_plan(plan())
        for change in ({'purpose': 'lock_refusal'}, {'fault': {'kind': 'dpms'}},
                       {'disposable': False}, {'fault': {'kind': 'lock', 'kill': True}},
                       {'recovery': {'pointer_stage': 'select_range'}}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.validate_plan({**plan(), **change})
        for field in ('path', 'sha256', 'source_sha256', 'uid', 'inode'):
            candidate = plan()
            candidate['lock_fixture'][field] = 'unreviewed'
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.validate_plan(candidate)

    def test_held_gate_cancel_and_own_release_are_verified_without_isolation_claim(self):
        result = proof.verify_cancelled(trace(CANCEL), record(), action())
        self.assertEqual(result['result'], 'verified')
        self.assertEqual(result['reason'], 'desktop_changed')
        self.assertEqual(result['continuous_primary_isolation'], 'unproven')

    def test_primary_transition_violations_are_retained_never_filtered_to_pass(self):
        for kind in ('cursor', 'pointer_focus', 'keyboard_focus', 'pointer_button', 'keyboard_key'):
            page = trace(CANCEL + [(13, kind, 0, 0)])
            if kind == 'cursor':
                page['events'][-1][3] += 10
            original = deepcopy(page)
            result = proof.transition_evidence(page)
            self.assertEqual(result['raw_primary_analysis']['result'], 'failed')
            self.assertEqual(result['continuous_primary_isolation'], 'unproven')
            self.assertEqual(result['primary_transition_events'], [page['events'][-1]])
            self.assertEqual(page, original)
            # This oracle proves synthetic cancellation only, not the cause
            # or acceptability of any primary-seat transition event.
            self.assertEqual(proof.verify_cancelled(page, record(), action())['result'], 'verified')

    def test_primary_activity_before_lock_is_not_a_transition_exception(self):
        candidate = record()
        candidate['prefix'] = trace(ACTIVE + [(5, 'pointer_focus', 0, 0)])
        page = trace(ACTIVE + [(5, 'pointer_focus', 0, 0)] + CANCEL[len(ACTIVE):])
        with self.assertRaisesRegex(AssertionError, 'pre-LOCK isolation'):
            proof.verify_cancelled(page, candidate, action())

    def test_unrelated_input_before_lock_is_not_an_ordinary_drag(self):
        candidate = record()
        candidate['prefix'] = trace(ACTIVE + [(5, 'pointer_axis', 1, 0)])
        page = trace(ACTIVE + [(5, 'pointer_axis', 1, 0)] + CANCEL[len(ACTIVE):])
        with self.assertRaisesRegex(AssertionError, 'unexpected input before LOCK'):
            proof.verify_cancelled(page, candidate, action())

    def test_missing_release_extra_input_cross_lane_and_false_completion_fail(self):
        tails = [
            [(8, 'agent_cancel', 1, 0)],
            [(8, 'agent_cancel', 1, 0), (9, 'pointer_button', 2, 0)],
            [(8, 'pointer_button', 1, 0), (9, 'agent_cancel', 1, 0)],
            [(8, 'agent_cancel', 1, 0), (9, 'pointer_button', 1, 1)],
            [(8, 'agent_cancel', 2, 0), (9, 'pointer_button', 1, 0)],
            [(5, 'agent_cancel', 1, 0), (9, 'pointer_button', 1, 0)],
        ]
        tails += [CANCEL[len(ACTIVE):] + [(11, kind, 1, 0)] for kind in
                  ('agent_cancel', 'agent_admitted', 'agent_drag_start', 'agent_drag_end',
                   'agent_action_end', 'keyboard_key', 'pointer_axis', 'pointer_motion',
                   'pointer_enter', 'pointer_button')]
        for tail in tails:
            with self.subTest(tail=tail), self.assertRaises(AssertionError):
                proof.verify_cancelled(trace(ACTIVE + tail), record(), action())

    def test_incomplete_trace_rewritten_history_or_missing_held_prefix_fail(self):
        for field, value in (('overflow', True), ('timed_out', True), ('hook', False),
                             ('active', False), ('count', 100)):
            page = trace(CANCEL)
            page[field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.verify_cancelled(page, record(), action())
        for mutation in ('history', 'press', 'admitted', 'start'):
            candidate, page = record(), trace(CANCEL)
            if mutation == 'history':
                page['events'][0][1] += 1
            else:
                index = {'press': 3, 'admitted': 1, 'start': 2}[mutation]
                candidate['prefix']['events'][index][2] = 'pointer_motion'
                page['events'][index][2] = 'pointer_motion'
            with self.subTest(mutation=mutation), self.assertRaises(AssertionError):
                proof.verify_cancelled(page, candidate, action())

    def test_stale_or_unacknowledged_lock_and_bad_clocks_fail(self):
        for change in ({'result': 'unproven'}, {'requested_ns': 3_000_000},
                       {'requested_ns': 300_000_000}, {'status_started_ns': -300_000_000},
                       {'ready': {'event': 'ready', 'observed_ns': -3_000_000_000}},
                       {'ack': {'event': 'finished', 'observed_ns': 7_000_000}},
                       {'ack': {'event': 'locked', 'observed_ns': 5_000_000}},
                       {'deadline_ns': 10_000_000}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.verify_cancelled(trace(CANCEL), {**record(), **change}, action())

    def test_unknown_refused_replayed_or_wrong_reason_cannot_certify_cancellation(self):
        for change in ({'outcome': 'unknown'}, {'replayed': True}, {'observed_ns': 30_000_000},
                       {'dispatch_ns': 5_000_000}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.verify_cancelled(trace(CANCEL), record(), {**action(), **change})
        for field, value in (('reason', 'session_unavailable'), ('reason', 'primary_target_busy'),
                             ('effect', 'refused'), ('route', 'foreground')):
            candidate = action()
            candidate['response']['structuredContent'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                proof.verify_cancelled(trace(CANCEL), record(), candidate)
        for delivery in ({'mode': 'unknown', 'delivered_count': 1},
                         {'mode': 'background', 'delivered_count': 0}):
            candidate = action()
            candidate['response']['structuredContent']['delivery'] = delivery
            with self.subTest(delivery=delivery), self.assertRaises(AssertionError):
                proof.verify_cancelled(trace(CANCEL), record(), candidate)

    def test_status_requires_held_owned_lane_then_cleared_authority_and_no_dispatch(self):
        for section, key, value, index in (
            ('gate_status', 'held_button', 0, 0), ('gate_status', 'held_keys', 1, 0),
            ('gate_status', 'drag_active', False, 0), ('gate_status', 'reserved', False, 0),
            ('gate_status', 'held_button', 272, 1), ('gate_status', 'reserved', True, 1),
            ('after', 'held_button', 272, 0), ('after', 'held_keys', 1, 0),
            ('after', 'drag_active', True, 0), ('after', 'lease_active', True, 0),
            ('after', 'pointer_focus', True, 0), ('after', 'keyboard_focus', True, 0),
            ('after', 'reserved', True, 0), ('after', 'desktop_generation', 1, 0),
            ('after', 'dispatches', 1, 0), ('after', 'epoch', 'replaced', 0)):
            candidate = record()
            candidate[section]['input']['lanes'][index][key] = value
            with self.subTest(section=section, key=key, index=index), self.assertRaises(AssertionError):
                proof.verify_cancelled(trace(CANCEL), candidate, action())

    def test_unlock_trace_keeps_primary_failures_but_rejects_any_new_synthetic_event(self):
        boundary = trace(CANCEL)
        stopped = proof.stopped_prefix(trace(CANCEL + [(14, 'keyboard_focus', 0, 0)]))
        result = proof.verify_transition_end(boundary, stopped)
        self.assertEqual(result['raw_primary_analysis']['result'], 'failed')
        self.assertFalse(result['analysis_uses_end_sentinel'])
        for kind in ('pointer_motion', 'pointer_button', 'agent_admitted', 'agent_cancel', 'pointer_leave'):
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                proof.verify_transition_end(boundary, proof.stopped_prefix(trace(CANCEL + [(14, kind, 1, 0)])))
        for field, value in (('overflow', True), ('active', True), ('count', 1)):
            malformed = {**stopped, field: value}
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.verify_transition_end(boundary, malformed)
        rewritten = deepcopy(stopped)
        rewritten['events'][0][1] += 1
        with self.assertRaises(AssertionError):
            proof.verify_transition_end(boundary, rewritten)


class FixtureTests(unittest.TestCase):
    def fixture(self):
        fixture = object.__new__(proof.ActiveLockFixture)
        fixture.child = Mock(stdin=io.BytesIO(), poll=Mock(return_value=None))
        fixture.record = {'before': status(1), 'ready': {'event': 'ready', 'observed_ns': 1}}
        fixture.config = {}
        fixture.requested = False
        fixture.check_targets = Mock()
        fixture.check_binary = Mock()
        fixture.check_running_binary = Mock()
        fixture.locked = Mock()
        fixture.event = Mock(return_value={'event': 'locked', 'observed_ns': 7_000_000})
        return fixture

    def test_injection_sends_one_lock_only_after_exact_identity_and_fresh_held_gate(self):
        fixture = self.fixture()
        pending = Mock(done=Mock(return_value=False))
        with patch.object(proof, 'power'), \
             patch.object(proof, 'production_status', side_effect=[status(1, held=True), status(2)]), \
             patch.object(proof, 'poll_active', return_value=(trace(ACTIVE), {1: 2_000_000})), \
             patch.object(proof.time, 'monotonic_ns', side_effect=[5_000_000, 6_000_000, 12_000_000]):
            self.assertEqual(fixture.inject(Mock(), trace([ACTIVE[0]]), pending), 1)
        self.assertEqual(fixture.child.stdin.getvalue(), b'LOCK\n')
        fixture.check_targets.assert_called_once()
        fixture.check_binary.assert_called_once()
        fixture.check_running_binary.assert_called_once()
        fixture.event.assert_called_once_with('locked')
        fixture.child.kill.assert_not_called()
        fixture.child.terminate.assert_not_called()

    def test_finished_stale_changed_identity_or_nonheld_drag_never_sends_lock(self):
        for failure in ('finished', 'stale_trace', 'stale_status', 'ready_expired',
                        'not_held', 'target', 'binary', 'running_binary', 'generation'):
            fixture = self.fixture()
            pending = Mock(done=Mock(return_value=failure == 'finished'))
            gate = status(1, held=failure != 'not_held')
            if failure == 'generation':
                gate['input']['lanes'][0]['desktop_generation'] = 2
            if failure in ('target', 'binary', 'running_binary'):
                getattr(fixture, {'target': 'check_targets', 'binary': 'check_binary',
                                  'running_binary': 'check_running_binary'}[failure]).side_effect = AssertionError('identity changed')
            if failure == 'ready_expired':
                fixture.record['ready']['observed_ns'] = -3_000_000_000
            page = trace(ACTIVE)
            now = [5_000_000, 6_000_000, 12_000_000]
            if failure == 'stale_trace':
                now = [299_000_000, 300_000_000]
            if failure == 'stale_status':
                now = [-300_000_000, 6_000_000]
            with self.subTest(failure=failure), patch.object(proof, 'power'), \
                 patch.object(proof, 'production_status', return_value=gate), \
                 patch.object(proof, 'poll_active', return_value=(page, {1: 2_000_000})), \
                 patch.object(proof.time, 'monotonic_ns', side_effect=now), self.assertRaises(AssertionError):
                fixture.inject(Mock(), trace([ACTIVE[0]]), pending)
            self.assertEqual(fixture.child.stdin.getvalue(), b'')
            self.assertFalse(fixture.requested)

    def test_lost_lock_ack_keeps_requested_state_for_graceful_cleanup(self):
        fixture = self.fixture()
        fixture.event.side_effect = TimeoutError('lost acknowledgement')
        with patch.object(proof, 'power'), \
             patch.object(proof, 'production_status', return_value=status(1, held=True)), \
             patch.object(proof, 'poll_active', return_value=(trace(ACTIVE), {1: 2_000_000})), \
             patch.object(proof.time, 'monotonic_ns', side_effect=[5_000_000, 6_000_000]), \
             self.assertRaises(TimeoutError):
            fixture.inject(Mock(), trace([ACTIVE[0]]), Mock(done=Mock(return_value=False)))
        self.assertTrue(fixture.requested)
        self.assertEqual(fixture.child.stdin.getvalue(), b'LOCK\n')
        self.assertNotEqual(fixture.record.get('result'), 'acknowledged')
        self.assertIs(proof.ActiveLockFixture.restore, settled.LockFixture.restore)
        self.assertIs(proof.ActiveLockFixture.check_running_binary, settled.LockFixture.check_running_binary)

    def test_arm_does_not_lock_and_binds_running_helper_after_ready(self):
        fixture = self.fixture()
        fixture.child = None
        child = Mock(pid=99)
        identity = {'pid': 99, 'uid': 1000, 'exe': '/test/session_lock_fixture', 'starttime': '1'}
        fixture.config = {'compositor': {'pid': 50, 'uid': 1000}}
        fixture.event.return_value = {'event': 'ready', 'observed_ns': 1}
        with tempfile.TemporaryDirectory() as directory:
            fixture.args = SimpleNamespace(evidence=Path(directory), lock_fixture=Path(identity['exe']))
            before = status()
            before['input']['lanes'][0]['pointer_focus'] = True
            with patch.object(proof, 'production_status', return_value=before), \
                 patch.object(proof.subprocess, 'Popen', return_value=child) as launch, \
                 patch.object(proof, '_identity', return_value=identity):
                fixture.arm()
            self.assertEqual(json.loads((fixture.args.evidence / 'pre-fault-status.json').read_text()), before)
        self.assertEqual(launch.call_args.args[0], ['/test/session_lock_fixture', '20000', '50'])
        child.stdin.write.assert_not_called()
        fixture.event.assert_called_once_with('ready')
        fixture.check_running_binary.assert_called_once()

    def test_drag_attempt_saved_before_transport_and_unknown_never_replayed(self):
        for outcome in ('partial', 'unknown', 'stale'):
            candidate = plan()['agents'][0]
            prepared = {'target': candidate['target'], 'session': candidate['name'],
                        'prepared_ns': 0, 'arguments': {'from_x': 1}}
            client = Mock()
            client.tool.return_value = action()['response']
            if outcome == 'unknown':
                client.tool.side_effect = TimeoutError('lost drag response')
            saved = []
            attempt = {'outcome': 'unknown', 'replayed': False}
            with patch.object(proof.time, 'monotonic_ns', return_value=proof.MAX_GROUNDING_AGE_NS + 1 if outcome == 'stale' else 1):
                result = proof.drag_once(client, candidate, prepared, attempt,
                    lambda name, value: saved.append((name, deepcopy(value))))
            self.assertEqual(saved[0][1]['outcome'], 'unknown')
            self.assertEqual(result['outcome'], 'response' if outcome == 'partial' else 'unknown')
            self.assertFalse(result['replayed'])
            self.assertEqual(client.tool.call_count, 0 if outcome == 'stale' else 1)
            self.assertEqual(saved[-1][1], result)


class RunTests(unittest.TestCase):
    def test_invalid_plan_retained_without_native_setup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'plan.json'
            path.write_text(json.dumps({**plan(), 'disposable': False}))
            args = SimpleNamespace(plan=path, evidence=root / 'evidence')
            with patch.object(proof, 'ActiveLockFixture') as fixture, patch('builtins.print'):
                self.assertEqual(proof.run(args), 1)
            fixture.assert_not_called()
            report = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(report['result'], 'failed')
            self.assertEqual(report['continuous_isolation_across_transitions'], 'unproven')

    def episode(self, failure=None):
        """Synthetic run ordering only; all native/Driver boundaries are mocked."""
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            path = root / 'plan.json'
            path.write_text(json.dumps(plan()))
            args = SimpleNamespace(plan=path, evidence=root / 'evidence', driver=root / 'driver',
                trace_socket=root / 'trace', foreground_journal=root / 'journal', primary_grab=root / 'grab')
            events = []
            clients = []
            def launch(*_args):
                client = Mock(process=Mock(pid=30 + len(clients) * 10, poll=Mock(return_value=None)), closed=False)
                client.tool.return_value = {'structuredContent': {'screen_width': 1280, 'screen_height': 800,
                    'effect': 'unverifiable', 'route': 'synthetic_events'}}
                clients.append(client)
                events.append('launch_' + str(client.process.pid))
                return client
            def close(client):
                events.append('close_' + str(client.process.pid))
                client.closed = True
                client.process.poll.return_value = 0
            fixture = Mock(config={}, record=record())
            fixture.inject.side_effect = lambda *_args: events.append('LOCK')
            def restore(*, cleanup=False):
                events.append('cleanup_unlock' if cleanup else 'explicit_unlock')
                if failure == 'restore' and not cleanup:
                    raise AssertionError('deadline unlock cannot pass')
                return {'result': 'restored', 'after': status(3)}
            fixture.restore.side_effect = restore
            initial, boundary = trace([ACTIVE[0]]), trace(CANCEL)
            if failure == 'release':
                boundary = trace(ACTIVE + [(8, 'agent_cancel', 1, 0)])
            transition = proof.stopped_prefix(trace(CANCEL + [(14, 'keyboard_focus', 0, 0)]))
            recovery = trace([(0, 'start', 0, 0), (1, 'agent_admitted', 1, 0),
                (2, 'pointer_button', 1, 1), (3, 'pointer_button', 1, 0), (4, 'agent_action_end', 1, 0)])
            trace_client = Mock()
            trace_client.collect.side_effect = [initial, boundary, boundary, transition, initial,
                                                recovery, proof.stopped_prefix(recovery)]
            def exchange(command):
                events.append(command)
                if failure == 'start_ack' and command == 'TRACE_START':
                    trace_client.collect.side_effect = None
                    trace_client.collect.return_value = transition
                    raise TimeoutError('lost trace start acknowledgement')
            trace_client.exchange.side_effect = exchange
            def drag(_client, _spec, _prepared, attempt, save):
                events.append('drag')
                attempt.update(action())
                if failure == 'unknown':
                    attempt.update(outcome='unknown', error='lost response')
                save('drag-action.json', attempt)
                return attempt
            pool = Mock()
            def submit(fn, *args):
                future = Future()
                future.set_result(fn(*args))
                return future
            pool.submit.side_effect = submit
            primary = {'pid': 10, 'address': '0x64', 'cursor': {'x': 30, 'y': 40}}
            baseline = {'held': True, 'clicks': 0, 'keys': 0, 'scroll': 0}
            final = status(3)
            final['input']['lanes'][0]['dispatches'] = 1
            def status_read(*_args):
                if len(clients) == 3:
                    return final
                return status(3 if 'explicit_unlock' in events else 2)
            snapshot = {'window_bounds': {'x': 10, 'y': 20, 'width': 800, 'height': 600},
                        'proof_image': 'fresh.png', 'proof_observation_started_ns': 10}
            mocks = {
                'ActiveLockFixture': Mock(return_value=fixture), 'provenance': Mock(return_value={'files': {}}),
                'DirectMCP': launch, 'connect_trace': Mock(return_value=trace_client),
                'close_owned': close, 'ThreadPoolExecutor': Mock(return_value=pool), 'drag_once': drag,
                'prepare_drag': Mock(return_value={'prepared_ns': 0}), 'settle_locked': Mock(),
                'grounded_snapshot': Mock(return_value=snapshot), 'wm': Mock(return_value=primary),
                'state': Mock(side_effect=lambda *_args: baseline if 'explicit_unlock' in events else {'held': False}),
                'production_status': status_read, 'power': Mock(), 'primary_acknowledgement': Mock(return_value='HELD\n'),
                'wait_for': Mock(), 'require_primary_active': Mock(), 'stop_process': Mock(),
            }
            for name, value in mocks.items():
                stack.enter_context(patch.object(proof, name, value))
            stack.enter_context(patch.object(proof.time, 'monotonic_ns',
                side_effect=lambda: 50 if len(clients) == 3 else 0))
            stack.enter_context(patch.object(proof.subprocess, 'Popen', return_value=Mock()))
            stack.enter_context(patch.object(proof.pointer_grounding, 'read_pixels'))
            stack.enter_context(patch.object(proof.pointer_grounding, 'rows', return_value=[]))
            stack.enter_context(patch.object(proof.pointer_grounding, 'calc_formula_selection', return_value=False))
            stack.enter_context(patch.object(proof.pointer_grounding, 'action', return_value=({'x': 1, 'y': 2}, {})))
            stack.enter_context(patch.object(proof.pointer_grounding, 'verify', return_value={'result': 'verified'}))
            stack.enter_context(patch('builtins.print'))
            result = proof.run(args)
            evidence = {file.name: json.loads(file.read_text()) for file in args.evidence.glob('*.json')}
            return result, evidence, events, clients

    def test_complete_synthetic_episode_restores_before_fresh_visible_recovery(self):
        result, evidence, events, clients = self.episode()
        self.assertEqual(result, 0, evidence.get('result.json'))
        report = evidence['result.json']
        self.assertEqual(report['runtime_pids'], [30, 40, 50])
        self.assertEqual(report['cancellation']['result'], 'verified')
        self.assertEqual(report['recovery']['result'], 'verified')
        self.assertEqual(evidence['recovery-grounding.json']['prepared_ns'], 10)
        self.assertEqual(evidence['recovery-action.json']['prepared_ns'], 10)
        self.assertEqual(report['transition']['raw_primary_analysis']['result'], 'failed')
        self.assertEqual(report['continuous_isolation_across_transitions'], 'unproven')
        self.assertLess(events.index('drag'), events.index('LOCK'))
        self.assertLess(events.index('LOCK'), events.index('close_40'))
        self.assertLess(events.index('close_40'), events.index('explicit_unlock'))
        self.assertLess(events.index('explicit_unlock'), events.index('launch_50'))
        self.assertEqual([c.args[0] for c in clients[-1].tool.call_args_list], ['start_session', 'click'])
        for name in ('transition-trace.json', 'transition-analysis.json', 'cancellation-boundary.json',
                     'recovery-grounding.json', 'recovery-after.json', 'recovery-trace.json', 'restoration.json'):
            self.assertIn(name, evidence)

    def test_unknown_attempt_or_failed_unlock_is_preserved_without_recovery_or_replay(self):
        for failure in ('unknown', 'restore', 'release'):
            result, evidence, events, clients = self.episode(failure)
            with self.subTest(failure=failure):
                self.assertEqual(result, 1)
                self.assertEqual(evidence['result.json']['recovery']['result'], 'unproven')
                self.assertEqual(len(clients), 2)
                self.assertEqual(events.count('drag'), 1)
                self.assertIn('cleanup_unlock', events)
                self.assertIn('drag-action.json', evidence)
                self.assertIn('failed-transition-trace.json', evidence)
                if failure == 'unknown':
                    self.assertEqual(evidence['drag-action.json']['outcome'], 'unknown')

    def test_lost_trace_start_ack_is_stopped_and_retained_without_dispatch(self):
        result, evidence, events, _clients = self.episode('start_ack')
        self.assertEqual(result, 1)
        self.assertNotIn('drag', events)
        self.assertNotIn('LOCK', events)
        self.assertIn('TRACE_STOP', events)
        self.assertIn('failed-transition-trace.json', evidence)


if __name__ == '__main__':
    unittest.main()
