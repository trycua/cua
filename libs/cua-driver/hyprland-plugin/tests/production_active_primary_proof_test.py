"""Portable orchestration/failure oracles; no native client transition is certified."""
from copy import deepcopy
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_active_primary_proof as proof
from production_primary_conflict_proof_test import plan as settled_plan
from production_session_fault_proof_test import ACTIVE, CANCEL, PARTIAL, status, trace


def plan():
    return {**settled_plan(), 'purpose': 'active_primary', 'case': 'active_drag',
        'fault': {'kind': 'primary_hover'}, 'recovery': {'pointer_stages': ['click_a1', 'click_b2']},
        'hover_fixture': {'path': '/test/primary_hover_fixture', 'device': 1, 'inode': 2,
                          'uid': 1000, 'sha256': 'a' * 64, 'source_sha256': 'b' * 64}}


def after_status():
    result = status(1)
    result['input']['lanes'][0]['reserved'] = True
    return result


def boundary():
    return trace(ACTIVE + [(7, 'pointer_focus', 0, 0)] + CANCEL[len(ACTIVE):])


def record():
    target = plan()['agents'][0]['target']
    return {'result': 'observed', 'target': target, 'prefix': trace(ACTIVE), 'lane': 1,
        'gate_status': status(1, held=True), 'after': after_status(),
        'ready': {'event': 'ready', 'x': 0, 'y': 0, 'observed_ns': 1},
        'requested_ns': 6_000_000, 'status_started_ns': 5_000_000,
        'ack': {'event': 'moved', 'x': 100, 'y': 100, 'observed_ns': 11_000_000},
        'point': [100, 100], 'primary_after': {**target, 'cursor': {'x': 100, 'y': 100}},
        'observed_ns': 12_000_000, 'deadline_ns': 20_000_000}


def action():
    response = deepcopy(PARTIAL)
    response['structuredContent']['reason'] = 'primary_target_busy'
    return {'outcome': 'response', 'replayed': False, 'response': response,
            'dispatch_ns': 500_000, 'observed_ns': 11_000_000}


class OracleTests(unittest.TestCase):
    def test_parked_foreground_accepts_new_observation_time_only(self):
        before = {'kind': 'state', 'time': 10, 'clicks': 2, 'keys': '',
                  'scroll': 0, 'motion': 4, 'held': False}
        for timestamp in (10, 20):
            proof.verify_parked_foreground(before, {**before, 'time': timestamp})
        for key, value in (('kind', 'event'), ('time', 9), ('time', True),
                           ('clicks', 3), ('keys', 'a'), ('scroll', 1),
                           ('motion', 5), ('held', True), ('unknown', 0)):
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_parked_foreground(before, {**before, key: value})
        for key in before:
            with self.subTest(missing=key), self.assertRaises(AssertionError):
                proof.verify_parked_foreground(before, {k: v for k, v in before.items() if k != key})

    def verify(self, page=None, fault=None, attempt=None):
        return proof.verify_cancelled(boundary() if page is None else page,
            record() if fault is None else fault, action() if attempt is None else attempt,
            plan()['agents'][0]['target'])

    def test_plan_requires_one_calc_exact_identity_hover_and_dynamic_recovery(self):
        proof.validate_plan(plan())
        for change in ({'purpose': 'primary_conflict'}, {'case': 'initial_refusal'},
                       {'disposable': False}, {'fault': {'kind': 'click'}},
                       {'recovery': {'pointer_stage': 'click_b2'}}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.validate_plan({**plan(), **change})
        for key, value in (('path', '/test/primary_grab'), ('uid', 1001),
                           ('sha256', 'unknown'), ('source_sha256', 'unknown'), ('inode', -1)):
            candidate = plan()
            candidate['hover_fixture'][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.validate_plan(candidate)

    def test_verifies_partial_conflict_release_without_transition_isolation_claim(self):
        result = self.verify()
        self.assertEqual(result['reason'], 'primary_target_busy')
        self.assertEqual(result['result'], 'verified')
        self.assertEqual(result['continuous_primary_isolation'], 'unproven')
        self.assertEqual(result['saved_app_effect'], 'unproven')

    def test_sync_ack_alone_or_wrong_primary_client_cannot_prove_transition(self):
        with self.assertRaisesRegex(AssertionError, 'missing primary focus'):
            self.verify(page=trace(CANCEL))
        for field, value in (('pid', 999), ('window_id', 201), ('cursor', {'x': 101, 'y': 100})):
            fault = record()
            fault['primary_after'][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                self.verify(fault=fault)

    def test_raw_primary_transition_is_never_filtered_or_relabelled_as_isolation(self):
        for kind in ('cursor', 'pointer_focus', 'keyboard_focus', 'pointer_button', 'keyboard_key'):
            page = boundary()
            row = [len(page['events']) + 1, 13_000_000, kind, 100, 100, 0, 0]
            if kind == 'cursor':
                row[3] += 10
            page['events'].append(row)
            page['count'] += 1
            original = deepcopy(page)
            result = proof.transition_evidence(page)
            self.assertEqual(result['raw_primary_analysis']['result'], 'failed')
            self.assertEqual(result['continuous_primary_isolation'], 'unproven')
            self.assertEqual(page, original)
            self.assertEqual(self.verify(page=page)['result'], 'verified')

    def test_incomplete_trace_bad_history_and_preexisting_primary_changes_fail(self):
        for field, value in (('overflow', True), ('hook', False), ('timed_out', True), ('active', False), ('count', 99)):
            page = boundary()
            page[field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                self.verify(page=page)
        page = boundary()
        page['events'][0][1] += 1
        with self.assertRaises(AssertionError):
            self.verify(page=page)
        fault, page = record(), boundary()
        fault['prefix']['events'][4][2] = page['events'][4][2] = 'pointer_focus'
        fault['prefix']['events'][4][5] = page['events'][4][5] = 0
        with self.assertRaisesRegex(AssertionError, 'isolation failed'):
            self.verify(page=page, fault=fault)

    def test_missing_release_repeated_input_sibling_mutation_and_false_completion_fail(self):
        for kind in ('agent_cancel', 'agent_admitted', 'agent_drag_start', 'agent_drag_end',
                     'agent_action_end', 'keyboard_key', 'pointer_axis', 'pointer_motion', 'pointer_button'):
            page = boundary()
            page['events'].append([page['count'] + 1, 11_000_000, kind, 100, 100, 1, 0])
            page['count'] += 1
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                self.verify(page=page)
        for index, field, value in ((7, 5, 2), (7, 6, 1), (6, 5, 2), (7, 2, 'pointer_motion')):
            page = boundary()
            page['events'][index][field] = value
            with self.subTest(index=index, field=field), self.assertRaises(AssertionError):
                self.verify(page=page)

    def test_exact_reason_acknowledged_delivery_and_no_replay_are_mandatory(self):
        for change in ({'outcome': 'unknown'}, {'replayed': True}, {'dispatch_ns': 9_000_000},
                       {'observed_ns': 30_000_000}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.verify(attempt={**action(), **change})
        for field, value in (('reason', 'desktop_changed'), ('reason', 'cancelled'),
                             ('effect', 'refused'), ('route', 'foreground'),
                             ('delivery', {'mode': 'unknown', 'delivered_count': 1}),
                             ('delivery', {'mode': 'background', 'delivered_count': 0})):
            attempt = action()
            attempt['response']['structuredContent'][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                self.verify(attempt=attempt)

    def test_stale_hold_or_fixture_ack_fails(self):
        for change in ({'requested_ns': 300_000_000}, {'status_started_ns': -300_000_000},
                       {'deadline_ns': 10_000_000}, {'observed_ns': 8_000_000},
                       {'result': 'unproven'}, {'target': {'pid': 99, 'window_id': 99}}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.verify(fault={**record(), **change})
        for section, key, value in (('ready', 'observed_ns', -3_000_000_000), ('ack', 'x', 99),
                                    ('ack', 'event', 'ready'), ('ack', 'observed_ns', 1)):
            fault = record()
            fault[section][key] = value
            with self.subTest(section=section, key=key), self.assertRaises(AssertionError):
                self.verify(fault=fault)

    def test_primary_cancel_preserves_generation_and_entire_sibling(self):
        for section, key, value, lane in (
            ('gate_status', 'held_button', 0, 0), ('gate_status', 'held_keys', 1, 0),
            ('after', 'held_button', 272, 0), ('after', 'held_keys', 1, 0),
            ('after', 'reserved', None, 0), ('after', 'pointer_focus', True, 0),
            ('after', 'desktop_generation', 2, 0), ('after', 'epoch', 'changed', 0),
            ('after', 'dispatches', 1, 0), ('after', 'reserved', True, 1),
            ('after', 'desktop_generation', 2, 1)):
            fault = record()
            fault[section]['input']['lanes'][lane][key] = value
            with self.subTest(section=section, key=key, lane=lane), self.assertRaises(AssertionError):
                self.verify(fault=fault)

    def test_immediate_cancel_allows_eof_but_terminal_gate_requires_unreserved(self):
        before = status(1, held=True)
        for reserved in (True, False):
            after = after_status()
            after['input']['lanes'][0]['reserved'] = reserved
            proof.cancelled_status(before, after, 1)
            if reserved:
                with self.assertRaisesRegex(AssertionError, 'retained capacity'):
                    proof.verify_terminal_reservation(before, after, 1)
            else:
                self.assertEqual(proof.verify_terminal_reservation(before, after, 1)['result'], 'verified')
        for field, value in (('reserved', 0), ('reserved', 1), ('pointer_focus', True),
                             ('held_button', 272), ('lease_active', True)):
            after = status(1)
            after['input']['lanes'][0][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError):
                proof.verify_terminal_reservation(before, after, 1)

    def test_terminal_wait_is_bounded_and_checks_every_observed_state(self):
        desktop = Mock()
        desktop.status.side_effect = [after_status(), status(1)]
        result = proof.await_terminal_reservation(desktop, status(1, held=True), 1)
        self.assertTrue(result['verification']['unreserved'])
        self.assertEqual(desktop.status.call_count, 2)
        with patch.object(proof, 'wait_for', side_effect=RuntimeError('timeout')) as wait:
            with self.assertRaisesRegex(RuntimeError, 'timeout'):
                proof.await_terminal_reservation(Mock(), status(1, held=True), 1)
            self.assertEqual(wait.call_args.kwargs['timeout'], 1)
        broken = status(1)
        broken['input']['lanes'][1]['reserved'] = True
        with self.assertRaises(AssertionError):
            proof.await_terminal_reservation(Mock(status=Mock(return_value=broken)), status(1, held=True), 1)

    def test_optional_motion_gate_rejects_invalid_and_unrecorded_displacement(self):
        candidate = plan()
        candidate['fault']['min_motion_px'] = 12
        proof.validate_plan(candidate)
        for value in (True, 0, -1, float('inf'), float('nan'), '12'):
            with self.subTest(value=value), self.assertRaises(AssertionError):
                proof.validate_plan({**candidate, 'fault': {'kind': 'primary_hover', 'min_motion_px': value}})
        fault = record()
        fault.update(min_motion_px=12, gate_first=deepcopy(fault['prefix']))
        with self.assertRaises(AssertionError):
            self.verify(fault=fault)

    def test_two_sample_path_requires_both_exact_observed_movements(self):
        candidate = plan()
        candidate['fault']['motion_path'] = 'two_sample'
        proof.validate_plan(candidate)
        with self.assertRaises(AssertionError):
            proof.validate_plan({**candidate, 'fault': {'kind': 'primary_hover', 'motion_path': 'click'}})
        fault, page = record(), boundary()
        origin = list(fault['prefix']['events'][-1][3:5])
        point = [int(origin[0]) + 100, int(origin[1])]
        midpoint = [int(origin[0]) + 50, int(origin[1])]
        fault.update(motion_path='two_sample', motion_from=origin, point=point,
                     intermediate={'event': 'intermediate', 'x': midpoint[0], 'y': midpoint[1], 'observed_ns': 6_500_000})
        fault['ack'].update(x=point[0], y=point[1])
        fault['primary_after']['cursor'] = dict(zip(('x', 'y'), point))
        n = len(fault['prefix']['events'])
        page['events'][n:n] = [[0, 6_100_000, 'cursor', *midpoint, 0, 0],
                              [0, 6_600_000, 'cursor', *point, 0, 0]]
        for index, row in enumerate(page['events'], 1):
            row[0] = index
        page['count'] = len(page['events'])
        self.assertEqual(self.verify(page=page, fault=fault)['result'], 'verified')
        for mutation in ('missing', 'wrong', 'extra', 'wrong_start', 'wrong_ack'):
            bad_page, bad_fault = deepcopy(page), deepcopy(fault)
            if mutation == 'missing':
                bad_page['events'][n][2] = 'pointer_motion'
            elif mutation == 'wrong':
                bad_page['events'][n][3] += 1
            elif mutation == 'extra':
                bad_page['events'].append([bad_page['count'] + 1, 12_000_000, 'cursor', *point, 0, 0])
                bad_page['count'] += 1
            elif mutation == 'wrong_start':
                bad_fault['motion_from'][0] += 2
            else:
                bad_fault['intermediate']['x'] += 1
            with self.subTest(mutation=mutation), self.assertRaises(AssertionError):
                self.verify(page=bad_page, fault=bad_fault)

    def test_recovery_chooses_effective_stage_from_current_selection(self):
        before = {'snapshot_id': 'new-after-cancellation'}
        for b2, stage in ((True, 'click_a1'), (False, 'click_b2')):
            with patch.object(proof.pointer_grounding, 'rows', return_value=['fresh']), \
                 patch.object(proof.pointer_grounding, 'calc_formula_selection', return_value=b2) as selection:
                self.assertEqual(proof.recovery_stage(before), stage)
                selection.assert_called_once_with(before, ['fresh'], 'B2')

    def test_invalid_plan_writes_failed_evidence_without_native_actions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'plan.json'
            path.write_text(json.dumps({**plan(), 'fault': {'kind': 'click'}}))
            args = SimpleNamespace(plan=path, evidence=root / 'evidence')
            with patch.object(proof, 'ExactDesktop') as desktop, patch.object(proof, 'DirectMCP') as driver, patch('builtins.print'):
                self.assertEqual(proof.run(args), 1)
            desktop.assert_not_called()
            driver.assert_not_called()
            result = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(result['result'], 'failed')
            self.assertEqual(result['cancellation']['result'], 'unproven')

    def test_initial_idle_hover_is_admitted_before_primary_setup_without_input(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'plan.json'
            path.write_text(json.dumps(plan()))
            args = SimpleNamespace(plan=path, evidence=root / 'evidence')
            before = status()
            before['input']['lanes'][0]['pointer_focus'] = True
            desktop = Mock()
            desktop.status.side_effect = lambda **kwargs: proof.clear_status(before, **kwargs)
            desktop.primary.side_effect = RuntimeError('reached primary setup')
            with patch.object(proof, 'ExactDesktop', return_value=desktop), \
                 patch.object(proof, 'app_process_identity'), \
                 patch.object(proof, 'provenance', return_value={'files': {}}), \
                 patch.object(proof, 'DirectMCP') as driver, patch('builtins.print'):
                self.assertEqual(proof.run(args), 1)
            driver.assert_not_called()
            desktop.status.assert_called_once_with(unreserved=True, allow_passive=True)
            self.assertEqual(json.loads((args.evidence / 'initial-status.json').read_text()), before)
            self.assertIn('reached primary setup', str(json.loads((args.evidence / 'result.json').read_text())['error']))

    def test_recovery_grounding_scopes_counter_and_preserves_original_age(self):
        spec = plan()['agents'][0]
        runtime = {'pid': 102, 'directory': str(Path.cwd())}
        previous = {'snapshot_id': 's00000001', 'proof_runtime': {**runtime, 'pid': 101},
            'proof_image': 'previous.png', 'proof_observation_started_ns': 10, 'proof_observation_finished_ns': 20}
        for failure in (None, 'cached', 'same_snapshot', 'same_artifact', 'dead'):
            observed = {**previous, 'proof_runtime': runtime, 'proof_image': 'fresh.png',
                'proof_observation_started_ns': 40, 'proof_observation_finished_ns': 50}
            if failure == 'cached':
                observed.update(proof_observation_started_ns=10, proof_observation_finished_ns=20)
            elif failure == 'same_snapshot':
                observed = dict(previous)
            elif failure == 'same_artifact':
                observed['proof_image'] = previous['proof_image']
            client = Mock(directory=Path.cwd(), process=Mock(pid=102, poll=Mock(return_value=0 if failure == 'dead' else None)))
            with self.subTest(failure=failure), \
                 patch.object(proof, 'grounded_snapshot', return_value=observed), \
                 patch.object(proof.time, 'monotonic_ns', side_effect=[30, 60]), \
                 patch.object(proof, 'recovery_stage', return_value='click_b2'), \
                 patch.object(proof.pointer_grounding, 'read_pixels', return_value='pixels'), \
                 patch.object(proof.pointer_grounding, 'action', return_value=({'x': 1}, {'stage': 'click_b2'})) as action:
                if failure:
                    with self.assertRaises(AssertionError):
                        proof.prepare_recovery(client, spec, previous, ['click_a1', 'click_b2'])
                    action.assert_not_called()
                else:
                    prepared = proof.prepare_recovery(client, spec, previous, ['click_a1', 'click_b2'])
                    self.assertEqual(prepared['prepared_ns'], 40)
                    self.assertIs(prepared['snapshot'], observed)
                    action.assert_called_once_with(observed, 'pixels', 'calc', 'click_b2')
                client.tool.assert_not_called()


class FixtureTests(unittest.TestCase):
    def fixture(self):
        value = object.__new__(proof.HoverFixture)
        value.desktop = Mock(plan=plan())
        value.desktop.guard.return_value = [{'pid': 20, 'at': [10, 20], 'size': [800, 600]}]
        value.desktop.primary.return_value = record()['primary_after']
        value.guard = Mock()
        value.mode = {'width': 1280, 'height': 800}
        value.sent = False
        value.record = record()
        value.child = Mock(stdin=io.BytesIO())
        value.event = Mock(return_value=record()['ack'])
        return value

    def test_exact_move_only_after_fresh_hold_target_and_ready_checks(self):
        fixture = self.fixture()
        pending = Mock(done=Mock(return_value=False))
        with patch.object(proof.time, 'monotonic_ns', side_effect=[6_000_000, 12_000_000]):
            fixture.move([100, 100], plan()['agents'][0]['target'], 1, pending=pending)
        self.assertEqual(fixture.child.stdin.getvalue(), b'MOVE 100 100\n')
        self.assertTrue(fixture.sent)
        self.assertEqual(fixture.record['prepared_ns'], 1)
        fixture.desktop.primary.assert_called_once_with(plan()['agents'][0]['target'])
        fixture.event.assert_called_once_with('moved')

    def test_replay_stale_gate_unheld_or_wrong_target_never_sends(self):
        for failure in ('replay', 'finished', 'stale_trace', 'stale_status', 'stale_ready',
                        'stale_image', 'outside_window', 'target', 'guard'):
            fixture = self.fixture()
            pending = Mock(done=Mock(return_value=failure == 'finished'))
            point, target, prepared_ns = [100, 100], plan()['agents'][0]['target'], 1
            if failure == 'replay': fixture.sent = True
            if failure == 'stale_trace': fixture.record['prefix']['events'][-1][1] = -300_000_000
            if failure == 'stale_status': fixture.record['status_started_ns'] = -300_000_000
            if failure == 'stale_ready': fixture.record['ready']['observed_ns'] = -3_000_000_000
            if failure == 'stale_image': prepared_ns = -6_000_000_000
            if failure == 'outside_window': point = [5, 5]
            if failure == 'target': target = {'pid': 33, 'window_id': 44}
            if failure == 'guard': fixture.guard.side_effect = AssertionError('identity changed')
            with self.subTest(failure=failure), patch.object(proof.time, 'monotonic_ns', return_value=6_000_000), self.assertRaises(AssertionError):
                fixture.move(point, target, prepared_ns, pending=pending)
            self.assertEqual(fixture.child.stdin.getvalue(), b'')

    def test_lost_ack_retains_single_attempt_and_cannot_retry(self):
        fixture = self.fixture()
        fixture.event.side_effect = RuntimeError('EOF')
        with patch.object(proof.time, 'monotonic_ns', return_value=6_000_000), self.assertRaises(RuntimeError):
            fixture.move([100, 100], plan()['agents'][0]['target'], 1)
        self.assertTrue(fixture.sent)
        with self.assertRaises(AssertionError):
            fixture.move([100, 100], plan()['agents'][0]['target'], 1)
        self.assertEqual(fixture.child.stdin.getvalue(), b'MOVE 100 100\n')

    def test_injection_requires_both_current_trace_and_current_held_status(self):
        prepared = {'snapshot': {'window_bounds': {'x': 10, 'y': 20}},
                    'arguments': {'from_x': 90, 'from_y': 80},
                    'target': plan()['agents'][0]['target'], 'prepared_ns': 1}
        for held in (True, False):
            fixture = self.fixture()
            fixture.move = Mock()
            pending = Mock(done=Mock(return_value=False))
            with patch.object(proof, 'poll_active', return_value=(trace(ACTIVE), {1: 2_000_000})), \
                 patch.object(proof, '_hypr', side_effect=[json.dumps(status(1, held=held)), json.dumps(after_status())]), \
                 patch.object(proof.time, 'monotonic_ns', side_effect=[5_000_000, 12_000_000]):
                if held:
                    fixture.inject(Mock(), trace([ACTIVE[0]]), pending, prepared)
                    fixture.move.assert_called_once_with([100, 100], prepared['target'], 1, pending=pending)
                else:
                    with self.assertRaises(AssertionError):
                        fixture.inject(Mock(), trace([ACTIVE[0]]), pending, prepared)
                    fixture.move.assert_not_called()

    def test_injection_uses_optional_motion_gate_on_both_sides_of_status(self):
        fixture = self.fixture()
        fixture.desktop.plan['fault']['min_motion_px'] = 12
        fixture.move = Mock()
        prepared = {'snapshot': {'window_bounds': {'x': 10, 'y': 20}},
                    'arguments': {'from_x': 90, 'from_y': 80},
                    'target': plan()['agents'][0]['target'], 'prepared_ns': 1}
        pending = Mock(done=Mock(return_value=False))
        page = trace(ACTIVE)
        with patch.object(proof, 'poll_fault_active', return_value=(page, {1: 2_000_000})) as gate, \
             patch.object(proof, 'poll_active') as legacy, \
             patch.object(proof, '_hypr', side_effect=[json.dumps(status(1, held=True)), json.dumps(status(1))]), \
             patch.object(proof.time, 'monotonic_ns', side_effect=[5_000_000, 12_000_000]):
            fixture.inject(Mock(), trace([ACTIVE[0]]), pending, prepared)
        self.assertEqual(gate.call_count, 2)
        self.assertEqual([call.args[3] for call in gate.call_args_list], [12, 12])
        self.assertEqual([call.kwargs['timeout'] for call in gate.call_args_list], [1, .25])
        legacy.assert_not_called()
        self.assertEqual(fixture.record['min_motion_px'], 12)
        self.assertEqual(fixture.record['gate_first'], page)

    def test_motion_gate_revalidated_before_sending_fixture_move(self):
        fixture = self.fixture()
        fixture.record['min_motion_px'] = 12
        pending = Mock(done=Mock(return_value=False))
        with patch.object(proof.time, 'monotonic_ns', return_value=6_000_000), \
             patch.object(proof, 'verify_held_gate', side_effect=AssertionError('motion insufficient')) as gate:
            with self.assertRaisesRegex(AssertionError, 'motion insufficient'):
                fixture.move([100, 100], plan()['agents'][0]['target'], 1, pending=pending)
        gate.assert_called_once_with(fixture.record)
        self.assertEqual(fixture.child.stdin.getvalue(), b'')
        self.assertFalse(fixture.sent)

    def test_two_sample_command_records_midpoint_and_keeps_exact_focus_check(self):
        fixture = self.fixture()
        fixture.desktop.plan['fault']['motion_path'] = 'two_sample'
        fixture.event.side_effect = [
            {'event': 'intermediate', 'x': 75, 'y': 100, 'observed_ns': 7_000_000},
            {'event': 'moved', 'x': 100, 'y': 100, 'observed_ns': 11_000_000}]
        with patch.object(proof, '_hypr', return_value='{"x":50,"y":100}'), \
             patch.object(proof.time, 'monotonic_ns', side_effect=[6_000_000, 12_000_000]):
            fixture.move([100, 100], plan()['agents'][0]['target'], 1)
        self.assertEqual(fixture.child.stdin.getvalue(), b'MOVE_FROM 50 100 100 100\n')
        self.assertEqual(fixture.record['motion_from'], [50, 100])
        fixture.desktop.primary.assert_called_once_with(plan()['agents'][0]['target'])
        fixture = self.fixture()
        fixture.desktop.plan['fault']['motion_path'] = 'two_sample'
        with patch.object(proof, '_hypr', return_value='{"x":100,"y":100}'), self.assertRaises(AssertionError):
            fixture.move([100, 100], plan()['agents'][0]['target'], 1)
        self.assertFalse(fixture.sent)
        self.assertEqual(fixture.child.stdin.getvalue(), b'')

    def test_close_checks_graceful_exit_and_finished_ack_without_input(self):
        fixture = self.fixture()
        fixture.sent = True
        fixture.child.poll.return_value = None
        fixture.child.returncode = 0
        fixture.record.pop('finished', None)
        fixture.event.return_value = {'event': 'finished', 'x': 100, 'y': 100, 'observed_ns': 1}
        with patch.object(proof, 'stop_process') as stop:
            fixture.close()
        self.assertTrue(fixture.child.stdin.closed)
        stop.assert_called_once_with(fixture.child)
        self.assertEqual(fixture.record['exit_code'], 0)
        fixture.event.assert_called_once_with('finished')
        fixture.child.returncode = 1
        with patch.object(proof, 'stop_process'), self.assertRaisesRegex(AssertionError, 'controller EOF'):
            fixture.close()

    def test_follow_mouse_and_single_unscaled_monitor_are_readonly_preconditions(self):
        monitor = {'id': 0, 'name': 'Test-1', 'x': 0, 'y': 0, 'width': 1280,
                   'height': 800, 'scale': 1, 'transform': 0, 'dpmsStatus': True}
        for field, value in ((None, None), ('scale', 2), ('x', 10), ('transform', 1), ('dpmsStatus', False)):
            mode = {**monitor, **({field: value} if field else {})}
            with patch.object(proof, '_hypr', side_effect=[json.dumps({'int': 1}), json.dumps([mode])]) as hypr:
                if field:
                    with self.assertRaises(AssertionError): proof.desktop_mode(Mock())
                else:
                    mode = proof.desktop_mode(Mock())
                    self.assertEqual(mode['width'], 1280)
                    self.assertEqual(mode['follow_mouse'], 1)
            self.assertTrue(all('dispatch' not in call.args for call in hypr.call_args_list))
        for value in (0, 2, 3, True, '1'):
            with patch.object(proof, '_hypr', return_value=json.dumps({'int': value})), self.assertRaises(AssertionError):
                proof.desktop_mode(Mock())


if __name__ == '__main__':
    unittest.main()
