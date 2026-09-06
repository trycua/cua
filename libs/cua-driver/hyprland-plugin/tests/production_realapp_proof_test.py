import copy
import io
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import zipfile

from production_mcp import DirectMCP, assert_distinct_runtimes, profile_environment, stop_process
from production_realapp_proof import assert_no_dispatch, check_response, run, validate_plan, verify_output
from primary_trace_test import START, trace


def plan():
    return {'purpose': 'apps', 'foreground': {'pid': 10, 'window_id': 100},
            'agents': [{'app': 'calc', 'target': {'pid': 20, 'window_id': 200}},
                       {'app': 'inkscape', 'target': {'pid': 30, 'window_id': 300}}],
            'phases': [{'parallel': [{'agent': 0, 'tool': 'drag', 'arguments': {}},
                                     {'agent': 1, 'tool': 'drag', 'arguments': {}}]}],
            'outputs': [{'agent': 0, 'attributes': {'value': '96'}},
                        {'agent': 1, 'rect_translation': [[10, 20], [30, 40]]}]}


class PlanTests(unittest.TestCase):
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
