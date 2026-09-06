"""Portable adversarial orchestration tests; no native desktop is certified."""
from contextlib import ExitStack
from copy import deepcopy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_primary_conflict_proof as proof


BOUNDS = {'x': 10, 'y': 20, 'width': 800, 'height': 600}
REFUSED = {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'primary_target_busy'}}
DELIVERED = {'structuredContent': {'effect': 'unverifiable', 'route': 'synthetic_events',
                                 'delivery': {'mode': 'background'}}}


def identity(pid, name='app'):
    return {'pid': pid, 'uid': 1000, 'starttime': '123', 'exe': '/usr/bin/' + name}


def plan(app='calc'):
    return {'purpose': 'primary_conflict', 'case': 'initial_refusal', 'disposable': True,
        'vm': {'machine_id': 'a' * 32, 'boot_id': 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'},
        'compositor': {**identity(50, 'Hyprland'), 'instance': 'test_1'},
        'processes': {'target': identity(20), 'foreground': identity(10)},
        'foreground': {'pid': 10, 'window_id': 100}, 'primary_point': [20, 20], 'package_versions': {},
        'agents': [{'app': app, 'name': 'primary-conflict', 'target': {'pid': 20, 'window_id': 200},
            'bounds': dict(BOUNDS), 'pointer_stage': 'select_range' if app == 'calc' else 'move_rectangle', 'drag': {}}],
        'recovery': {'pointer_stage': 'click_b2' if app == 'calc' else 'scroll_down'}}


def trace(events=(), active=True):
    rows = [(0, 'start', 0, 0), *events]
    if not active:
        rows += [(20, 'stop', 0, 0)]
    return {'hook': True, 'active': active, 'overflow': False, 'timed_out': False, 'count': len(rows),
        'events': [[i + 1, timestamp, kind, 30, 40, lane, value]
                   for i, (timestamp, kind, lane, value) in enumerate(rows)]}


RECOVERY = [(1, 'agent_admitted', 1, 0), (2, 'pointer_button', 1, 1),
            (3, 'pointer_button', 1, 0), (4, 'agent_action_end', 1, 0)]


def status():
    return {'configured': True, 'transport': {'ready': True}, 'input': {
        'protocol': 3, 'test_only': False, 'transport_ready': True,
        'seat_lifetime': 'compositor', 'upgrade': 'desktop_restart', 'lanes': [
            {'lane': lane, 'held_button': 0, 'held_keys': 0, 'drag_active': False,
             'lease_active': False, 'keyboard_focus': False, 'pointer_focus': False, 'reserved': False}
            for lane in (0, 1)]}}


def client(pid):
    return Mock(process=Mock(pid=pid, poll=Mock(return_value=None)))


class OracleTests(unittest.TestCase):
    def test_exact_refusal_and_zero_synthetic_events_required(self):
        proof.verify_refusal(trace(), trace(), REFUSED)
        for response in (DELIVERED, {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'policy_denied'}},
                         {**REFUSED, 'structuredContent': {**REFUSED['structuredContent'], 'delivery': {'mode': 'unknown'}}}):
            with self.subTest(response=response), self.assertRaises(AssertionError):
                proof.verify_refusal(trace(), trace(), response)
        for kind in ('agent_admitted', 'agent_cancel', 'pointer_enter', 'pointer_leave', 'pointer_button',
                     'pointer_motion', 'keyboard_key', 'agent_drag_start', 'agent_action_end'):
            for lane in (1, 2):
                with self.subTest(kind=kind, lane=lane), self.assertRaises(AssertionError):
                    proof.verify_refusal(trace(), trace([(1, kind, lane, 0)]), REFUSED)
                # A pre-call synthetic event must not disappear into a baseline.
                before = trace([(1, kind, lane, 0)])
                with self.assertRaises(AssertionError):
                    proof.verify_refusal(before, before, REFUSED)

    def test_incomplete_or_rewritten_trace_cannot_pass(self):
        for key, value in (('hook', False), ('active', False), ('overflow', True), ('timed_out', True), ('count', 9)):
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_refusal(trace(), {**trace(), key: value}, REFUSED)
        changed = trace()
        changed['events'][0][3] += 1
        with self.assertRaisesRegex(AssertionError, 'history'):
            proof.verify_refusal(trace(), changed, REFUSED)

    def test_trace_oracle_never_ignores_primary_input_or_warp_and_return(self):
        for kind in ('pointer_focus', 'keyboard_focus', 'pointer_button', 'keyboard_key', 'pointer_axis'):
            self.assertEqual(proof.analyze(trace([(1, kind, 0, 0)], active=False))['result'], 'failed')
        warped = trace([(1, 'cursor', 0, 0), (2, 'cursor', 0, 0)], active=False)
        warped['events'][1][3] += 20
        self.assertEqual(proof.analyze(warped)['result'], 'failed')

    def test_current_held_state_and_preexisting_reservations_are_checked(self):
        proof.clear_status(status(), unreserved=True)
        for field, value in (('held_button', 272), ('held_keys', 1), ('held_keys', False),
                             ('drag_active', True), ('lease_active', True), ('keyboard_focus', True),
                             ('reserved', True), ('pointer_focus', True)):
            row = status()
            row['input']['lanes'][1][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.clear_status(row, unreserved=True)
        passive = status()
        passive['input']['lanes'][0].update(pointer_focus=True, reserved=True)
        proof.clear_status(passive)  # Completed input may retain passive focus until runtime close.


class OwnershipTests(unittest.TestCase):
    def test_trace_requires_exact_socket_peer_and_unowned_trace(self):
        path = Path('/run/user/1000/hypr/test_1/cua-input-v3.sock')
        for failure in (None, 'peer', 'protocol', 'active', 'owner', 'alias'):
            desktop = object.__new__(proof.ExactDesktop)
            desktop.instance, desktop.compositor = 'test_1', identity(50, 'Hyprland')
            desktop.guard = Mock()
            tracer = Mock(hello={'protocol': 2 if failure == 'protocol' else 3})
            tracer.collect.return_value = {'active': failure == 'active'}
            tracer.socket.getsockopt.return_value = proof.struct.pack('3i', 51 if failure == 'peer' else 50, 1000, 1000)
            with self.subTest(failure=failure), ExitStack() as stack:
                stack.enter_context(patch.object(proof.socket, 'SO_PEERCRED', 17, create=True))
                stack.enter_context(patch.dict(proof.os.environ, {'XDG_RUNTIME_DIR': '/run/user/1000'}))
                stack.enter_context(patch.object(Path, 'resolve', return_value=Path('/different') if failure == 'alias' else path))
                stack.enter_context(patch.object(Path, 'lstat', return_value=SimpleNamespace(
                    st_mode=proof.stat.S_IFSOCK | 0o600, st_uid=1001 if failure == 'owner' else 1000)))
                factory = stack.enter_context(patch.object(proof, 'Trace', return_value=tracer))
                if failure:
                    with self.assertRaises(AssertionError):
                        desktop.trace(path)
                    if failure in ('peer', 'protocol', 'active'):
                        tracer.close.assert_called_once()
                    else:
                        factory.assert_not_called()
                else:
                    self.assertIs(desktop.trace(path), tracer)
                    tracer.exchange.assert_not_called()

    def test_plan_requires_exact_vm_processes_and_narrow_case(self):
        for app in ('calc', 'inkscape'):
            proof.validate_plan(plan(app))
        updates = ({'case': 'active_drag'}, {'disposable': False}, {'fault': {'kind': 'primary'}},
                   {'recovery': {'pointer_stage': 'select_range'}},
                   {'vm': {'machine_id': 'unknown', 'boot_id': 'unknown'}})
        for update in updates:
            with self.subTest(update=update), self.assertRaises(AssertionError):
                proof.validate_plan({**plan(), **update})
        for owner, key, value in (('target', 'pid', 21), ('target', 'uid', 1001),
                                  ('foreground', 'starttime', ''), ('foreground', 'exe', 'relative')):
            candidate = plan()
            candidate['processes'][owner][key] = value
            with self.subTest(owner=owner, key=key), self.assertRaises(AssertionError):
                proof.validate_plan(candidate)

    def test_guard_revalidates_boot_pid_and_window_before_control(self):
        windows = [{'pid': 20, 'address': '0xc8', 'xwayland': False, 'title': 'cua-smoke-calc.ods',
                    'at': [10, 20], 'size': [800, 600]}, {'pid': 10, 'address': '0x64', 'xwayland': False}]
        for failure in (None, 'boot', 'process', 'address', 'geometry', 'native', 'document', 'ambiguous'):
            candidate = plan()
            desktop = object.__new__(proof.ExactDesktop)
            desktop.plan, desktop.instance = candidate, 'test_1'
            desktop.compositor = identity(50, 'Hyprland')
            rows = deepcopy(windows)
            if failure == 'address':
                rows[0]['address'] = '0xc9'
            elif failure == 'geometry':
                rows[0]['size'][0] += 1
            elif failure == 'native':
                rows[0]['xwayland'] = True
            elif failure == 'document':
                rows[0]['title'] = 'unowned'
            elif failure == 'ambiguous':
                rows.append(rows[0])
            with self.subTest(failure=failure), ExitStack() as stack:
                stack.enter_context(patch.object(proof.platform, 'system', return_value='Linux'))
                stack.enter_context(patch.object(proof, 'guest_identity', return_value={} if failure == 'boot' else candidate['vm']))
                stack.enter_context(patch.dict(proof.os.environ, {'HYPRLAND_INSTANCE_SIGNATURE': 'test_1'}))
                stack.enter_context(patch.object(proof.os, 'getuid', return_value=1000))
                stack.enter_context(patch.object(proof, '_same_compositor'))
                stack.enter_context(patch.object(proof, '_identity', side_effect=lambda pid: {} if failure == 'process' else identity(pid)))
                stack.enter_context(patch.object(proof, '_hypr', return_value=json.dumps(rows)))
                if failure:
                    with self.assertRaises(AssertionError):
                        desktop.guard()
                else:
                    self.assertEqual(desktop.guard(), windows)


class ActionTests(unittest.TestCase):
    def test_single_normal_call_fresh_snapshot_unknown_never_replayed(self):
        for phase in ('refusal', 'recovery'):
            for failure in (None, 'guard', 'stale', 'unknown', 'same_snapshot', 'effect', 'pre_activity',
                            'dead_before', 'dead_after', 'observation'):
                if phase == 'refusal' and failure == 'effect':
                    continue
                with self.subTest(phase=phase, failure=failure), ExitStack() as stack:
                    spec = plan()['agents'][0]
                    if phase == 'recovery':
                        spec['pointer_stage'] = 'click_b2'
                    actor, observer = client(101), client(102)
                    if failure == 'dead_before':
                        actor.process.poll.return_value = 1
                    elif failure == 'dead_after':
                        actor.process.poll.side_effect = [None, 1]
                    actor.tool.side_effect = TimeoutError('lost reply') if failure == 'unknown' else None
                    actor.tool.return_value = REFUSED if phase == 'refusal' else DELIVERED
                    before = {'snapshot_id': 'before', 'proof_image': 'before.png'}
                    after = {'snapshot_id': 'before' if failure == 'same_snapshot' else 'after', 'proof_image': 'after.png'}
                    prepared = {'snapshot': before, 'arguments': {'x': 20, 'y': 30}, 'oracle': {'stage': 'click_b2'},
                                'prepared_ns': 0}
                    stack.enter_context(patch.object(proof, 'prepare_drag', return_value=prepared))
                    snapshots = stack.enter_context(patch.object(proof, 'grounded_snapshot', return_value=after,
                        side_effect=AssertionError('snapshot unavailable') if failure == 'observation' else None))
                    stack.enter_context(patch.object(proof.time, 'monotonic_ns', return_value=proof.MAX_GROUNDING_AGE_NS + 1 if failure == 'stale' else 1))
                    stack.enter_context(patch.object(proof.pointer_grounding, 'read_pixels'))
                    stack.enter_context(patch.object(proof.pointer_grounding, 'verify', side_effect=AssertionError('effect missing') if failure == 'effect' else None))
                    tracer = Mock(collect=Mock(side_effect=[trace(RECOVERY) if failure == 'pre_activity' else trace(),
                                                           trace() if phase == 'refusal' else trace(RECOVERY)]))
                    guard = Mock(side_effect=AssertionError('primary changed') if failure == 'guard' else None)
                    save, record = Mock(), {}
                    if failure:
                        with self.assertRaises(AssertionError):
                            proof.action(actor, observer, spec, phase, tracer, guard, save, record)
                    else:
                        proof.action(actor, observer, spec, phase, tracer, guard, save, record)
                    attempted = failure not in ('stale', 'guard', 'pre_activity', 'dead_before')
                    self.assertEqual(actor.tool.call_count, int(attempted))
                    self.assertFalse(record['replayed'])
                    if attempted:
                        snapshots.assert_called_once_with(observer, spec['target'], spec, session=False)
                        self.assertEqual(actor.tool.call_args.args[1]['delivery_mode'], 'background')
                        self.assertEqual(actor.tool.call_args.args[1]['pid'], 20)
                        self.assertEqual(actor.tool.call_args.args[0], 'drag' if phase == 'refusal' else 'click')
                    if failure == 'unknown':
                        self.assertEqual(record['outcome'], 'unknown')
                        self.assertIn('after', record)
                        self.assertTrue(save.call_count >= 3)
                    if failure == 'observation':
                        self.assertEqual(record['observation_error'], 'snapshot unavailable')
                        self.assertIn('trace_after', record)
                        self.assertEqual(record['outcome'], 'response')
                        self.assertTrue(save.call_count >= 3)


class RunTests(unittest.TestCase):
    def test_invalid_plan_preserves_raw_plan_and_cleanup_without_native_calls(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            path = directory / 'plan.json'
            candidate = {**plan(), 'disposable': False}
            path.write_text(json.dumps(candidate))
            args = SimpleNamespace(plan=path, evidence=directory / 'evidence')
            with patch.object(proof, 'ExactDesktop') as desktop, patch('builtins.print'):
                self.assertEqual(proof.run(args), 1)
            desktop.assert_not_called()
            self.assertEqual(json.loads((args.evidence / 'plan.json').read_text()), candidate)
            self.assertEqual(json.loads((args.evidence / 'cleanup.json').read_text()), {'errors': []})

    def test_run_separates_setup_reaps_all_children_and_retains_failure_evidence(self):
        for failure in (None, 'refusal', 'recovery', 'close', 'release', 'primary_event', 'held',
                        'reused', 'start_lost', 'restarted_trace', 'reserved_refusal'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root, ExitStack() as stack:
                directory = Path(root)
                path = directory / 'plan.json'
                path.write_text(json.dumps(plan()))
                args = SimpleNamespace(plan=path, evidence=directory / 'evidence', driver=Path('/driver'),
                    primary_grab=Path('/grab'), trace_socket=Path('/cua-input-v3.sock'), foreground_journal=Path('/journal'))
                desktop = Mock()
                desktop.status.return_value = status()
                desktop.primary.side_effect = lambda target: {**target, 'cursor': {'x': 30, 'y': 40}, 'workspace': 1}
                stack.enter_context(patch.object(proof, 'ExactDesktop', return_value=desktop))
                stack.enter_context(patch.object(proof, 'app_process_identity'))
                stack.enter_context(patch.object(proof, 'provenance', return_value={'files': {}}))
                observer, refused, fresh = client(101), client(102), client(102 if failure == 'reused' else 103)
                observer.tool.return_value = {'structuredContent': {'screen_width': 1600, 'screen_height': 900}}
                for actor in (refused, fresh):
                    actor.tool.return_value = {}
                launched = []
                def launch(*_args):
                    value = (observer, refused, fresh)[len(launched)]
                    launched.append(value)
                    return value
                stack.enter_context(patch.object(proof, 'DirectMCP', side_effect=launch))
                stack.enter_context(patch.object(proof, 'grounded_snapshot', return_value={'window_bounds': BOUNDS}))
                grab = Mock(poll=Mock(return_value=None))
                grabbed = [False]
                def start_grab(*_args, **_kwargs):
                    self.assertIsNotNone(refused.process.poll())
                    self.assertEqual(len(launched), 2, 'new actor launched before intentional setup')
                    self.assertFalse(tracing[0], 'setup inside action trace')
                    grabbed[0] = True
                    return grab
                stack.enter_context(patch.object(proof.subprocess, 'Popen', side_effect=start_grab))
                grab.terminate.side_effect = lambda: setattr(grab.poll, 'return_value', 0)
                stack.enter_context(patch.object(proof, 'stop_process', side_effect=RuntimeError('release failed') if failure == 'release' else None))
                stack.enter_context(patch.object(proof, 'primary_acknowledgement', return_value='HELD\n'))
                stack.enter_context(patch.object(proof, 'wait_for', side_effect=lambda check, **kwargs: check()))
                stack.enter_context(patch.object(proof, 'state', side_effect=lambda _: {
                    'held': grabbed[0] and grab.poll() is None, 'clicks': 0, 'keys': 0, 'scroll': 0}))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', return_value=0))
                starts, tracing, action_done = [0], [False], [False]
                def exchange(command):
                    if command == 'TRACE_START':
                        self.assertFalse(tracing[0])
                        starts[0] += 1
                        tracing[0], action_done[0] = True, False
                        if failure == 'start_lost':
                            raise TimeoutError('trace start acknowledgement lost')
                    elif command == 'TRACE_STOP':
                        tracing[0] = False
                def collect():
                    events = RECOVERY if starts[0] == 2 and action_done[0] else []
                    if not tracing[0] and failure == 'primary_event':
                        events = [*events, (10, 'keyboard_key', 0, 1)]
                    if not tracing[0] and failure == 'held':
                        events = [*events, (10, 'pointer_button', 1, 1)]
                    if not tracing[0] and failure == 'restarted_trace':
                        events = [*events, (10, 'start', 0, 0)]
                    return trace(events, active=tracing[0])
                tracer = Mock(exchange=Mock(side_effect=exchange), collect=Mock(side_effect=collect))
                desktop.trace.return_value = tracer
                def do_action(actor, obs, spec, phase, trace_obj, guard, save, record):
                    guard()
                    action_done[0] = True
                    record.update(trace_after=collect(), outcome='response', replayed=False)
                    save(phase + '-action.json', record)
                    if failure == 'reserved_refusal':
                        def reserved_status(*, unreserved=False):
                            value = status()
                            value['input']['lanes'][0]['reserved'] = True
                            return proof.clear_status(value, unreserved=unreserved)
                        desktop.status.side_effect = reserved_status
                    if failure == phase:
                        raise AssertionError(phase + ' failed')
                stack.enter_context(patch.object(proof, 'action', side_effect=do_action))
                def close(value):
                    value.process.poll.return_value = 0
                    if failure == 'close' and value is fresh:
                        raise RuntimeError('close failed')
                stack.enter_context(patch.object(proof, 'close_owned', side_effect=close))
                stack.enter_context(patch('builtins.print'))
                self.assertEqual(proof.run(args), 0 if failure is None else 1)
                report = json.loads((args.evidence / 'result.json').read_text())
                self.assertEqual(report['active_drag_cancellation'], 'unproven')
                self.assertEqual(report['continuous_isolation_across_setup'], 'unproven')
                self.assertTrue((args.evidence / 'refusal-trace.json').exists())
                self.assertTrue((args.evidence / 'provenance.json').exists())
                self.assertTrue(all(value.process.poll() is not None for value in launched))
                tracer.close.assert_called_once()
                if failure in ('release', 'close', 'primary_event', 'held'):
                    self.assertTrue(report.get('error') or json.loads((args.evidence / 'cleanup.json').read_text())['errors'])
                if failure is None:
                    self.assertEqual(starts[0], 2)
                    self.assertTrue((args.evidence / 'setup-transition.json').exists())
                    self.assertTrue((args.evidence / 'recovery-trace.json').exists())
                    self.assertEqual(set(report['phases']), {'refusal', 'recovery'})
                    files = json.loads((args.evidence / 'provenance.json').read_text())['files']
                    for name in ('desktop_faults.py', 'production_cancel_proof.py',
                                 'production_desktop_fault_proof.py', 'production_geometry_fault_proof.py'):
                        self.assertEqual(len(files[name]['sha256']), 64)
                if failure == 'reserved_refusal':
                    self.assertEqual(len(launched), 2, 'recovery followed a non-idle refusal')


if __name__ == '__main__':
    unittest.main()
