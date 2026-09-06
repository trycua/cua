import copy
from contextlib import ExitStack
import io
import json
from pathlib import Path
import subprocess
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import zipfile

from production_mcp import DirectMCP, assert_distinct_runtimes, profile_environment, stop_process
from production_realapp_proof import (assert_no_dispatch, assert_primary_state, check_response,
                                    expected_primary_motion, move_primary, primary_acknowledgement,
                                    primary_trajectory, run, validate_plan, verify_output)
from primary_trace import analyze
from primary_trace_test import START, STOP, trace


def plan():
    return {'purpose': 'apps', 'foreground': {'pid': 10, 'window_id': 100},
            'agents': [{'app': 'calc', 'target': {'pid': 20, 'window_id': 200}},
                       {'app': 'inkscape', 'target': {'pid': 30, 'window_id': 300}}],
            'phases': [{'parallel': [{'agent': 0, 'tool': 'drag', 'arguments': {}},
                                     {'agent': 1, 'tool': 'drag', 'arguments': {}}]}],
            'outputs': [{'agent': 0, 'attributes': {'value': '96'}},
                        {'agent': 1, 'rect_translation': [[10, 20], [30, 40]]}]}


class PlanTests(unittest.TestCase):
    def test_moving_primary_is_optional_boolean_and_excludes_negative_control(self):
        validate_plan({**plan(), 'moving_primary': True})
        for value in ('true', 1, None):
            with self.assertRaises(AssertionError):
                validate_plan({**plan(), 'moving_primary': value})
        negative = {**plan(), 'purpose': 'negative_control',
                    'phases': [{'negative_control': True}]}
        validate_plan(negative)
        with self.assertRaisesRegex(AssertionError, 'parked'):
            validate_plan({**negative, 'moving_primary': True})

    def test_real_apps_need_independent_processes_and_both_output_oracles(self):
        validate_plan(plan())
        for mutate in (lambda p: p['agents'][1].update(target=p['agents'][0]['target']),
                       lambda p: p['agents'][1].update(app='gtk-fixture'),
                       lambda p: p.update(outputs=p['outputs'][:1]),
                       lambda p: p.update(phases=[])):
            bad = plan()
            mutate(bad)
            with self.assertRaises(AssertionError):
                validate_plan(bad)

    def test_public_arguments_cannot_override_reviewed_target_or_session(self):
        for key in ('pid', 'window_id', 'session', 'delivery_mode'):
            bad = plan()
            bad['phases'][0]['parallel'][0]['arguments'][key] = 'other'
            with self.assertRaises(AssertionError):
                validate_plan(bad)

    def test_one_connection_cannot_execute_parallel_calls(self):
        bad = plan()
        bad['phases'][0]['parallel'][1]['agent'] = 0
        with self.assertRaises(AssertionError):
            validate_plan(bad)

    def test_parallel_denial_has_no_quiet_no_dispatch_interval(self):
        bad = plan()
        bad['phases'][0]['parallel'][0]['expect'] = {'kind': 'refused', 'reason': 'permission_denied'}
        with self.assertRaises(AssertionError):
            validate_plan(bad)

    def test_policy_plan_cannot_count_dispatched_action_as_deny(self):
        bad = plan()
        bad['purpose'] = 'policy'
        with self.assertRaises(AssertionError):
            validate_plan(bad)


class ResponseTests(unittest.TestCase):
    def test_common_policy_refusal_uses_existing_envelope_not_invented_action_fields(self):
        result = {'isError': True, 'structuredContent': {'status': 'refused',
                  'refusal': {'code': 'bounded_resource_outside_manifest', 'message': 'out of scope'}}}
        check_response(result, {'kind': 'refused', 'reason': 'bounded_resource_outside_manifest'})
        with self.assertRaises(AssertionError):
            check_response(result, {'kind': 'refused', 'reason': 'permission_denied'})

    def test_dispatched_does_not_mean_verified_app_effect(self):
        response = {'structuredContent': {'route': 'synthetic_events', 'effect': 'unverifiable',
                                         'delivery': {'mode': 'background'}}}
        checked = check_response(response, {'kind': 'dispatched'})
        self.assertFalse(checked['app_effect_verified'])
        for change in ({'effect': 'partial'}, {'effect': 'suspected_noop'},
                       {'delivery': {'mode': 'unknown'}}, {'route': 'global_input'}):
            with self.assertRaises(AssertionError):
                check_response({'structuredContent': {**response['structuredContent'], **change}},
                               {'kind': 'dispatched'})

    def test_refusal_requires_exact_reason_and_absent_delivery(self):
        content = {'effect': 'refused', 'reason': 'permission_denied'}
        expected = {'kind': 'refused', 'reason': 'permission_denied'}
        check_response({'isError': True, 'structuredContent': content}, expected)
        for change in ({'reason': 'unsupported'}, {'effect': 'partial'},
                       {'delivery': {'mode': 'unknown', 'delivered_count': 0}}):
            with self.assertRaises(AssertionError):
                check_response({'isError': True, 'structuredContent': {**content, **change}}, expected)

    def test_partial_and_unknown_are_explicit_not_success_relabels(self):
        content = {'route': 'synthetic_events', 'effect': 'partial',
                   'delivery': {'mode': 'unknown', 'delivered_count': 2}}
        for kind in ('partial', 'unknown'):
            self.assertEqual(check_response({'isError': True, 'structuredContent': content},
                                           {'kind': kind})['expected'], kind)
        with self.assertRaises(AssertionError):
            check_response({'structuredContent': {**content, 'delivery': {'mode': 'background'}}},
                           {'kind': 'partial'})

    def test_no_dispatch_rejects_press_without_completion(self):
        before = {**trace(START), 'active': True}
        assert_no_dispatch(before, copy.deepcopy(before))
        for kind in ('pointer_button', 'agent_approved', 'agent_drag_start'):
            after = {**trace(START, (kind, 100, 200, 1, 1)), 'active': True}
            with self.assertRaises(AssertionError):
                assert_no_dispatch(before, after)

    def test_no_dispatch_rejects_missing_or_changed_telemetry(self):
        before = {**trace(START), 'active': True}
        for change in ({'hook': False}, {'overflow': True}, {'timed_out': True},
                       {'count': 2}, {'active': False}, {'events': []}):
            with self.assertRaises(AssertionError):
                assert_no_dispatch(before, {**before, **change})


class OutputTests(unittest.TestCase):
    def test_svg_asserts_saved_translation_and_rejects_resize(self):
        before = b'<svg><rect id="shape" x="1" y="2" width="3" height="4"/></svg>'
        after = b'<svg><rect id="shape" x="11" y="32" width="3" height="4"/></svg>'
        oracle = {'agent': 1, 'path': 'drawing.svg', 'xpath': './rect',
                  'rect_translation': [[10, 10], [30, 30]]}
        self.assertTrue(verify_output(before, after, oracle)['verified'])
        for invalid in (before, after.replace(b'width="3"', b'width="5"')):
            with self.assertRaises(AssertionError):
                verify_output(before, invalid, oracle)

    def test_calc_can_use_independently_read_ods_content(self):
        def ods(value):
            stream = io.BytesIO()
            with zipfile.ZipFile(stream, 'w') as archive:
                archive.writestr('content.xml', f'<sheet><cell value="{value}"/></sheet>')
            return stream.getvalue()
        oracle = {'agent': 0, 'path': 'sheet.ods', 'zip_member': 'content.xml',
                  'xpath': './cell', 'attributes': {'value': '96'}}
        self.assertTrue(verify_output(ods(1), ods(96), oracle)['verified'])
        with self.assertRaises(AssertionError):
            verify_output(ods(1), ods(75), oracle)


def motion_command(sequence, point, command_ns=1, ack_ns=2):
    x, y = point
    return {'sequence': sequence, 'x': x, 'y': y, 'command_ns': command_ns,
            'ack_ns': ack_ns, 'acknowledgement': f'MOVED {x} {y}\n'}


class MovingPrimaryTests(unittest.TestCase):
    def test_entire_square_must_fit_inside_foreground_and_desktop(self):
        bounds = {'x': 0, 'y': 0, 'width': 600, 'height': 600}
        desktop = {'screen_width': 800, 'screen_height': 800}
        route = primary_trajectory(bounds, [100, 200], desktop)
        self.assertEqual(len(route), 32)
        self.assertEqual(route[0], [120, 200])
        self.assertEqual(route[-1], [100, 200])
        for point in ([450, 200], [100, 450], [-1, 200]):
            with self.assertRaises(AssertionError):
                primary_trajectory(bounds, point, desktop)
        with self.assertRaisesRegex(AssertionError, 'desktop'):
            primary_trajectory({**bounds, 'x': 700}, [100, 200], desktop)

    def test_moving_endpoints_allow_only_cursor_change(self):
        before = {'pid': 10, 'address': '0x10', 'workspace': 1, 'cursor': {'x': 100, 'y': 200}}
        after = {**before, 'cursor': {'x': 120, 'y': 200}}
        assert_primary_state(before, after, True)
        with self.assertRaises(AssertionError):
            assert_primary_state(before, after, False)
        for key in ('pid', 'address', 'workspace'):
            with self.assertRaises(AssertionError):
                assert_primary_state(before, {**after, key: 'changed'}, True)

    def test_missing_malformed_incomplete_and_reordered_logs_fail(self):
        route = [[120, 200], [140, 200]]
        commands = [motion_command(1, route[0]), motion_command(2, route[1], 3, 4)]
        self.assertEqual(expected_primary_motion(commands, route), route)
        malformed = [None, [], {}, ['bad'], commands[1:], list(reversed(commands))]
        for change in ({'sequence': True}, {'sequence': 3}, {'x': float('nan')},
                       {'y': 300}, {'command_ns': 0}, {'ack_ns': None}, {'ack_ns': -1},
                       {'acknowledgement': None}, {'acknowledgement': 'MOVED 140 200\n'}):
            malformed.append([{**commands[0], **change}, commands[1]])
        for log in malformed:
            with self.subTest(log=log), self.assertRaises(AssertionError):
                expected_primary_motion(log, route)

    def test_trace_must_match_commands_without_extra_motion_focus_or_grab_changes(self):
        route = [[120, 200], [100, 200]]
        commands = [motion_command(1, route[0]), motion_command(2, route[1], 3, 4)]
        expected = expected_primary_motion(commands, route)
        events = [('cursor', *point, 0, 0) for point in route]
        self.assertEqual(analyze(trace(START, *events, STOP), expected_motion=expected)['result'], 'passed')
        bad_events = [events[:1], list(reversed(events)),
                      [events[0], ('cursor', 150, 200, 0, 0), *events[1:]]]
        for kind in ('pointer_focus', 'keyboard_focus', 'pointer_button', 'pointer_axis', 'keyboard_key'):
            bad_events.append([events[0], (kind, 120, 200, 0, 0), events[1]])
        for rows in bad_events:
            self.assertEqual(analyze(trace(START, *rows, STOP), expected_motion=expected)['result'], 'failed')
        self.assertEqual(analyze({**trace(START, *events, STOP), 'overflow': True},
                                 expected_motion=expected)['result'], 'inconclusive')

    def test_acknowledgement_rejects_timeout_eof_partial_and_oversized_lines(self):
        stream = Mock()
        for chunks, readiness in (([b''], [[stream]]), ([b'MOVED 120'], [[stream], []]),
                                  ([b'x' * 128, b'x\n'], [[stream], [stream]])):
            with patch('production_realapp_proof.os.read', side_effect=chunks), \
                    patch('production_realapp_proof.select.select', side_effect=[(r, [], []) for r in readiness]), \
                    self.assertRaises(AssertionError):
                primary_acknowledgement(stream)
        with patch('production_realapp_proof.os.read', side_effect=[b'MOVED ', b'120 200\n']), \
                patch('production_realapp_proof.select.select', return_value=([stream], [], [])):
            self.assertEqual(primary_acknowledgement(stream), 'MOVED 120 200\n')

    def test_independent_stream_retains_each_command_and_requires_exact_ack(self):
        for acknowledgement in ('MOVED 120 200\n', 'MOVED 999 200\n', 'MOVED 120 200\nextra\n'):
            commands, grab, ready = [], Mock(), Mock()
            done = Mock(wait=Mock(side_effect=[False, True]))
            with patch('production_realapp_proof.primary_acknowledgement', return_value=acknowledgement):
                if acknowledgement == 'MOVED 120 200\n':
                    move_primary(grab, [[120, 200]], done, ready, commands, Mock())
                    self.assertEqual(expected_primary_motion(commands, [[120, 200]]), [[120, 200]])
                    ready.set.assert_called_once()
                else:
                    with self.assertRaisesRegex(AssertionError, 'malformed'):
                        move_primary(grab, [[120, 200]], done, ready, commands, Mock())
                    ready.set.assert_not_called()
            grab.stdin.write.assert_called_once_with('MOVE 120 200\n')
            grab.stdin.flush.assert_called_once()
            self.assertEqual(commands[0]['acknowledgement'], acknowledgement)

    def test_no_trace_fails_before_provenance_or_process_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source_plan = root / 'plan.json'
            source_plan.write_text(json.dumps({**plan(), 'moving_primary': True}))
            args = SimpleNamespace(plan=source_plan, evidence=root / 'evidence', trace_socket=None)
            with patch('production_realapp_proof.provenance') as provenance, \
                    patch('production_realapp_proof.subprocess.Popen') as spawn:
                self.assertEqual(run(args), 1)
                provenance.assert_not_called()
                spawn.assert_not_called()
            result = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(result['result'], 'failed')
            self.assertEqual(result['error'], 'moving primary requires continuous trace')
            self.assertEqual(result['continuous_isolation'], 'unproven')

    def test_runner_uses_acknowledged_motion_and_brackets_background_action(self):
        # Exercise orchestration without a compositor, Driver process, or GUI.
        for during_action, failure in ((True, None), (False, None), (True, 'ack'),
                                       (True, 'join'), (True, 'action'), (True, 'startup'),
                                       (True, 'late_ack'), (True, 'motion')):
            with self.subTest(during_action=during_action, failure=failure), \
                    tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                root = Path(directory)
                bounds = {'x': 0, 'y': 0, 'width': 600, 'height': 600}
                spec = {'target': {'pid': 20, 'window_id': 200}, 'bounds': bounds,
                        'name': 'agent', 'profile': {'mode': 'standard'}}
                source_plan = root / 'plan.json'
                source_plan.write_text(json.dumps({
                    'purpose': 'policy', 'moving_primary': True, 'primary_point': [100, 200],
                    'foreground': {'pid': 10, 'window_id': 100}, 'agents': [spec],
                    'phases': [{'agent': 0, 'tool': 'click', 'arguments': {'x': 10, 'y': 20},
                                'expect': {'kind': 'refused', 'reason': 'permission_denied'}}]}))
                args = SimpleNamespace(plan=source_plan, evidence=root / 'evidence',
                                       trace_socket=root / 'cua-input-v3.sock', driver=root / 'driver',
                                       primary_grab=root / 'primary-grab', foreground_journal=root / 'journal',
                                       record_video=False)
                log, action_calls, held = [], [], [True]

                def append_motion(point):
                    now = time.monotonic_ns()
                    log.append(motion_command(len(log) + 1, point, now, now))

                def movement(grab, trajectory, done, ready, commands, mark):
                    nonlocal log
                    log = commands
                    append_motion([120, 200])
                    if failure == 'motion':
                        raise RuntimeError('movement failed')
                    ready.set()

                def tool(name, arguments):
                    if name == 'get_window_state':
                        action_calls.append('snapshot')
                        return {'structuredContent': {'screenshot_width': 600, 'window_bounds': bounds}}
                    if name == 'get_desktop_state':
                        return {'structuredContent': {'screen_width': 800, 'screen_height': 800}}
                    if name == 'click':
                        action_calls.append('click')
                        self.assertEqual(arguments['delivery_mode'], 'background')
                        if during_action:
                            append_motion([140, 200])
                            if failure == 'ack':
                                log[-1]['acknowledgement'] = None
                            if failure == 'late_ack':
                                log[-1]['ack_ns'] += 1_000_000_000_000
                        if failure == 'action':
                            raise RuntimeError('transport failed')
                        return {'isError': True, 'structuredContent': {
                            'effect': 'refused', 'reason': 'permission_denied'}}
                    return {'structuredContent': {}}

                agent = Mock(process=Mock(pid=11, poll=Mock(return_value=None)))
                observer = Mock()
                agent.tool.side_effect = observer.tool.side_effect = tool
                grab = Mock(stdout=io.StringIO('HELD\n'), poll=Mock(return_value=0))
                trace_client = Mock(hello={'protocol': 3})
                initial = ('cursor', 120, 200, 0, 0)
                final = ('cursor', 140, 200, 0, 0)
                events = [initial, final] if during_action else [initial]
                trace_client.collect.side_effect = [
                    {**trace(START, initial), 'active': True},
                    {**trace(START, *events), 'active': True},
                    trace(START, *events, ('stop', *events[-1][1:3], 0, 0))]

                workers = []
                def thread(target, daemon):
                    self.assertTrue(daemon)
                    worker = Mock(is_alive=Mock(side_effect=lambda: failure == 'join' and held[0]))
                    worker.start.side_effect = target
                    workers.append(worker)
                    return worker

                replacements = {
                    'provenance': Mock(return_value={}),
                    'DirectMCP': Mock(side_effect=[agent, observer]),
                    'subprocess.Popen': Mock(return_value=grab),
                    'primary_acknowledgement': Mock(side_effect=AssertionError('incomplete HELD'))
                        if failure == 'startup' else Mock(return_value='HELD\n'),
                    'select.select': Mock(return_value=([grab.stdout], [], [])),
                    'wait_for': lambda predicate: self.assertTrue(predicate()),
                    'state': lambda path: {'held': held[0], 'clicks': 0, 'keys': 0, 'scroll': 0},
                    'wm': lambda: {'pid': 10, 'address': '0x10', 'workspace': 1,
                                   'cursor': {'x': log[-1]['x'] if log else 100, 'y': 200}},
                    'Trace': Mock(return_value=trace_client), 'threading.Thread': thread,
                    'move_primary': movement, 'stop_process': lambda process: held.__setitem__(0, False),
                }
                for name, replacement in replacements.items():
                    stack.enter_context(patch('production_realapp_proof.' + name, replacement))
                self.assertEqual(run(args), 0 if during_action and failure is None else 1)
                if failure not in ('startup', 'motion'):
                    click_index = action_calls.index('click')
                    self.assertEqual(action_calls[click_index - 1:click_index + 2], ['snapshot', 'click', 'snapshot'])
                self.assertIn('controlled', replacements['subprocess.Popen'].call_args.args[0])
                report = json.loads((args.evidence / 'result.json').read_text())
                if during_action and failure is None:
                    self.assertEqual(report['continuous_isolation'], 'passed')
                    self.assertEqual(report['primary_commands_during_actions'], 1)
                    self.assertEqual(json.loads((args.evidence / 'expected-primary-motion.json').read_text()),
                                     [[120, 200], [140, 200]])
                    self.assertEqual(report['actions'][0]['no_dispatch'], 'verified')
                else:
                    self.assertEqual(report['continuous_isolation'], 'unproven')
                self.assertEqual(json.loads((args.evidence / 'primary-motion-commands.json').read_text()), log)
                self.assertFalse(held[0])
                agent.close.assert_called_once()
                observer.close.assert_called_once()
                if failure != 'startup':
                    trace_client.close.assert_called_once()
                    workers[0].join.assert_called_with(timeout=3)
                    self.assertEqual(workers[0].join.call_count, 2)
                if failure == 'join':
                    cleanup = json.loads((args.evidence / 'cleanup.json').read_text())
                    self.assertIn('stop_primary_motion', [row['operation'] for row in cleanup['errors']])
                    self.assertFalse(workers[0].is_alive())


class RuntimeTests(unittest.TestCase):
    def test_profiles_keep_managed_policy_and_remove_inherited_approval(self):
        inherited = {'CUA_DRIVER_PERMISSION_MODE': 'unrestricted',
                     'CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS': '1',
                     'CUA_DRIVER_SESSION_POLICY_FILE': 'old-manifest',
                     'CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED': '1',
                     'CUA_DRIVER_MANAGED_POLICY_FILE': 'managed.yaml'}
        env = profile_environment({'mode': 'standard'}, inherited)
        self.assertEqual(env, {'CUA_DRIVER_PERMISSION_MODE': 'standard',
                               'CUA_DRIVER_MANAGED_POLICY_FILE': 'managed.yaml'})

    def test_profile_matrix_uses_normal_environment_contract(self):
        with tempfile.NamedTemporaryFile() as manifest:
            for mode in ('standard', 'bounded', 'unrestricted'):
                profile = {'mode': mode, 'manifest': manifest.name, 'approve_manifest': True,
                           'acknowledge_unrestricted': True}
                env = profile_environment(profile, {})
                self.assertEqual(env['CUA_DRIVER_PERMISSION_MODE'], mode)
                self.assertEqual(env['CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED'], '1')
            for mode in ('standard', 'unrestricted'):
                profile_environment({'mode': mode, 'acknowledge_unrestricted': True}, {})
        for profile in ({'mode': 'bounded'}, {'mode': 'unrestricted'},
                        {'mode': 'standard', 'manifest': 'unreviewed.yaml'}):
            with self.assertRaises(AssertionError):
                profile_environment(profile, {})

    def test_distinct_labels_are_not_process_ownership(self):
        def client(pid):
            return SimpleNamespace(process=Mock(pid=pid, poll=Mock(return_value=None)))
        self.assertEqual(assert_distinct_runtimes([client(1), client(2)]), [1, 2])
        with self.assertRaises(AssertionError):
            assert_distinct_runtimes([client(1), client(1)])

    def test_owned_process_is_reaped_after_terminate_timeout(self):
        process = Mock()
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired('child', 5),
                                    subprocess.TimeoutExpired('child', 5), 0]
        stop_process(process)
        process.stdin.close.assert_called_once()
        process.terminate.assert_called_once()
        process.kill.assert_called_once()
        self.assertEqual(process.wait.call_count, 3)

    def test_initialization_failure_closes_owned_process(self):
        process = Mock()
        process.poll.return_value = 0
        with tempfile.TemporaryDirectory() as directory, \
                patch('production_mcp.subprocess.Popen', return_value=process) as spawn, \
                patch('driver_input_live.MCP.rpc', side_effect=TimeoutError('unknown')):
            with self.assertRaises(TimeoutError):
                DirectMCP(Path('/synthetic/driver'), Path(directory), {'mode': 'standard'})
            self.assertEqual(spawn.call_args.args[0], ['/synthetic/driver', 'mcp', '--direct'])
            process.wait.assert_called_once()

    def test_timeout_poison_prevents_replay(self):
        mcp = DirectMCP.__new__(DirectMCP)
        mcp.failed = mcp.closed = False
        with patch('driver_input_live.MCP.rpc', side_effect=TimeoutError('unknown')) as rpc:
            with self.assertRaises(TimeoutError):
                mcp.rpc('tools/call', {})
            with self.assertRaisesRegex(RuntimeError, 'do not replay'):
                mcp.rpc('tools/call', {})
            rpc.assert_called_once()

    def test_preflight_failure_is_retained_as_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            import json
            source_plan = root / 'plan.json'
            source_plan.write_text(json.dumps(plan()))
            args = SimpleNamespace(plan=source_plan, evidence=root / 'evidence')
            with patch('production_realapp_proof.provenance', side_effect=AssertionError('SHA mismatch')):
                self.assertEqual(run(args), 1)
            result = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(result['result'], 'failed')
            self.assertEqual(result['error'], 'SHA mismatch')
            self.assertTrue((args.evidence / 'cleanup.json').is_file())


if __name__ == '__main__':
    unittest.main()
