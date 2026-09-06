"""Portable adversarial preparation tests; never native lifetime certification."""
from contextlib import ExitStack
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_target_lifetime_proof as proof
from production_primary_conflict_proof_test import plan as base_plan, identity, DELIVERED
from production_session_fault_proof_test import ACTIVE, PARTIAL, status, trace


DESTROYED = ACTIVE + [(8, 'agent_cancel', 1, 0)]


def owned(directory):
    return {'document': {'path': directory + '/cua-smoke-calc.ods', 'device': 1,
            'inode': 2, 'uid': 1000, 'sha256': 'a' * 64}, 'profile': directory + '/calc-profile'}


def plan():
    result = base_plan()
    result.update(purpose='target_lifetime', case='active_drag', fault={'kind': 'destroy', 'signal': 'SIGKILL'})
    result['agents'][0]['owned'] = owned('/test/original')
    fresh = {**deepcopy(result['agents'][0]), 'name': 'fresh', 'target': {'pid': 30, 'window_id': 300},
             'pointer_stage': 'click_b2', 'owned': owned('/test/replacement')}
    result['recovery'] = {'mode': 'prepared_distinct_process', 'agent': fresh, 'identity': identity(30)}
    return result


def statuses():
    before, after = status(held=True), status()
    after['input']['lanes'][0]['reserved'] = True
    for old, new in zip(before['input']['lanes'], after['input']['lanes']):
        for key in ('seat_resources', 'pointer_resources', 'keyboard_resources'):
            old[key], new[key] = 3, 2
    return before, after


def record():
    before, after = statuses()
    return {'result': 'observed', 'target': plan()['agents'][0]['target'], 'lane': 1,
            'prefix': trace(ACTIVE), 'gate_status': before, 'after': after,
            'requested_ns': 6_000_000, 'status_started_ns': 5_000_000,
            'gone': {'pidfd_exited': True, 'window_absent': True, 'observed_ns': 9_000_000},
            'observed_ns': 10_000_000}


def action():
    response = deepcopy(PARTIAL)
    response['structuredContent']['reason'] = 'stale_target'
    return {'outcome': 'response', 'replayed': False, 'response': response,
            'dispatch_ns': 500_000, 'observed_ns': 11_000_000}


class PlanTests(unittest.TestCase):
    def test_exact_disposable_distinct_replacement_only(self):
        proof.validate_plan(plan())
        mutations = [lambda p: p.update(disposable=False),
            lambda p: p['fault'].update(signal='SIGTERM'),
            lambda p: p['recovery'].update(mode='relaunch'),
            lambda p: p['recovery']['agent'].update(pointer_stage='select_range'),
            lambda p: p['recovery']['agent'].update(target=p['agents'][0]['target']),
            lambda p: p['recovery']['identity'].update(pid=20),
            lambda p: p['agents'][0]['owned'].update(profile='/real/profile'),
            lambda p: p['agents'][0]['owned']['document'].update(path='/real/work.ods'),
            lambda p: p['agents'][0].update(drag={'pid': 99}),
            lambda p: p['recovery']['agent'].update(owned=p['agents'][0]['owned'])]
        for mutate in mutations:
            candidate = plan()
            mutate(candidate)
            with self.subTest(candidate=candidate), self.assertRaises(AssertionError):
                proof.validate_plan(candidate)

    def test_saved_output_exact_bytes_identity_and_alias_refusal(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            path = root / 'cua-smoke-calc.ods'
            path.write_bytes(b'existing saved bytes')
            info = path.stat()
            spec = {'owned': {'document': {'path': str(path), 'device': info.st_dev,
                'inode': info.st_ino, 'uid': info.st_uid, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}}}
            self.assertEqual(proof.saved_document(spec), b'existing saved bytes')
            for key, value in (('inode', info.st_ino + 1), ('sha256', '0' * 64)):
                changed = deepcopy(spec)
                changed['owned']['document'][key] = value
                with self.assertRaises(AssertionError):
                    proof.saved_document(changed)
            alias = root / 'alias.ods'
            alias.symlink_to(path)
            spec['owned']['document']['path'] = str(alias)
            with self.assertRaises(AssertionError):
                proof.saved_document(spec)

    def test_calc_requires_identity_and_exact_launch_argv(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            profile = root / 'calc-profile'
            profile.mkdir(mode=0o700)
            spec = {'owned': owned(str(root))}
            expected_identity = {**identity(20), 'uid': os.getuid()}
            argv = [expected_identity['exe'], f'-env:UserInstallation={profile.as_uri()}',
                    '--norestore', '--nologo', '--calc', str(root / 'cua-smoke-calc.ods')]
            for words in (argv, argv + ['/private.ods'], argv[:-1], [*argv[:1], '-env:UserInstallation=file:///real', *argv[2:]]):
                with patch.object(proof, '_identity', return_value=expected_identity), \
                     patch.object(Path, 'read_bytes', return_value=b'\0'.join(x.encode() for x in words) + b'\0'):
                    if words == argv:
                        proof.check_calc(spec, expected_identity)
                    else:
                        with self.assertRaises(AssertionError):
                            proof.check_calc(spec, expected_identity)
            with patch.object(proof, '_identity', return_value={}), self.assertRaises(AssertionError):
                proof.check_calc(spec, expected_identity)


class OracleTests(unittest.TestCase):
    def test_destroyed_surface_requires_no_invented_release(self):
        result = proof.verify_fault(trace(DESTROYED), record(), action())
        self.assertEqual(result['wire_release_events'], 0)
        self.assertEqual(result['destroyed_client_release_ack'], 'unprovable')
        observed_release = DESTROYED + [(9, 'pointer_button', 1, 0)]
        self.assertEqual(proof.verify_fault(trace(observed_release), record(), action())['wire_release_events'], 1)

    def test_unknown_refusal_success_wrong_reason_or_replay_cannot_pass(self):
        mutations = [lambda a: a.update(outcome='unknown'), lambda a: a.update(replayed=True),
            lambda a: a.update(response=DELIVERED),
            lambda a: a['response']['structuredContent'].update(reason='cancelled'),
            lambda a: a['response']['structuredContent']['delivery'].update(delivered_count=0),
            lambda a: a['response']['structuredContent']['delivery'].update(mode='unknown')]
        for mutate in mutations:
            observed = action()
            mutate(observed)
            with self.subTest(action=observed), self.assertRaises(AssertionError):
                proof.verify_fault(trace(DESTROYED), record(), observed)

    def test_raw_trace_rejects_corruption_primary_leaks_other_lane_and_replay(self):
        for field, value in (('hook', False), ('active', False), ('overflow', True), ('timed_out', True), ('count', 0)):
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.verify_fault({**trace(DESTROYED), field: value}, record(), action())
        for kind, lane, value in [('pointer_button', 0, 0), ('keyboard_key', 0, 1), ('pointer_focus', 0, 0),
                ('pointer_axis', 0, 0), ('agent_cancel', 2, 0), ('agent_admitted', 1, 0),
                ('agent_drag_end', 1, 0), ('agent_drag_start', 1, 0), ('agent_action_end', 1, 0),
                ('pointer_motion', 1, 0), ('pointer_button', 1, 1), ('keyboard_key', 1, 1)]:
            with self.subTest(kind=kind, lane=lane), self.assertRaises(AssertionError):
                proof.verify_fault(trace(DESTROYED + [(9, kind, lane, value)]), record(), action())
        for corrupt in ('history', 'warp_return', 'missing_cancel', 'early_cancel'):
            page = trace(DESTROYED)
            if corrupt == 'history':
                page['events'][1][1] += 1
            elif corrupt == 'warp_return':
                page = trace(DESTROYED + [(9, 'cursor', 0, 0), (10, 'cursor', 0, 0)])
                page['events'][-2][3] += 2
            elif corrupt == 'missing_cancel':
                page = trace(ACTIVE)
            else:
                page['events'][-1][1] = 5_000_000
            with self.subTest(corrupt=corrupt), self.assertRaises(AssertionError):
                proof.verify_fault(page, record(), action())

    def test_exit_absence_fresh_gate_and_pruned_state_required(self):
        for mutate in [lambda r: r['gone'].update(pidfd_exited=False),
            lambda r: r['gone'].update(window_absent=False),
            lambda r: r.update(requested_ns=300_000_000),
            lambda r: r.update(status_started_ns=7_000_000),
            lambda r: r['after']['input']['lanes'][0].update(held_button=272),
            lambda r: r['after']['input']['lanes'][0].update(pointer_resources=3),
            lambda r: r['after']['input']['lanes'][1].update(dispatches=1),
            lambda r: r['after']['input']['lanes'][1].update(pointer_resources=4),
            lambda r: r['after']['input']['lanes'][1].update(reserved=True)]:
            observed = record()
            mutate(observed)
            with self.subTest(record=observed), self.assertRaises(AssertionError):
                proof.verify_fault(trace(DESTROYED), observed, action())


class InjectionTests(unittest.TestCase):
    def test_pidfd_binding_revalidates_identity_and_closes_on_failure(self):
        with ExitStack() as stack:
            guard = stack.enter_context(patch.object(proof.TargetLifetime, 'guard',
                side_effect=[None, AssertionError('recycled PID')]))
            stack.enter_context(patch.object(proof.subprocess, 'run', return_value=SimpleNamespace(returncode=0)))
            for name in ('app_process_identity', 'check_calc', 'saved_document'):
                stack.enter_context(patch.object(proof, name))
            opened = stack.enter_context(patch.object(proof.os, 'pidfd_open', create=True, return_value=77))
            closed = stack.enter_context(patch.object(proof.os, 'close'))
            with self.assertRaisesRegex(AssertionError, 'recycled PID'):
                proof.TargetLifetime(plan())
            self.assertEqual(guard.call_count, 2)
            opened.assert_called_once_with(20)
            closed.assert_called_once_with(77)

    def test_pidfd_only_once_and_no_signal_before_all_gates(self):
        for failure in (None, 'identity', 'pending', 'held', 'stale', 'signal_lost', 'exit_timeout'):
            with self.subTest(failure=failure), ExitStack() as stack:
                fault = object.__new__(proof.TargetLifetime)
                fault.fd, fault.sent, fault.record = 77, False, {}
                fault.guard = Mock(side_effect=AssertionError('identity') if failure == 'identity' else None)
                before, after = statuses()
                fault.status = Mock(side_effect=[status() if failure == 'held' else before, after])
                fault.gone = Mock(return_value={'pidfd_exited': True, 'window_absent': True, 'observed_ns': 9_000_000})
                pending = Mock(done=Mock(return_value=failure == 'pending'))
                stack.enter_context(patch.object(proof, 'poll_active', return_value=(trace(ACTIVE), {1: 2})))
                stack.enter_context(patch.object(proof, 'saved_document'))
                fault.spec = plan()['agents'][0]
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[5_000_000,
                    300_000_000 if failure == 'stale' else 6_000_000, 10_000_000]))
                send = stack.enter_context(patch.object(proof.signal, 'pidfd_send_signal', create=True,
                    side_effect=OSError('lost signal result') if failure == 'signal_lost' else None))
                stack.enter_context(patch.object(proof, 'wait_for', side_effect=TimeoutError('exit')
                    if failure == 'exit_timeout' else lambda check, timeout: check()))
                if failure:
                    with self.assertRaises((AssertionError, OSError, TimeoutError)):
                        fault.inject(Mock(), trace(ACTIVE[:1]), pending, Mock())
                else:
                    self.assertEqual(fault.inject(Mock(), trace(ACTIVE[:1]), pending, Mock()), 1)
                    self.assertEqual(fault.record['result'], 'observed')
                if failure in ('identity', 'pending', 'held', 'stale'):
                    send.assert_not_called()
                else:
                    send.assert_called_once_with(77, proof.signal.SIGKILL)
                    with self.assertRaisesRegex(AssertionError, 'one termination'):
                        fault.inject(Mock(), trace(ACTIVE[:1]), pending, Mock())

    def test_gone_requires_pidfd_exit_and_old_address_absence(self):
        for exited, windows in ((False, []), (True, [{'pid': 21, 'address': '0xc8'}]),
                                (True, [{'pid': 20, 'address': '0xff'}]), (True, [])):
            fault = object.__new__(proof.TargetLifetime)
            fault.fd, fault.spec, fault.destroyed = 77, plan()['agents'][0], False
            fault.guard = Mock(return_value=windows)
            with patch.object(proof.select, 'select', return_value=([77] if exited else [], [], [])):
                self.assertEqual(bool(fault.gone()), exited and not windows)


class RecoveryTests(unittest.TestCase):
    def test_one_fresh_distinct_action_unknown_never_replayed(self):
        for failure in (None, 'alive', 'reused_runtime', 'stale', 'unknown', 'bad_effect', 'same_snapshot'):
            with self.subTest(failure=failure), ExitStack() as stack:
                candidate = plan()
                recovered_status = status()
                recovered_status['input']['lanes'][0]['reserved'] = True
                fault = SimpleNamespace(destroyed=True, spec=candidate['agents'][0], fresh=candidate['recovery']['agent'],
                    guard=Mock(), status=Mock(return_value=recovered_status))
                client = Mock(process=Mock(pid=100 if failure == 'reused_runtime' else 101, poll=Mock(return_value=None)))
                observer = Mock(process=Mock(pid=102, poll=Mock(return_value=None)))
                victim = Mock(process=Mock(pid=100, poll=Mock(return_value=None if failure == 'alive' else 0)))
                client.tool.side_effect = [{}, TimeoutError('lost reply') if failure == 'unknown' else DELIVERED]
                before = {'snapshot_id': 1, 'proof_image': 'before.png'}
                after = {'snapshot_id': 1 if failure == 'same_snapshot' else 2, 'proof_image': 'after.png'}
                prepared = {'target': fault.fresh['target'], 'snapshot': before, 'prepared_ns': 100,
                            'arguments': {'x': 20, 'y': 30}, 'oracle': {'stage': 'click_b2'}}
                stack.enter_context(patch.object(proof, 'prepare_drag', return_value=prepared))
                stack.enter_context(patch.object(proof, 'grounded_snapshot', return_value=after))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', return_value=
                    proof.MAX_GROUNDING_AGE_NS + 101 if failure == 'stale' else 101))
                stack.enter_context(patch.object(proof.pointer_grounding, 'read_pixels', return_value=[]))
                stack.enter_context(patch.object(proof.pointer_grounding, 'verify',
                    side_effect=AssertionError('bad effect') if failure == 'bad_effect' else None))
                page = trace(DESTROYED + [(12, 'agent_admitted', 1, 0), (13, 'pointer_button', 1, 1),
                    (14, 'pointer_button', 1, 0), (15, 'agent_action_end', 1, 0)])
                collector, result = Mock(collect=Mock(return_value=page)), {}
                if failure:
                    with self.assertRaises(AssertionError):
                        proof.recover(client, observer, victim, fault, collector, trace(DESTROYED), 1, Mock(), Mock(), result)
                else:
                    proof.recover(client, observer, victim, fault, collector, trace(DESTROYED), 1, Mock(), Mock(), result)
                    self.assertEqual(result['result'], 'verified')
                clicks = [call for call in client.tool.call_args_list if call.args[0] == 'click']
                self.assertEqual(len(clicks), 0 if failure in ('alive', 'reused_runtime', 'stale') else 1)
                if clicks:
                    self.assertEqual(clicks[0].args[1]['pid'], 30)


if __name__ == '__main__':
    unittest.main()
