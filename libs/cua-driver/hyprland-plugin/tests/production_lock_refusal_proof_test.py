"""Portable failure-oracle tests only; no native lock or guest certification."""
from copy import deepcopy
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_lock_refusal_proof as proof
from production_session_fault_proof_test import plan as session_plan, status, trace, REFUSED, retained_status


def plan():
    return {**session_plan(), 'purpose': 'lock_refusal', 'fault': {'kind': 'lock'},
        'lock_fixture': {'path': '/test/session_lock_fixture', 'device': 1, 'inode': 2,
            'uid': 1000, 'sha256': 'b' * 64, 'source_sha256': 'c' * 64}}


def refusal():
    return {'outcome': 'response', 'replayed': False, 'response': deepcopy(REFUSED),
        'prepared_ns': 1_000_000, 'lock_ack_ns': 2_000_000, 'runtime_started_ns': 3_000_000,
        'dispatch_ns': 4_000_000, 'observed_ns': 5_000_000, 'deadline_ns': 10_000_000,
        'before': status(2), 'after': status(2),
        'trace_before': trace([(3, 'start', 0, 0)]), 'trace_after': trace([(3, 'start', 0, 0)])}


class PlanTests(unittest.TestCase):
    def test_lock_preflight_accepts_only_inert_unreserved_hover_before_launch(self):
        for reserved in (False, True):
            with self.subTest(reserved=reserved), tempfile.TemporaryDirectory() as directory:
                fixture = object.__new__(proof.LockFixture)
                fixture.args = SimpleNamespace(evidence=Path(directory), lock_fixture=Path('/test/session_lock_fixture'))
                fixture.config = {'compositor': {'pid': 50}}
                fixture.check_targets = Mock()
                fixture.check_binary = Mock()
                fixture.child = None
                fixture.record = {}
                before = status()
                before['input']['lanes'][1].update(pointer_focus=True, reserved=reserved)
                with patch.object(proof, 'production_status', return_value=before), \
                     patch.object(proof.subprocess, 'Popen', side_effect=RuntimeError('launch reached')) as launch:
                    if reserved:
                        with self.assertRaisesRegex(AssertionError, 'authority'):
                            fixture.lock()
                        launch.assert_not_called()
                    else:
                        with self.assertRaisesRegex(RuntimeError, 'launch reached'):
                            fixture.lock()
                        launch.assert_called_once()
                self.assertEqual(json.loads((fixture.args.evidence / 'pre-fault-status.json').read_text()), before)

    def test_exact_lock_disposable_calc_identity_plan(self):
        proof.validate_plan(plan())
        for change in ({'purpose': 'session_fault'}, {'fault': {'kind': 'dpms'}},
                       {'disposable': False}, {'fault': {'kind': 'lock', 'kill_to_unlock': True}},
                       {'recovery': {'pointer_stage': 'select_range'}}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.validate_plan({**plan(), **change})
        for field, value in (('path', '/test/arbitrary'), ('sha256', 'unknown'),
                             ('source_sha256', 'x' * 64), ('uid', 0), ('inode', True)):
            candidate = plan()
            candidate['lock_fixture'][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.validate_plan(candidate)

    def test_binary_and_source_hash_inode_and_canonical_path_required(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            binary = root / 'session_lock_fixture'
            binary.write_bytes(b'fixture')
            binary.chmod(0o700)
            source = root / 'libs/cua-driver/hyprland-plugin/tests/session_lock_fixture.c'
            source.parent.mkdir(parents=True)
            source.write_bytes(b'fixture source')
            info = binary.stat()
            fixture = object.__new__(proof.LockFixture)
            fixture.args = SimpleNamespace(source=root, lock_fixture=binary)
            fixture.plan = {'lock_fixture': {'path': str(binary), 'device': info.st_dev,
                'inode': info.st_ino, 'uid': info.st_uid,
                'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                'source_sha256': hashlib.sha256(source.read_bytes()).hexdigest()}}
            fixture.check_binary()
            original = deepcopy(fixture.plan)
            for field, value in (('inode', info.st_ino + 1), ('sha256', '0' * 64),
                                 ('source_sha256', '0' * 64), ('path', str(root / 'elsewhere'))):
                fixture.plan = deepcopy(original)
                fixture.plan['lock_fixture'][field] = value
                with self.subTest(field=field), self.assertRaises(AssertionError):
                    fixture.check_binary()

    def test_invalid_plan_preserves_failed_report_before_native_setup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'plan.json'
            path.write_text(json.dumps({**plan(), 'disposable': False}))
            args = SimpleNamespace(plan=path, evidence=root / 'evidence')
            with patch.object(proof, 'LockFixture') as fixture, patch('builtins.print'):
                self.assertEqual(proof.run(args), 1)
            fixture.assert_not_called()
            report = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(report['active_lock_cancellation'], 'unproven')
            self.assertEqual(report['continuous_isolation_across_transitions'], 'unproven')
            self.assertEqual(report['result'], 'failed')

    def test_running_executable_must_match_reviewed_inode_and_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'session_lock_fixture'
            binary.write_bytes(b'reviewed executable')
            info = binary.stat()
            fixture = object.__new__(proof.LockFixture)
            fixture.child = Mock(pid=42, poll=Mock(return_value=None))
            fixture.identity = {'pid': 42, 'uid': os.getuid(), 'exe': str(binary), 'starttime': '1'}
            expected = {'device': info.st_dev, 'inode': info.st_ino, 'uid': info.st_uid,
                        'sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
            for change in ({}, {'inode': info.st_ino + 1}, {'sha256': '0' * 64}):
                fixture.plan = {'lock_fixture': {**expected, **change}}
                with binary.open('rb') as stream, \
                     patch.object(proof.Path, 'open', return_value=stream) as opened, \
                     patch.object(proof, '_identity', return_value=fixture.identity), \
                     self.subTest(change=change):
                    if change:
                        with self.assertRaises(AssertionError):
                            fixture.check_running_binary()
                    else:
                        fixture.check_running_binary()
                    opened.assert_called_once_with('rb')
            fixture.child.poll.return_value = 1
            with self.assertRaises(AssertionError):
                fixture.check_running_binary()


class OracleTests(unittest.TestCase):
    def test_prelock_refresh_is_bounded_and_retains_actual_observations(self):
        spec = {**plan()['agents'][0], 'pointer_stage': 'click_b2'}
        for age in (1_000_000_000, 3_750_000_000, 3_750_000_001, 6_000_000_000):
            with self.subTest(age=age):
                first_ns = 1_000_000_000
                checked_ns = first_ns + age
                refreshed = age > 3_750_000_000
                snapshots = [
                    {'proof_observation_started_ns': first_ns, 'proof_image': 'first.png',
                     'tree': 'complete first tree including late toolbar'},
                    {'proof_observation_started_ns': checked_ns, 'proof_image': 'second.png',
                     'tree': 'complete second tree including late formula table'}]
                saved = {}
                def save(name, value):
                    saved[name] = deepcopy(value)
                # A 3.9s replacement is allowed despite lacking the heuristic margin.
                clocks = [checked_ns, checked_ns, checked_ns + 3_900_000_000,
                          checked_ns + 3_900_000_000]
                with patch.object(proof, 'grounded_snapshot', side_effect=snapshots) as observe, \
                     patch.object(proof.pointer_grounding, 'read_pixels'), \
                     patch.object(proof.pointer_grounding, 'action', return_value=({'x': 1, 'y': 2}, {})), \
                     patch.object(proof.time, 'monotonic_ns', side_effect=clocks):
                    selected = proof.prepare_refusal_click(Mock(), spec, save)
                attempt = 2 if refreshed else 1
                self.assertEqual(observe.call_count, attempt)
                self.assertTrue(all(call.kwargs == {'session': False} for call in observe.call_args_list))
                self.assertEqual(saved['refusal-grounding.json'], selected)
                decision = selected['prelock_decision']
                self.assertEqual(decision['selected_attempt'], attempt)
                self.assertEqual(decision['checked_ns'], checked_ns)
                self.assertEqual(decision['initial_age_ns'], age)
                self.assertEqual(decision['refresh_requested'], refreshed)
                self.assertEqual(decision['reserve_ns'], 1_250_000_000)
                self.assertEqual(decision['max_age_ns'], 5_000_000_000)
                for index in range(attempt):
                    retained = saved[f'refusal-grounding-attempt-{index + 1}.json']
                    self.assertEqual(retained['snapshot'], snapshots[index])
                    self.assertEqual(retained['prepared_ns'], snapshots[index]['proof_observation_started_ns'])
                    self.assertEqual(retained['prelock_decision']['selected_attempt'], attempt)
                if not refreshed:
                    self.assertNotIn('refusal-grounding-attempt-2.json', saved)

    def test_prelock_does_not_retry_observation_or_grounding_failures(self):
        spec = {**plan()['agents'][0], 'pointer_stage': 'click_b2'}
        valid = {'proof_observation_started_ns': 1, 'proof_image': 'first.png'}
        for failing_attempt in (1, 2):
            for phase in ('observation', 'grounding', 'missing', 'invalid', 'future'):
                with self.subTest(attempt=failing_attempt, phase=phase):
                    bad = {'proof_image': 'bad.png'}
                    if phase in ('invalid', 'future'):
                        bad['proof_observation_started_ns'] = False if phase == 'invalid' else 9_000_000_000
                    observations = [valid] * (failing_attempt - 1) + [
                        RuntimeError('observation failed') if phase == 'observation'
                        else valid if phase == 'grounding' else bad]
                    actions = [({}, {})] * (failing_attempt - 1) + [AssertionError('grounding failed')]
                    saved = {}
                    with patch.object(proof, 'grounded_snapshot', side_effect=observations) as observe, \
                         patch.object(proof.pointer_grounding, 'read_pixels'), \
                         patch.object(proof.pointer_grounding, 'action', side_effect=actions), \
                         patch.object(proof.time, 'monotonic_ns', return_value=4_000_000_000), \
                         self.assertRaises((AssertionError, KeyError, RuntimeError)):
                        proof.prepare_refusal_click(Mock(), spec, lambda name, value: saved.update({name: deepcopy(value)}))
                    self.assertEqual(observe.call_count, failing_attempt)
                    self.assertNotIn('refusal-grounding.json', saved)
                    if failing_attempt == 2:
                        self.assertEqual(saved['refusal-grounding-attempt-1.json']['snapshot'], valid)
                        self.assertIsNone(saved['refusal-grounding-attempt-1.json']['prelock_decision']['selected_attempt'])

    def test_expired_replacement_still_fails_actual_click_guard_without_third_observation(self):
        client = Mock()
        observations = [{'prepared_ns': 1, 'snapshot': {'proof_image': 'first.png'}, 'arguments': {}},
                        {'prepared_ns': 4_000_000_000, 'snapshot': {'proof_image': 'second.png'}, 'arguments': {}}]
        with patch.object(proof, 'prepare_click', side_effect=observations) as prepare, \
             patch.object(proof.time, 'monotonic_ns', side_effect=[4_000_000_000, 9_000_000_001]):
            selected = proof.prepare_refusal_click(client, {}, Mock())
        self.assertEqual(prepare.call_count, 2)
        with patch.object(proof.time, 'monotonic_ns', return_value=9_000_000_002), \
             self.assertRaisesRegex(AssertionError, 'grounding expired'):
            proof.click_once(client, {}, {**selected, 'outcome': 'unknown', 'replayed': False}, Mock(), 'attempt.json')
        client.tool.assert_not_called()

    def test_click_grounding_retains_original_observation_timestamp(self):
        spec = {**plan()['agents'][0], 'pointer_stage': 'click_b2'}
        snapshot = {'proof_observation_started_ns': 10, 'proof_image': 'fresh.png'}
        client = Mock()
        with patch.object(proof, 'grounded_snapshot', return_value=snapshot) as observed, \
             patch.object(proof.pointer_grounding, 'read_pixels', return_value='pixels'), \
             patch.object(proof.pointer_grounding, 'action', return_value=({'x': 1, 'y': 2}, {'verified': False})), \
             patch.object(proof.time, 'monotonic_ns', return_value=500):
            record = proof.prepare_click(client, spec, session=False)
        observed.assert_called_once_with(client, spec['target'], spec, session=False)
        self.assertIs(record['snapshot'], snapshot)
        self.assertEqual(record['prepared_ns'], 10)
        # A slow observation cannot acquire a new five-second budget.
        with patch.object(proof.time, 'monotonic_ns', return_value=proof.MAX_GROUNDING_AGE_NS + 11), \
             self.assertRaisesRegex(AssertionError, 'grounding expired'):
            proof.click_once(client, record['arguments'],
                {**record, 'outcome': 'unknown', 'replayed': False}, Mock(), 'attempt.json')
        client.tool.assert_not_called()

    def test_click_grounding_rejects_missing_invalid_or_future_observation_time(self):
        spec = {**plan()['agents'][0], 'pointer_stage': 'click_b2'}
        for timestamp in (None, False, 0, -1, 501):
            snapshot = {'proof_image': 'fresh.png'}
            if timestamp is not None:
                snapshot['proof_observation_started_ns'] = timestamp
            with self.subTest(timestamp=timestamp), \
                 patch.object(proof, 'grounded_snapshot', return_value=snapshot), \
                 patch.object(proof.time, 'monotonic_ns', return_value=500), \
                 self.assertRaises((AssertionError, KeyError)):
                proof.prepare_click(Mock(), spec)

    def test_click_attempt_is_saved_on_transport_failure_without_replay(self):
        for fails in (False, True):
            client = Mock()
            client.tool.side_effect = TimeoutError('unknown delivery') if fails else None
            client.tool.return_value = deepcopy(REFUSED)
            record = {'outcome': 'unknown', 'replayed': False, 'prepared_ns': 1, 'dispatch_ns': 2}
            saved = []
            def save(name, value):
                saved.append((name, deepcopy(value)))
            with self.subTest(fails=fails), patch.object(proof.time, 'monotonic_ns', return_value=3):
                if fails:
                    with self.assertRaises(TimeoutError):
                        proof.click_once(client, {'x': 10, 'y': 20}, record, save, 'attempt.json')
                else:
                    self.assertEqual(proof.click_once(client, {'x': 10, 'y': 20}, record, save, 'attempt.json'), REFUSED)
            client.tool.assert_called_once_with('click', {'x': 10, 'y': 20})
            self.assertEqual(saved[0][1]['outcome'], 'unknown')
            self.assertEqual(saved[-1][1]['outcome'], 'unknown' if fails else 'response')
            self.assertEqual(saved[-1][1]['observed_ns'], 3)
            self.assertFalse(saved[-1][1]['replayed'])

    def test_saving_evidence_cannot_allow_expired_grounding_to_dispatch(self):
        client = Mock()
        record = {'outcome': 'unknown', 'replayed': False, 'prepared_ns': 1}
        with patch.object(proof.time, 'monotonic_ns', return_value=proof.MAX_GROUNDING_AGE_NS + 2), \
             self.assertRaisesRegex(AssertionError, 'grounding expired'):
            proof.click_once(client, {}, record, Mock(), 'attempt.json')
        client.tool.assert_not_called()

    def test_new_runtime_allows_reaped_refusal_but_rejects_reuse_or_live_closed_process(self):
        def runtime(pid, closed=False, exited=False):
            return SimpleNamespace(closed=closed, process=Mock(pid=pid, poll=Mock(return_value=0 if exited else None)))
        proof.verify_runtimes([runtime(10), runtime(20, closed=True, exited=True), runtime(30)])
        for clients in ([runtime(10), runtime(10)], [runtime(10, closed=True)], [runtime(10, exited=True)]):
            with self.subTest(clients=clients), self.assertRaises(AssertionError):
                proof.verify_runtimes(clients)

    def test_exact_refusal_without_synthetic_input_or_dispatch(self):
        self.assertEqual(proof.verify_refusal(refusal())['result'], 'verified')
        for change in ({'outcome': 'unknown'}, {'replayed': True},
                       {'response': {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'primary_target_busy'}}},
                       {'runtime_started_ns': 1}, {'dispatch_ns': 1}, {'prepared_ns': -6_000_000_000},
                       {'observed_ns': 11_000_000}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.verify_refusal({**refusal(), **change})
        for key, value in (('dispatches', 1), ('desktop_generation', 3), ('epoch', 'changed'),
                           ('held_button', 272), ('held_keys', 1), ('pointer_focus', True),
                           ('lease_active', True), ('reserved', True)):
            candidate = refusal()
            candidate['after']['input']['lanes'][0][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_refusal(candidate)

    def test_unfiltered_strict_trace_rejects_primary_and_synthetic_events(self):
        for lane, kind in ((0, 'pointer_focus'), (0, 'keyboard_focus'), (0, 'pointer_button'),
                           (0, 'keyboard_key'), (1, 'agent_admitted'), (2, 'pointer_enter'),
                           (1, 'agent_cancel'), (1, 'pointer_button')):
            candidate = refusal()
            candidate['trace_after'] = trace([(3, 'start', 0, 0), (4, kind, lane, 0)])
            with self.subTest(lane=lane, kind=kind), self.assertRaises(AssertionError):
                proof.verify_refusal(candidate)
        for field in ('overflow', 'timed_out'):
            candidate = refusal()
            candidate['trace_after'][field] = True
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.verify_refusal(candidate)

    def test_each_lock_transition_advances_generation_without_dispatch(self):
        proof.stable_status(status(1), status(2), advanced=True)
        passive = status(1)
        passive['input']['lanes'][0]['pointer_focus'] = True
        proof.stable_status(passive, status(2), advanced=True)
        with self.assertRaises(AssertionError):
            proof.stable_status(passive, passive)
        after = status(2)
        after['input']['lanes'][0]['pointer_focus'] = True
        with self.assertRaises(AssertionError):
            proof.stable_status(passive, after, advanced=True)
        passive['input']['lanes'][0]['reserved'] = True
        with self.assertRaises(AssertionError):
            proof.stable_status(passive, status(2), advanced=True)
        for after in (status(1), status(0)):
            with self.assertRaises(AssertionError):
                proof.stable_status(status(1), after, advanced=True)
        replacement = status(2)
        replacement['input']['lanes'][1]['epoch'] = 'replaced'
        with self.assertRaises(AssertionError):
            proof.stable_status(status(1), replacement, advanced=True)

    def test_stopped_refusal_trace_must_preserve_the_verified_prefix(self):
        prefix = refusal()['trace_after']
        stopped = proof.stopped_prefix(prefix)
        self.assertEqual(proof.verify_refusal_cleanup(prefix, stopped)['result'], 'passed')
        replacement = deepcopy(stopped)
        replacement['events'][0][1] -= 1
        with self.assertRaisesRegex(AssertionError, 'history changed'):
            proof.verify_refusal_cleanup(prefix, replacement)
        synthetic = proof.stopped_prefix(trace([(3, 'start', 0, 0), (4, 'agent_admitted', 1, 0)]))
        with self.assertRaisesRegex(AssertionError, 'synthetic events'):
            proof.verify_refusal_cleanup(prefix, synthetic)


class FixtureTests(unittest.TestCase):
    def test_lost_trace_start_ack_still_stops_and_preserves_trace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'plan.json'
            path.write_text(json.dumps(plan()))
            args = SimpleNamespace(plan=path, evidence=root / 'evidence', driver=root / 'driver',
                                   trace_socket=root / 'trace', foreground_journal=root / 'journal')
            fixture = Mock(config={}, deadline_ns=30, record={'after': status(2),
                           'ack': {'event': 'locked', 'observed_ns': 10}})
            client = SimpleNamespace(process=Mock(pid=42, poll=Mock(return_value=None)), closed=False)
            trace_client = Mock()
            def exchange(command):
                if command == 'TRACE_START':
                    raise TimeoutError('lost start acknowledgement')
                return {'ok': True}
            trace_client.exchange.side_effect = exchange
            trace_client.collect.return_value = proof.stopped_prefix(refusal()['trace_after'])
            order = []
            fixture.lock.side_effect = lambda: order.append('lock')
            def observe(*args, **kwargs):
                order.append('observe')
                return {'proof_image': f'fresh-{len(order)}.png', 'proof_observation_started_ns': 1}
            with patch.object(proof, 'LockFixture', return_value=fixture), \
                 patch.object(proof, 'provenance', return_value={'files': {}}), \
                 patch.object(proof, 'DirectMCP', return_value=client), \
                 patch.object(proof, 'connect_trace', return_value=trace_client), \
                 patch.object(proof, 'state', return_value={'held': False}), \
                 patch.object(proof, 'wm', return_value={}), \
                 patch.object(proof, 'production_status', return_value=status(2)), \
                 patch.object(proof, 'grounded_snapshot', side_effect=observe), \
                 patch.object(proof.pointer_grounding, 'read_pixels'), \
                 patch.object(proof.pointer_grounding, 'action', return_value=({'x': 1, 'y': 1}, {})), \
                 patch.object(proof.time, 'monotonic_ns', return_value=4_000_000_000), \
                 patch.object(proof, 'settle_locked'), patch.object(proof, 'close_owned'), \
                 patch('builtins.print'):
                self.assertEqual(proof.run(args), 1)
            self.assertEqual([call.args[0] for call in trace_client.exchange.call_args_list], ['TRACE_START', 'TRACE_STOP'])
            self.assertEqual(order, ['observe', 'observe', 'lock'])
            self.assertEqual(json.loads((args.evidence / 'refusal-grounding.json').read_text())['attempt'], 2)
            self.assertTrue((args.evidence / 'failed-phase-trace.json').is_file())
            fixture.restore.assert_called_once_with(cleanup=True)

    def test_settling_restarts_on_primary_transition_and_rejects_generation_change(self):
        fixture = Mock(record={'after': status(2)}, config={})
        def sample_until_ready(predicate, timeout):
            self.assertIsNone(predicate())
            self.assertIsNone(predicate())
            return predicate()
        with patch.object(proof, 'wm', side_effect=[{'pid': 10}, {'pid': None}, {'pid': None}]), \
             patch.object(proof, 'production_status', return_value=status(2)), \
             patch.object(proof.time, 'monotonic_ns', side_effect=[1, 100_000_001, 200_000_001]), \
             patch.object(proof, 'wait_for', side_effect=sample_until_ready):
            self.assertEqual(proof.settle_locked(fixture)['primary'], {'pid': None})
        with patch.object(proof, 'wm', return_value={}), \
             patch.object(proof, 'production_status', return_value=status(3)), \
             patch.object(proof, 'wait_for', side_effect=lambda fn, timeout: fn()), \
             self.assertRaises(AssertionError):
            proof.settle_locked(fixture)

    def fixture(self):
        fixture = object.__new__(proof.LockFixture)
        fixture.child = Mock(stdin=io.BytesIO(), stdout=Mock(), wait=Mock(return_value=0))
        fixture.events = [{'event': 'locked', 'observed_ns': 10}]
        fixture.record = {'after': status(2)}
        fixture.requested, fixture.restored = True, False
        fixture.deadline_ns = 100
        fixture.config = {}
        fixture.locked = Mock()
        return fixture

    def test_restore_requires_protocol_ack_and_successful_exit_never_kill(self):
        fixture = self.fixture()
        def acknowledgement():
            event = {'event': 'unlocked', 'observed_ns': 50}
            fixture.events.append(event)
            return event
        fixture.read_event = acknowledgement
        with patch.object(proof, 'wait_for', side_effect=lambda fn, timeout: fn()), \
             patch.object(proof, 'guard_guest'), patch.object(proof, 'production_status', return_value=status(3)), \
             patch.object(proof.time, 'monotonic_ns', return_value=40):
            result = fixture.restore()
        self.assertTrue(fixture.child.stdin.closed)
        self.assertEqual(result['result'], 'restored')
        fixture.child.kill.assert_not_called()
        fixture.child.terminate.assert_not_called()

    def test_missing_ack_or_deadline_unlock_cannot_pass(self):
        for failure in ('missing_ack', 'early_ack', 'deadline', 'bad_exit'):
            fixture = self.fixture()
            event = {'event': 'unlocked', 'observed_ns': 30 if failure == 'early_ack' else 110 if failure == 'deadline' else 50}
            def acknowledgement():
                fixture.events.append(event)
                return event
            fixture.read_event = acknowledgement
            if failure == 'bad_exit':
                fixture.child.wait.return_value = 1
            with self.subTest(failure=failure), \
                 patch.object(proof, 'wait_for', side_effect=TimeoutError('no ack') if failure == 'missing_ack' else lambda fn, timeout: fn()), \
                 patch.object(proof, 'guard_guest'), patch.object(proof, 'production_status', return_value=status(3)), \
                 patch.object(proof.time, 'monotonic_ns', return_value=40), \
                self.assertRaises((AssertionError, TimeoutError)):
                fixture.restore()
            self.assertEqual(fixture.record['restoration']['result'], 'unproven')
            fixture.child.kill.assert_not_called()
            fixture.child.terminate.assert_not_called()
            fixture.child.wait.assert_called_once()

    def test_failed_lock_guard_still_closes_stdin_and_reaps(self):
        fixture = self.fixture()
        fixture.locked.side_effect = AssertionError('already unlocked')
        with patch.object(proof.time, 'monotonic_ns', return_value=40), \
             self.assertRaisesRegex(AssertionError, 'already unlocked'):
            fixture.restore()
        self.assertTrue(fixture.child.stdin.closed)
        fixture.child.wait.assert_called_once()
        fixture.child.kill.assert_not_called()
        fixture.child.terminate.assert_not_called()

    def test_cleanup_waits_for_independent_deadline_without_signaling(self):
        fixture = self.fixture()
        fixture.deadline_ns = 20_000_000_000
        fixture.read_event = Mock()
        fixture.child.wait.side_effect = subprocess.TimeoutExpired('session_lock_fixture', 22)
        with patch.object(proof, 'wait_for', side_effect=TimeoutError('missing ack')), \
             patch.object(proof.time, 'monotonic_ns', return_value=1_000_000_000), \
             self.assertRaises(subprocess.TimeoutExpired):
            fixture.restore(cleanup=True)
        self.assertTrue(fixture.child.stdin.closed)
        fixture.child.wait.assert_called_once_with(timeout=22)
        fixture.child.kill.assert_not_called()
        fixture.child.terminate.assert_not_called()
        self.assertFalse(fixture.restored)

    def test_event_reader_retains_protocol_evidence_and_rejects_bad_clock(self):
        fixture = self.fixture()
        fixture.events = []
        fixture.buffer = b'{"event":"ready","observed_ns":10}\n'
        with patch.object(proof.time, 'monotonic_ns', return_value=20):
            self.assertEqual(fixture.read_event()['event'], 'ready')
            fixture.buffer = b'{"event":"locked","observed_ns":30}\n'
            with self.assertRaises(AssertionError):
                fixture.read_event()
        self.assertEqual(len(fixture.events), 1)

    def test_partial_event_never_blocks_on_readline(self):
        fixture = self.fixture()
        fixture.events, fixture.buffer = [], b''
        with patch.object(proof.select, 'select', side_effect=[([fixture.child.stdout], [], []), ([], [], [])]), \
             patch.object(proof.os, 'read', return_value=b'{"event":"locked"'):
            self.assertIsNone(fixture.read_event())
            self.assertIsNone(fixture.read_event())
        self.assertEqual(fixture.events, [])
        fixture.child.stdout.readline.assert_not_called()


class RetainedPointerTests(unittest.TestCase):
    def test_settled_lock_accepts_only_cleanup_option_not_a_motion_claim(self):
        value = plan()
        value['fault']['pointer_cleanup'] = 'retained_inert'
        proof.validate_plan(value)
        for change in ({'pointer_cleanup': 'unknown'}, {'min_motion_px': 12}, {'kill_to_unlock': True}):
            bad = deepcopy(value)
            bad['fault'].update(change)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.validate_plan(bad)

    def test_stable_refusal_and_restoration_preserve_both_lane_presence(self):
        for lane in (1, 2):
            before, after = retained_status(1, lane), retained_status(2, lane)
            proof.stable_status(before, after, advanced=True, policy='retained_inert')
            proof.stable_status(after, deepcopy(after), policy='retained_inert')
            with self.assertRaises(AssertionError):
                proof.stable_status(before, after, advanced=True)
            for index, key, value in ((lane - 1, 'pointer_focus', False), (2 - lane, 'pointer_focus', True),
                                     (lane - 1, 'reserved', True), (lane - 1, 'held_button', 272),
                                     (lane - 1, 'held_keys', 1), (lane - 1, 'drag_active', True),
                                     (lane - 1, 'lease_active', True), (lane - 1, 'keyboard_focus', True),
                                     (lane - 1, 'dispatches', 1), (lane - 1, 'epoch', 'different'),
                                     (lane - 1, 'desktop_generation', 1)):
                bad = deepcopy(after)
                bad['input']['lanes'][index][key] = value
                with self.subTest(lane=lane, key=key), self.assertRaises(AssertionError):
                    proof.stable_status(before, bad, advanced=True, policy='retained_inert')
            bad_before = deepcopy(before)
            bad_before['input']['lanes'][lane - 1]['reserved'] = True
            with self.assertRaises(AssertionError):
                proof.stable_status(bad_before, after, advanced=True, policy='retained_inert')

    def test_refusal_does_not_own_input_or_dispatch_with_retained_hover(self):
        value = refusal()
        value.update(pointer_cleanup='retained_inert', before=retained_status(), after=retained_status())
        self.assertEqual(proof.verify_refusal(value)['result'], 'verified')
        for side, key, change in (('before', 'reserved', True), ('after', 'pointer_focus', False),
                                   ('after', 'dispatches', 1), ('after', 'held_keys', 1)):
            bad = deepcopy(value)
            bad[side]['input']['lanes'][0][key] = change
            with self.subTest(side=side, key=key), self.assertRaises(AssertionError):
                proof.verify_refusal(bad)

    def test_unlock_continuity_keeps_raw_primary_failure_and_rejects_synthetic_activity(self):
        initial = trace([(0, 'start', 0, 0)])
        stopped = proof.stopped_prefix(trace([(0, 'start', 0, 0), (1, 'keyboard_focus', 0, 0)]))
        result = proof.verify_inert_transition(initial, stopped)
        self.assertEqual(result['continuous_primary_isolation'], 'unproven')
        self.assertEqual(result['raw_primary_analysis'], proof.analyze(stopped))
        self.assertNotEqual(result['raw_primary_analysis']['result'], 'passed')
        for kind in ('pointer_leave', 'pointer_enter', 'pointer_motion', 'agent_admitted', 'pointer_button'):
            bad = proof.stopped_prefix(trace([(0, 'start', 0, 0), (1, kind, 1, 0)]))
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                proof.verify_inert_transition(initial, bad)
        bad = deepcopy(stopped)
        bad['overflow'] = True
        with self.assertRaises(AssertionError):
            proof.verify_inert_transition(initial, bad)

    def test_shared_settling_and_graceful_restore_thread_policy_without_deadline_change(self):
        settling = Mock(config={'pointer_cleanup': 'retained_inert'}, record={'after': retained_status()})
        def sample_twice(fn, timeout):
            self.assertEqual(timeout, 2)
            self.assertIsNone(fn())
            return fn()
        with patch.object(proof, 'wm', return_value={}), \
             patch.object(proof, 'production_status', return_value=retained_status()), \
             patch.object(proof.time, 'monotonic_ns', side_effect=[1, 100_000_001]), \
             patch.object(proof, 'wait_for', side_effect=sample_twice):
            self.assertEqual(proof.settle_locked(settling)['status'], retained_status())
        fixture = FixtureTests().fixture()
        fixture.config['pointer_cleanup'] = 'retained_inert'
        fixture.record['after'] = retained_status(2)
        fixture.events = [{'event': 'locked', 'observed_ns': 2}]
        fixture.read_event = Mock(return_value={'event': 'unlocked', 'observed_ns': 4})
        fixture.child.wait.return_value = 0
        with patch.object(proof, 'guard_guest'), \
             patch.object(proof, 'production_status', return_value=retained_status(3)), \
             patch.object(proof, 'wait_for', side_effect=lambda fn, timeout: fn()), \
             patch.object(proof.time, 'monotonic_ns', return_value=3):
            # Model the event reader's append as in the real helper.
            def unlock():
                event = {'event': 'unlocked', 'observed_ns': 4}
                fixture.events.append(event)
                return event
            fixture.read_event.side_effect = unlock
            self.assertEqual(fixture.restore()['result'], 'restored')
        fixture.child.kill.assert_not_called()
        fixture.child.terminate.assert_not_called()
        self.assertEqual(proof.LOCK_MS, 20000)


if __name__ == '__main__':
    unittest.main()
