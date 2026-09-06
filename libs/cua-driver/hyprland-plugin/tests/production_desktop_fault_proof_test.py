"""Portable contracts only: no native VM, compositor, policy or input is touched."""
from contextlib import ExitStack, nullcontext
from copy import deepcopy
import json
import os
from pathlib import Path
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_desktop_fault_proof as proof
from production_geometry_fault_proof_test import (ACTIVE, CANCEL, BOUNDS, action,
                                                  client, plan as geometry_plan, trace)


def plan():
    candidate = geometry_plan()
    candidate.update(purpose='desktop_fault', fault={'kind': 'config_disable'},
        vm={'machine_id': 'a' * 32, 'boot_id': 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'},
        compositor={'pid': 50, 'instance': 'test_1', 'uid': 1000, 'starttime': '77', 'exe': '/usr/bin/Hyprland'},
        config={'path': '/guest/input.lua', 'device': 1, 'inode': 2, 'uid': 1000,
                'mode': 0o600, 'sha256': proof.digest(proof.ENABLED.encode())})
    return candidate


def status(enabled=True):
    return {'configured': enabled, 'transport': {'ready': enabled}, 'input': {
        'protocol': 3, 'test_only': False, 'seat_lifetime': 'compositor', 'upgrade': 'desktop_restart',
        'transport_ready': enabled, 'lanes': [
            {'lane': lane, 'held_button': 0, 'held_keys': 0, 'drag_active': False, 'lease_active': False}
            for lane in (0, 1)]}}


def record():
    return {'result': 'observed', 'kind': 'config_disable', 'prefix': trace(ACTIVE), 'lane': 1,
            'gate_ns': 5_000_000, 'requested_ns': 6_000_000, 'acknowledged_ns': 10_000_000,
            'watchdog_deadline_ns': 90_000_000, 'before': status(), 'after': status(False),
            'config': {'sha256': proof.digest(proof.DISABLED.encode())}}


def restoration():
    return {'result': 'restored', 'started_ns': 12_000_000, 'observed_ns': 13_000_000,
            'config': {'sha256': proof.digest(proof.ENABLED.encode())}, 'status': status()}


class OracleTests(unittest.TestCase):
    def test_partial_unknown_and_exact_history_pass_without_claiming_app_effect(self):
        for observed in (action(), {'outcome': 'unknown', 'replayed': False}):
            result = proof.verify_fault(trace(CANCEL), record(), restoration(), observed)
            self.assertEqual(result['result'], 'verified')
            self.assertEqual(result['saved_document_effect'], 'unproven')

    def test_missing_press_release_or_cancel_cross_lane_and_replay_fail(self):
        cases = [ACTIVE, CANCEL[:6] + CANCEL[7:], CANCEL[:5] + CANCEL[6:]]
        for kind in ('agent_admitted', 'agent_drag_end', 'agent_action_end', 'keyboard_key',
                     'pointer_axis', 'pointer_enter', 'pointer_motion'):
            cases.append(CANCEL + [(11, kind, 1, 0)])
        cases.append(CANCEL + [(11, 'pointer_leave', 2, 0)])
        for rows in cases:
            with self.subTest(rows=rows), self.assertRaises(AssertionError):
                proof.verify_fault(trace(rows), record(), restoration(), action())
        candidate = record()
        candidate['prefix'] = trace(ACTIVE[:3])
        with self.assertRaises(AssertionError):
            proof.verify_fault(trace(CANCEL), candidate, restoration(), action())
        with self.assertRaises(AssertionError):
            proof.verify_fault(trace(CANCEL), record(), restoration(), {**action(), 'replayed': True})

    def test_fault_cancellation_and_release_must_precede_restoration(self):
        for changes in ({'requested_ns': 9_000_000}, {'gate_ns': 7_000_000},
                        {'requested_ns': 300_000_000}, {'watchdog_deadline_ns': 12_000_000}):
            with self.subTest(changes=changes), self.assertRaises(AssertionError):
                proof.verify_fault(trace(CANCEL), {**record(), **changes}, restoration(), action())
        late = ACTIVE + [(12, 'agent_cancel', 1, 0), (13, 'pointer_button', 1, 0)]
        with self.assertRaisesRegex(AssertionError, 'during fault'):
            proof.verify_fault(trace(late), record(), restoration(), action())
        late_release = ACTIVE + [(8, 'agent_cancel', 1, 0), (12, 'pointer_button', 1, 0)]
        with self.assertRaisesRegex(AssertionError, 'delayed'):
            proof.verify_fault(trace(late_release), record(), restoration(), action())

    def test_reconnected_trace_must_be_complete_and_preserve_prefix(self):
        for key, value in (('overflow', True), ('timed_out', True), ('hook', False), ('active', False), ('count', 0)):
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_fault({**trace(CANCEL), key: value}, record(), restoration(), action())
        for page in (trace(CANCEL[5:]), trace(CANCEL)):
            page['events'][1][1] += 1
            with self.assertRaises(AssertionError):
                proof.verify_fault(page, record(), restoration(), action())

    def test_primary_input_focus_and_transient_warp_fail(self):
        for kind, value in (('pointer_button', 0), ('keyboard_key', 1), ('pointer_axis', 0), ('pointer_focus', 0)):
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                proof.verify_fault(trace(CANCEL + [(11, kind, 0, value)]), record(), restoration(), action())
        page = trace(CANCEL + [(11, 'cursor', 0, 0), (12, 'cursor', 0, 0)])
        page['events'][-2][3] += 5
        with self.assertRaises(AssertionError):
            proof.verify_fault(page, record(), restoration(), action())

    def test_exact_fault_bytes_and_restoration_required(self):
        for key, value in (('result', 'unproven'), ('kind', 'keymap'), ('config', {'sha256': 'bad'})):
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_fault(trace(CANCEL), {**record(), key: value}, restoration(), action())
        for update in ({'result': 'not_needed'}, {'config': {'sha256': 'bad'}}, {'status': status(False)}):
            with self.assertRaises(AssertionError):
                proof.verify_fault(trace(CANCEL), record(), {**restoration(), **update}, action())


class SafetyTests(unittest.TestCase):
    def test_plan_rejects_other_faults_and_incomplete_identity(self):
        proof.validate_plan(plan())
        for change in ({'disposable': False}, {'fault': {'kind': 'keymap'}}, {'fault': {'kind': 'dpms'}},
                       {'fault': {'kind': 'lock'}}, {'vm': {'machine_id': 'bad', 'boot_id': 'bad'}},
                       {'config': {**plan()['config'], 'sha256': 'arbitrary config'}},
                       {'compositor': {**plan()['compositor'], 'exe': '/usr/bin/other'}},
                       {'recovery': {'pointer_stage': 'select_range'}}):
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.validate_plan({**plan(), **change})

    def test_host_bare_metal_and_wrong_process_refused_before_file_access(self):
        for system, vm_return in (('Darwin', 0), ('Linux', 1)):
            with patch.object(proof.platform, 'system', return_value=system), \
                 patch.object(proof.subprocess, 'run', return_value=Mock(returncode=vm_return)), \
                 patch.object(proof, 'file_identity') as files, self.assertRaises(AssertionError):
                proof.ConfigFault(plan(), Path('/unused'))
            files.assert_not_called()
        with patch.object(proof.platform, 'system', return_value='Linux'), \
             patch.object(proof.subprocess, 'run', return_value=Mock(returncode=0)), \
             patch.object(proof.os, 'getuid', return_value=1000), \
             patch.object(proof, '_identity', return_value={}), \
             patch.object(proof, 'file_identity') as files, self.assertRaises(AssertionError):
            proof.ConfigFault(plan(), Path('/unused'))
        files.assert_not_called()

    def test_status_rejects_old_signed_and_uncleared_native_state(self):
        for enabled in (True, False):
            with patch.object(proof, '_hypr', return_value=json.dumps(status(enabled))):
                proof.production_status('exact', enabled)
        mutations = [('protocol', 0), ('test_only', True), ('seat_lifetime', 'action'), ('transport_ready', True)]
        for key, value in mutations:
            candidate = status(False)
            candidate['input'][key] = value
            with patch.object(proof, '_hypr', return_value=json.dumps(candidate)), self.assertRaises(AssertionError):
                proof.production_status('exact', False)
        for key, value in (('held_button', True), ('held_keys', 1), ('drag_active', True), ('lease_active', True)):
            candidate = status(False)
            candidate['input']['lanes'][1][key] = value
            with patch.object(proof, '_hypr', return_value=json.dumps(candidate)), self.assertRaises(AssertionError):
                proof.production_status('exact', False)
        candidate = status(False)
        candidate['experiment'] = candidate.pop('input')
        with patch.object(proof, '_hypr', return_value=json.dumps(candidate)), self.assertRaises(AssertionError):
            proof.production_status('exact', False)

    def prepare_files(self, directory):
        config = {'path': str(directory / 'input.lua'), 'files': {}}
        for name, data in (('original', proof.ENABLED), ('disabled', proof.DISABLED), ('restored', proof.ENABLED)):
            path = Path(config['path']) if name == 'original' else directory / (name + '.stage')
            path.write_bytes(data.encode())
            path.chmod(0o600)
            config['files'][name] = {'path': str(path), 'identity': proof.file_identity(path)}
        return config

    def test_atomic_fixed_transition_restores_exact_bytes_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as root, patch.object(proof, '_guard'):
            config = self.prepare_files(Path(root).resolve())
            proof._replace(config, False)
            self.assertEqual(Path(config['path']).read_bytes(), proof.DISABLED.encode())
            self.assertEqual(proof.file_identity(config['path']), config['files']['disabled']['identity'])
            proof._replace(config, True)
            proof._replace(config, True)
            self.assertEqual(Path(config['path']).read_bytes(), proof.ENABLED.encode())
            self.assertEqual(proof.file_identity(config['path']), config['files']['restored']['identity'])

    def test_unrelated_bytes_inode_symlink_and_stage_change_preserved(self):
        for change in ('bytes', 'inode', 'link', 'stage'):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as root, patch.object(proof, '_guard'):
                config = self.prepare_files(Path(root).resolve())
                proof._replace(config, False)
                path = Path(config['path'])
                if change == 'bytes':
                    path.write_bytes(b'unrelated')
                elif change == 'inode':
                    replacement = path.with_name('replacement')
                    replacement.write_bytes(proof.DISABLED.encode())
                    replacement.chmod(0o600)
                    os.replace(replacement, path)
                elif change == 'link':
                    path.unlink()
                    path.symlink_to(config['files']['restored']['path'])
                else:
                    Path(config['files']['restored']['path']).write_bytes(b'unrelated')
                with patch.object(proof.os, 'replace') as replace, self.assertRaises(AssertionError):
                    proof._replace(config, True)
                replace.assert_not_called()

    def test_group_writable_and_hardlinked_config_are_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root).resolve() / 'input.lua'
            path.write_bytes(proof.ENABLED.encode())
            path.chmod(0o666)
            with self.assertRaises(AssertionError):
                proof.file_identity(path)
            path.chmod(0o600)
            os.link(path, path.with_name('alias'))
            with self.assertRaises(AssertionError):
                proof.file_identity(path)

    def test_constructor_stages_only_fixed_bytes_and_close_preserves_original(self):
        with tempfile.TemporaryDirectory() as root, ExitStack() as stack:
            directory = Path(root).resolve()
            path = directory / 'input.lua'
            path.write_bytes(proof.ENABLED.encode())
            path.chmod(0o600)
            candidate = plan()
            candidate['config'] = {'path': str(path), **proof.file_identity(path)}
            candidate['compositor']['uid'] = os.getuid()
            identity = {k: v for k, v in candidate['compositor'].items() if k != 'instance'}
            stack.enter_context(patch.object(proof.platform, 'system', return_value='Linux'))
            stack.enter_context(patch.object(proof.subprocess, 'run', return_value=Mock(returncode=0)))
            stack.enter_context(patch.object(proof, '_identity', return_value=identity))
            stack.enter_context(patch.object(proof, '_guard'))
            stack.enter_context(patch.object(proof, 'production_status', return_value=status()))
            reload = stack.enter_context(patch.object(proof, '_reload'))
            fault = proof.ConfigFault(candidate, directory)
            original = proof.file_identity(path)
            for name, data in (('disabled', proof.DISABLED), ('restored', proof.ENABLED)):
                staged = Path(fault.config['files'][name]['path'])
                self.assertEqual(staged.read_bytes(), data.encode())
                self.assertEqual(staged.suffix, '.stage')
            fault.close()
            self.assertEqual(proof.file_identity(path), original)
            self.assertEqual(list(directory.iterdir()), [path])
            reload.assert_not_called()

    def test_final_active_gate_failure_prevents_atomic_replacement(self):
        with tempfile.TemporaryDirectory() as root, patch.object(proof, '_guard'):
            config = self.prepare_files(Path(root).resolve())
            with patch.object(proof.os, 'replace') as replace, self.assertRaisesRegex(AssertionError, 'stale'):
                proof._replace(config, False, before_replace=Mock(side_effect=AssertionError('stale')))
            replace.assert_not_called()
            self.assertEqual(Path(config['path']).read_bytes(), proof.ENABLED.encode())

    def test_restore_readback_failure_preserves_cleanup_obligation(self):
        with tempfile.TemporaryDirectory() as root, ExitStack() as stack:
            config = self.prepare_files(Path(root).resolve())
            stack.enter_context(patch.object(proof, '_guard'))
            stack.enter_context(patch.object(proof, '_locked', side_effect=lambda _: nullcontext()))
            stack.enter_context(patch.object(proof, '_reload', side_effect=TimeoutError('lost reload reply')))
            proof._replace(config, False)
            with self.assertRaises(TimeoutError):
                proof.restore_config(config)
            self.assertEqual(Path(config['path']).read_bytes(), proof.ENABLED.encode())
            # Restored bytes alone are not acknowledgment; retry must reload.
            with self.assertRaises(TimeoutError):
                proof.restore_config(config)

    def test_stale_vm_prevents_compositor_commands(self):
        with patch.object(proof.platform, 'system', return_value='Linux'), \
             patch.object(proof, 'guest_identity', return_value={}), \
             patch.object(proof, '_same_compositor') as compositor, self.assertRaises(AssertionError):
            proof._guard({'vm': plan()['vm']})
        compositor.assert_not_called()

    def test_trace_reconnection_binds_exact_compositor_peer_and_protocol(self):
        for peer, protocol in (((50, 1000, 1000), 3), ((51, 1000, 1000), 3),
                               ((50, 1001, 1000), 3), ((50, 1000, 1000), 0)):
            connection = Mock(hello={'protocol': protocol})
            connection.socket.getsockopt.return_value = struct.pack('3i', *peer)
            with patch.object(proof, '_guard') as guard, patch.object(proof.socket, 'SO_PEERCRED', 17, create=True):
                if peer[:2] == (50, 1000) and protocol == 3:
                    proof.verify_trace_peer(connection, {'compositor': plan()['compositor']})
                    guard.assert_called_once()
                else:
                    with self.assertRaises(AssertionError):
                        proof.verify_trace_peer(connection, {'compositor': plan()['compositor']})
                    guard.assert_not_called()

    def controller(self):
        fault = object.__new__(proof.ConfigFault)
        fault.config = {'path': '/unused', 'deadline_ns': 10_000_000_000}
        fault.child = Mock(poll=Mock(return_value=None))
        fault.cancel_fd = 8
        fault.record = {'result': 'unproven', 'kind': 'config_disable'}
        fault.restoration = None
        fault.mutated = False
        return fault

    def test_injection_requires_pending_press_fresh_gate_live_watchdog(self):
        for failure in (None, 'done', 'stale', 'watchdog', 'deadline', 'lost_reply'):
            fault = self.controller()
            if failure == 'watchdog':
                fault.child.poll.return_value = 0
            if failure == 'deadline':
                fault.config['deadline_ns'] = 1
            with ExitStack() as stack:
                stack.enter_context(patch.object(proof, '_locked', side_effect=lambda _: nullcontext()))
                stack.enter_context(patch.object(proof, 'poll_active', return_value=(trace(ACTIVE), {1: 2})))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[5, 300_000_006 if failure == 'stale' else 6, 7]))
                replace = stack.enter_context(patch.object(proof, '_replace', side_effect=lambda *_args, **kwargs: kwargs['before_replace']()))
                stack.enter_context(patch.object(proof, '_reload', side_effect=TimeoutError('lost') if failure == 'lost_reply' else lambda *_: status(False)))
                stack.enter_context(patch.object(proof, 'file_identity', return_value={'sha256': proof.digest(proof.DISABLED.encode())}))
                if failure:
                    with self.assertRaises((AssertionError, TimeoutError)):
                        fault.inject(Mock(), trace(ACTIVE[:1]), Mock(done=Mock(return_value=failure == 'done')), Mock())
                else:
                    fault.inject(Mock(), trace(ACTIVE[:1]), Mock(done=Mock(return_value=False)), Mock())
                    self.assertEqual(fault.record['result'], 'observed')
                self.assertEqual(replace.call_count, int(failure != 'watchdog'))
                self.assertEqual(fault.mutated, failure in (None, 'lost_reply'))
                if failure == 'lost_reply':
                    self.assertTrue(fault.mutated)

    def test_failed_restore_never_cancels_watchdog_or_deletes_backup(self):
        fault = self.controller()
        fault.mutated = True
        fault._clean_staged = Mock()
        with patch.object(proof, 'restore_config', side_effect=RuntimeError('unowned config')), \
             patch.object(proof.os, 'write') as write, patch.object(proof.os, 'close') as close:
            with self.assertRaises(RuntimeError):
                fault.close()
        write.assert_not_called()
        close.assert_called_once_with(8)
        fault._clean_staged.assert_not_called()

    def test_watchdog_eof_and_timeout_restore_cancel_does_not(self):
        for ready, data in (([], b''), ([8], b''), ([8], b'C')):
            with tempfile.TemporaryDirectory() as root, ExitStack() as stack:
                config = {'deadline_ns': 1, 'record': str(Path(root) / 'watchdog.json')}
                stack.enter_context(patch.object(proof, '_guard'))
                stack.enter_context(patch.object(proof.select, 'select', return_value=(ready, [], [])))
                stack.enter_context(patch.object(proof.os, 'read', return_value=data))
                restore = stack.enter_context(patch.object(proof, 'restore_config', return_value=restoration()))
                stack.enter_context(patch('builtins.print'))
                proof.watchdog(config, 8)
                self.assertEqual(restore.call_count, int(data != b'C'))
                self.assertTrue(Path(config['record']).exists())


class RunnerTests(unittest.TestCase):
    def test_invalid_plan_fails_before_native_controller_or_allocation(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            path = directory / 'plan.json'
            path.write_text(json.dumps({**plan(), 'disposable': False}))
            args = SimpleNamespace(plan=path, evidence=directory / 'evidence')
            with patch.object(proof, 'ConfigFault') as controller, patch.object(proof, 'DirectMCP') as launch, patch('builtins.print'):
                self.assertEqual(proof.run(args), 1)
            controller.assert_not_called()
            launch.assert_not_called()

    def test_reconnect_without_reset_restore_before_snapshot_and_new_action(self):
        for failure in (None, 'inject', 'restore', 'recovery'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root, ExitStack() as stack:
                directory = Path(root)
                path = directory / 'plan.json'
                path.write_text(json.dumps(plan()))
                args = SimpleNamespace(plan=path, evidence=directory / 'evidence', driver=Path('/driver'),
                    primary_grab=Path('/grab'), foreground_journal=Path('/journal'), trace_socket=Path('/cua-input-v3.sock'))
                order = []
                fault = Mock(record=record())
                def inject(*_):
                    order.append('inject')
                    if failure == 'inject':
                        raise AssertionError('injection failed')
                    return trace(ACTIVE), 1
                fault.inject.side_effect = inject
                def restore():
                    order.append('restore')
                    if failure == 'restore':
                        raise AssertionError('restore failed')
                    return restoration()
                fault.restore.side_effect = restore
                stack.enter_context(patch.object(proof, 'ConfigFault', return_value=fault))
                stack.enter_context(patch.object(proof, 'provenance', return_value={'files': {}}))
                agent, observer, fresh = client(100), client(101), client(102)
                agent.tool.return_value = {}
                observer.tool.return_value = {'structuredContent': {'screen_width': 1920, 'screen_height': 1080}}
                stack.enter_context(patch.object(proof, 'DirectMCP', side_effect=[agent, observer, fresh]))
                def snapshot(*_args, **_kwargs):
                    order.append('snapshot')
                    return {'window_bounds': dict(BOUNDS)}
                stack.enter_context(patch.object(proof, 'grounded_snapshot', side_effect=snapshot))
                stack.enter_context(patch.object(proof, 'prepare_drag', return_value={}))
                stack.enter_context(patch.object(proof, 'call_drag', return_value=action()))
                grab = Mock(poll=Mock(return_value=None))
                grab.terminate.side_effect = lambda: setattr(grab.poll, 'return_value', 0)
                stack.enter_context(patch.object(proof.subprocess, 'Popen', return_value=grab))
                stack.enter_context(patch.object(proof, 'stop_process'))
                stack.enter_context(patch.object(proof, 'primary_acknowledgement', return_value='HELD\n'))
                stack.enter_context(patch.object(proof, 'wait_for', return_value=True))
                stack.enter_context(patch.object(proof, 'wm', return_value={'pid': 10}))
                stack.enter_context(patch.object(proof, 'state', return_value={'held': True, 'clicks': 0, 'keys': 0, 'scroll': 0}))
                stack.enter_context(patch.object(proof, 'require_primary_active'))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', return_value=0))
                first = Mock(hello={'protocol': 3}, collect=Mock(return_value=trace(ACTIVE[:1])))
                page = trace(CANCEL + [(14, 'agent_admitted', 1, 0), (15, 'pointer_button', 1, 1),
                                      (16, 'pointer_button', 1, 0), (17, 'agent_action_end', 1, 0)])
                last = proof.stopped_prefix(page if failure is None else trace(CANCEL))
                second = Mock(hello={'protocol': 3}, collect=Mock(side_effect=[trace(CANCEL), trace(CANCEL), last]))
                stack.enter_context(patch.object(proof, 'connect_trace', side_effect=[first, second]))
                def recover(*args):
                    order.append('recovery')
                    if failure == 'recovery':
                        raise AssertionError('recovery failed')
                    args[-1]['result'] = 'verified'
                    return page
                stack.enter_context(patch.object(proof, 'recover', side_effect=recover))
                stack.enter_context(patch.object(proof, 'close_owned', side_effect=lambda c: setattr(c.process.poll, 'return_value', 0)))
                stack.enter_context(patch('builtins.print'))
                self.assertEqual(proof.run(args), int(failure is not None))
                fault.close.assert_called_once()
                first.exchange.assert_any_call('TRACE_START')
                self.assertFalse(any(call.args == ('TRACE_START',) for call in second.exchange.call_args_list))
                self.assertEqual(agent.process.poll(), 0)
                self.assertEqual(observer.process.poll(), 0)
                if failure not in ('inject', 'restore'):
                    self.assertLess(order.index('restore'), len(order) - 1 - order[::-1].index('snapshot'))
                    self.assertLess(order.index('restore'), order.index('recovery'))
                report = json.loads((args.evidence / 'result.json').read_text())
                self.assertFalse(report['full_desktop_matrix'])
                self.assertFalse(report['physical_hardware'])


if __name__ == '__main__':
    unittest.main()
