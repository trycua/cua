"""Synthetic policy/oracle tests only: no native input or managed host policy."""
import base64
import copy
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from production_app_smoke import GroundingUnavailable, create_documents
from production_app_smoke_test import CALC, INKSCAPE, INKSCAPE_SELECTED, changed_ods
from production_policy_proof import (
    CASES, DENIALS, MANAGED_ENV, SMOKE_STEPS, check_refusal, documents_for_case,
    expected_refusal, run, snapshot, start_client, validate_plan, verify_evidence,
)


def plan(case='resource_wrong_window', app='calc', mode='unrestricted'):
    suffix = 'calc.ods' if app == 'calc' else 'inkscape.svg'
    return {'case': case, 'app': app, 'mode': mode, 'disposable': True,
            'target': {'pid': 20, 'window_id': 200},
            'document': '/disposable/cua-smoke-' + suffix}


def page(events, active=True):
    return {'active': active, 'hook': True, 'overflow': False, 'timed_out': False,
            'count': len(events), 'events': copy.deepcopy(events)}


def window(candidate, stage, identity):
    source = CALC if candidate['app'] == 'calc' else (
        INKSCAPE_SELECTED if stage == 'move' else INKSCAPE)
    return {'structuredContent': {**copy.deepcopy(source),
            **candidate['target'],
            'window_title': Path(candidate['document']).name,
            'snapshot_id': f'snapshot-{identity}', 'screenshot_width': 800,
            'screenshot_height': 600, 'window_bounds': {'x': 0, 'y': 0, 'width': 800, 'height': 600}}}


def evidence(candidate):
    events = [[1, 0, 'start', 100, 100, 0, 0]]
    result = {'actions': []}
    stages = list(SMOKE_STEPS[candidate['app']])
    stages = ([stages[0]] if candidate['case'] in DENIALS else []) + stages
    for index, stage in enumerate(stages):
        denied = index == 0 and candidate['case'] in DENIALS
        tool, args = SMOKE_STEPS[candidate['app']][stage]
        before = page(events)
        if not denied:
            for kind, pressed in [('agent_admitted', 0), ('keyboard_key', 1),
                                  ('keyboard_key', 0), ('agent_action_end', 0)]:
                events.append([len(events) + 1, len(events), kind, 100, 100, 1, pressed])
        response = ({'isError': True, 'structuredContent': {
            'status': 'refused', 'refusal': expected_refusal(candidate)}} if denied else
            {'structuredContent': {'effect': 'unverifiable', 'route': 'synthetic_events',
                                   'delivery': {'mode': 'background'}}})
        result['actions'].append({'stage': stage, 'tool': tool, 'phase': 'deny' if denied else 'control',
            'mode': candidate.get('mode', 'unrestricted'),
            'primary_before': {'pid': 10, 'address': '0x64'},
            'configuration': list(documents_for_case(candidate, control=not denied)),
            'runtime_pid': 100 if denied else 101,
            'arguments': {**args, **candidate['target'], 'session': 'policy-proof',
                          'delivery_mode': 'background'},
            'times_ns': list(range(index * 4 + 1, index * 4 + 5)),
            'before': window(candidate, stage, index * 2),
            'after': window(candidate, stage, index * 2 + 1),
            'trace_before': before, 'trace_after': page(events), 'response': response})
    events.append([len(events) + 1, len(events), 'stop', 100, 100, 0, 0])
    result['trace'] = page(events, active=False)
    with tempfile.TemporaryDirectory() as temporary:
        documents = create_documents(Path(temporary))
        before = documents[candidate['app']].read_bytes()
    after = changed_ods(before) if candidate['app'] == 'calc' else before.replace(b'x="40"', b'x="42"')
    result.update(document_before=base64.b64encode(before).decode(),
                  document_after=base64.b64encode(after).decode())
    return result


class PolicyTests(unittest.TestCase):
    def test_native_window_addresses_use_u64_not_pid_range(self):
        for window_id in (0x55ABCDEF1234, 2**64 - 1):
            candidate = plan('resource_wrong_window')
            candidate['target']['window_id'] = window_id
            self.assertEqual(verify_evidence(candidate, evidence(candidate))['result'], 'passed')
            excluded = documents_for_case(candidate)[0]['resources']['desktop']['windows'][0]
            self.assertNotEqual(excluded['window_id'], window_id)
            self.assertLessEqual(excluded['window_id'], 2**64 - 1)
        for window_id in (True, 0, -1, 2**64):
            candidate = plan()
            candidate['target']['window_id'] = window_id
            with self.subTest(window_id=window_id), self.assertRaises(AssertionError):
                validate_plan(candidate)

    def test_exact_resource_and_managed_matrix_all_profiles(self):
        for mode in ('standard', 'bounded', 'unrestricted'):
            for app in ('calc', 'inkscape'):
                for case in CASES:
                    candidate = plan(case, app, mode)
                    with self.subTest(case=case, app=app, mode=mode):
                        if case == 'no_manifest_allow' and mode == 'bounded':
                            with self.assertRaisesRegex(AssertionError, 'bounded'):
                                validate_plan(candidate)
                            continue
                        self.assertEqual(verify_evidence(candidate, evidence(candidate))['result'], 'passed')
                        manifest, managed = documents_for_case(candidate)
                        if case == 'no_manifest_allow':
                            self.assertIsNone(manifest)
                            self.assertIsNone(managed)
                            continue
                        target = manifest['resources']['desktop']['windows'][0]
                        self.assertEqual(target['pid'], 21 if case == 'resource_wrong_pid' else 20)
                        self.assertEqual(target['window_id'], 201 if case == 'resource_wrong_window' else 200)
                        self.assertEqual('deny' in (managed or {}), case == 'managed_deny')
                        self.assertEqual(manifest['version'], 3)
                        self.assertTrue(manifest['expires_after'] and manifest['idle_timeout'])

    def test_plan_rejects_scope_escape_and_non_synthetic_document(self):
        mutations = [lambda p: p.update(disposable=False), lambda p: p.update(case='arbitrary'),
            lambda p: p.update(mode='autonomous'), lambda p: p.update(app='terminal'),
            lambda p: p.update(profile={'manifest': '/host-policy'}),
            lambda p: p.update(arguments={'scope': 'desktop'}),
            lambda p: p.update(document='/business/file.ods'),
            lambda p: p.update(document='cua-smoke-calc.ods'),
            lambda p: p['target'].update(pid=True), lambda p: p['target'].update(window_id=0),
            lambda p: p['target'].update(delivery_mode='foreground')]
        for mutate in mutations:
            candidate = plan()
            mutate(candidate)
            with self.subTest(candidate=candidate), self.assertRaises(AssertionError):
                validate_plan(candidate)

    def test_source_exact_envelope_and_admission_boundary(self):
        for case in DENIALS:
            expected = expected_refusal(plan(case))
            core = {'isError': True, 'structuredContent': {'status': 'refused', 'refusal': expected}}
            check_refusal(core, expected)
            invalid = [dict(core, isError=False),
                dict(core, structuredContent={'effect': 'refused', 'reason': expected['code']}),
                dict(core, structuredContent={**core['structuredContent'], 'delivery': {'mode': 'background'}}),
                dict(core, structuredContent={'status': 'refused', 'refusal': {**expected, 'code': 'lane_busy'}}),
                dict(core, content=[{'type': 'text', 'text': 'Permission denied'}])]
            for result in invalid:
                with self.subTest(case=case, result=result), self.assertRaises(AssertionError):
                    check_refusal(result, expected)
        expected = expected_refusal(plan('managed_deny'))
        flat = {'isError': True, 'structuredContent': {'code': 'permission_denied'},
                'content': [{'type': 'text', 'text': expected['message']}]}
        self.assertEqual(check_refusal(flat, expected), 'mcp-tool-admission')
        flat['content'][0]['text'] = expected['message'].replace('managed', 'user')
        with self.assertRaises(AssertionError):
            check_refusal(flat, expected)

    def test_policy_injection_is_process_local_restored_and_never_overwrites_host_ceiling(self):
        candidate = plan('managed_deny')
        manifest, managed = documents_for_case(candidate)
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {}, clear=True):
            directory = Path(temporary)
            def spawned(driver, path, profile):
                self.assertEqual(os.environ[MANAGED_ENV], str((directory / 'managed.yaml').resolve()))
                self.assertEqual(json.loads((directory / 'managed.yaml').read_text()), managed)
                self.assertEqual(json.loads(Path(profile['manifest']).read_text()), manifest)
                self.assertEqual(profile['mode'], 'unrestricted')
                self.assertTrue(profile['acknowledge_unrestricted'] and profile['approve_manifest'])
                raise RuntimeError('synthetic spawn failure')
            with patch('production_policy_proof.DirectMCP', side_effect=spawned):
                with self.assertRaisesRegex(RuntimeError, 'spawn failure'):
                    start_client(Path('/driver'), directory, 'unrestricted', manifest, managed)
            self.assertNotIn(MANAGED_ENV, os.environ)
            for name in (MANAGED_ENV, 'CUA_DRIVER_POLICY_FILE'):
                with patch.dict(os.environ, {name: '/never-read-host-policy'}), \
                        patch('production_policy_proof.DirectMCP') as spawn:
                    with self.assertRaisesRegex(AssertionError, 'inherited'):
                        start_client(Path('/driver'), directory, 'unrestricted', manifest, managed)
                    spawn.assert_not_called()
                    self.assertEqual(os.environ[name], '/never-read-host-policy')


class OracleTests(unittest.TestCase):
    def test_only_inkscape_snapshots_bound_visited_nodes_and_keep_images_and_depth(self):
        for app in ('calc', 'inkscape'):
            for stage in SMOKE_STEPS[app]:
                for before in (True, False):
                    with self.subTest(app=app, stage=stage, before=before):
                        candidate = plan(app=app)
                        client = Mock()
                        expected = window(candidate, stage, 1)
                        client.tool.side_effect = [
                            {'structuredContent': {'windows': [candidate['target']]}}, expected]
                        self.assertEqual(snapshot(client, candidate, stage=stage, before=before), expected)
                        arguments = {**candidate['target'], 'session': 'policy-proof'}
                        if app == 'inkscape' and not (stage == 'select' and before):
                            arguments['max_elements'] = 2500
                        client.tool.assert_called_with('get_window_state', arguments)

    def test_bounded_projection_cannot_omit_selection_grounding(self):
        candidate = plan('resource_allow', app='inkscape')
        for stage in ('select', 'move'):
            value = evidence(candidate)
            row = next(row for row in value['actions'] if row['stage'] == stage)
            row['before']['structuredContent']['tree_markdown'] = ''
            with self.subTest(stage=stage), self.assertRaises(GroundingUnavailable):
                verify_evidence(candidate, value)

    def test_rejects_tampered_result_trace_snapshots_configuration_and_output(self):
        mutations = {
            'dropped_action': lambda e: e['actions'].pop(),
            'wrong_target': lambda e: e['actions'][0]['arguments'].update(window_id=201),
            'wrong_tool': lambda e: e['actions'][0].update(tool='click'),
            'wrong_profile': lambda e: e['actions'][0]['configuration'][0]['resources']['desktop'].update(windows=[]),
            'wrong_mode': lambda e: e['actions'][0].update(mode='standard'),
            'refusal_code': lambda e: e['actions'][0]['response']['structuredContent']['refusal'].update(code='permission_denied'),
            'refusal_message': lambda e: e['actions'][0]['response']['structuredContent']['refusal'].update(message='denied'),
            'false_error_success': lambda e: e['actions'][1]['response'].update(isError=True),
            'partial': lambda e: e['actions'][1]['response']['structuredContent'].update(effect='partial'),
            'other_route': lambda e: e['actions'][1]['response']['structuredContent'].update(route='accessibility'),
            'same_runtime': lambda e: e['actions'][0].update(runtime_pid=101),
            'changed_runtime': lambda e: e['actions'][2].update(runtime_pid=102),
            'stale_snapshot': lambda e: e['actions'][0].update(after=e['actions'][0]['before']),
            'wrong_document': lambda e: e['actions'][0]['before']['structuredContent'].update(window_title='other'),
            'wrong_snapshot_pid': lambda e: e['actions'][0]['before']['structuredContent'].update(pid=21),
            'wrong_snapshot_window': lambda e: e['actions'][0]['after']['structuredContent'].update(window_id=201),
            'foreground_target': lambda e: e['actions'][0]['primary_before'].update(pid=20),
            'missing_image': lambda e: e['actions'][0]['before']['structuredContent'].update(screenshot_width=0),
            'unordered_time': lambda e: e['actions'][0]['times_ns'].__setitem__(3, 0),
            'trace_overflow': lambda e: e['trace'].update(overflow=True),
            'trace_timeout': lambda e: e['actions'][0]['trace_after'].update(timed_out=True),
            'trace_incomplete': lambda e: e['trace']['events'].pop(),
            'changed_prefix': lambda e: e['actions'][1]['trace_after']['events'][1].__setitem__(3, 500),
            'no_saved_effect': lambda e: e.update(document_after=e['document_before']),
        }
        candidate = plan()
        good = evidence(candidate)
        for name, mutate in mutations.items():
            bad = copy.deepcopy(good)
            mutate(bad)
            with self.subTest(name=name), self.assertRaises(AssertionError):
                verify_evidence(candidate, bad)

    def test_synthetic_dispatch_in_denial_rejected_even_with_consistent_trace_prefixes(self):
        candidate = plan()
        bad = evidence(candidate)
        # Move the quiet interval boundary past the first positive-control
        # admission, preserving a complete internally consistent trace.
        bad['actions'][0]['trace_after'] = page(bad['trace']['events'][:2])
        with self.assertRaisesRegex(AssertionError, 'denied call reached'):
            verify_evidence(candidate, bad)

    def test_no_dispatch_flag_cannot_replace_raw_trace(self):
        candidate = plan()
        bad = evidence(candidate)
        bad['actions'][0].update(no_dispatch='verified', trace_after={})
        with self.assertRaises(AssertionError):
            verify_evidence(candidate, bad)

    def test_calc_preexisting_expected_text_cannot_pass_as_an_input_effect(self):
        candidate = plan('resource_allow')
        bad = evidence(candidate)
        # A rewrite of container bytes must not be enough if A1 already had a.
        bad['document_before'] = bad['document_after']
        with self.assertRaisesRegex(AssertionError, 'initial A1 is not blank'):
            verify_evidence(candidate, bad)

    def test_exact_window_discovery_precedes_snapshot(self):
        client = Mock()
        client.tool.return_value = {'structuredContent': {'windows': [
            {'pid': 20, 'window_id': 200}, {'pid': 20, 'window_id': 201}]}}
        with self.assertRaisesRegex(AssertionError, 'stale or ambiguous'):
            snapshot(client, plan())
        self.assertEqual([call.args[0] for call in client.tool.call_args_list], ['list_windows'])

    def test_failed_provenance_cannot_start_native_process_or_trace(self):
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {}, clear=True):
            directory = Path(temporary)
            path = directory / 'plan.json'
            path.write_text(json.dumps(plan()))
            args = SimpleNamespace(plan=path, evidence=directory / 'evidence')
            with patch('production_policy_proof.provenance', side_effect=AssertionError('source differs')), \
                    patch('production_policy_proof.DirectMCP') as spawn, \
                    patch('production_policy_proof.Trace') as trace:
                self.assertEqual(run(args), 1)
                spawn.assert_not_called()
                trace.assert_not_called()
            report = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(report['result'], 'failed')
            self.assertIn('source differs', report['error']['message'])


class OrchestrationTests(unittest.TestCase):
    def exercise(self, directory, failure=None, *, app='calc', case='managed_deny',
                 missing_grounding=None):
        candidate = plan(case, app=app)
        document = create_documents(directory)[app]
        candidate['document'] = str(document.resolve())
        path = directory / 'plan.json'
        path.write_text(json.dumps(candidate))
        args = SimpleNamespace(plan=path, evidence=directory / 'evidence', driver=Path('/driver'),
                               trace_socket=Path('/disposable/trace'))
        events, clients, calls = [], [], []

        class FakeTrace:
            active = False
            closed = False

            def collect(self):
                return page(events, self.active)

            def exchange(self, command):
                calls.append(('trace', command))
                self.active = command == 'TRACE_START'
                events.append([len(events) + 1, len(events),
                               'start' if self.active else 'stop', 100, 100, 0, 0])

            def close(self):
                self.closed = True

        class FakeClient:
            def __init__(self, driver, path, profile):
                self.directory, self.name, self.profile = path, path.name, profile
                self.process = SimpleNamespace(pid=100 + len(clients))
                self.failed, self.closed, self.counter = False, False, 0
                self.snapshots = []
                clients.append(self)

            def tool(self, tool, arguments):
                calls.append((self.name, tool))
                self.counter += 1
                if tool == 'list_windows':
                    assert self.name == 'observer', 'window-only manifest cannot enumerate application'
                    return {'structuredContent': {'windows': [candidate['target']]}}
                if tool == 'get_window_state':
                    stage = list(SMOKE_STEPS[app])[
                        len(self.snapshots) // 2 if self.name == 'control' else 0]
                    self.snapshots.append(dict(arguments))
                    result = window(candidate, stage, f'{self.name}-{self.counter}')
                    if missing_grounding == (self.name, stage):
                        result['structuredContent'].update(elements=[], tree_markdown='')
                    return result
                if failure == 'unknown':
                    self.failed = True
                    raise RuntimeError('unknown outcome; no replay')
                if self.name == 'denied':
                    if failure == 'unexpected_dispatch':
                        events.append([len(events) + 1, len(events), 'agent_admitted', 100, 100, 1, 0])
                    return {'isError': True, 'structuredContent': {'code': 'permission_denied'},
                            'content': [{'type': 'text', 'text': expected_refusal(candidate)['message']}]}
                for kind, pressed in [('agent_admitted', 0), ('keyboard_key', 1),
                                      ('keyboard_key', 0), ('agent_action_end', 0)]:
                    events.append([len(events) + 1, len(events), kind, 100, 100, 1, pressed])
                if tool == 'hotkey' and arguments['keys'] == ['ctrl', 's']:
                    before = document.read_bytes()
                    document.write_bytes(changed_ods(before) if app == 'calc' else
                                         before.replace(b'x="40"', b'x="42"'))
                return {'structuredContent': {'effect': 'unverifiable', 'route': 'synthetic_events',
                                               'delivery': {'mode': 'background'}}}

            def close(self):
                self.closed = True

        trace = FakeTrace()
        with patch.dict(os.environ, {}, clear=True), \
                patch('production_policy_proof.provenance', return_value={}), \
                patch('production_policy_proof.app_process_identity', return_value={}), \
                patch('production_policy_proof.wm', return_value={
                    'pid': 20 if failure == 'foreground_target' else 10, 'address': '0x64'}), \
                patch('production_policy_proof.subprocess.check_output',
                      return_value=json.dumps([{'pid': 20, 'address': '0xc8', 'xwayland': False}])), \
                patch('production_policy_proof.Trace', return_value=trace), \
                patch('production_policy_proof.DirectMCP', FakeClient):
            status = run(args)
            self.assertNotIn(MANAGED_ENV, os.environ)
        self.assertTrue(trace.closed and not trace.active)
        self.assertTrue(all(client.closed for client in clients))
        return status, json.loads((args.evidence / 'result.json').read_text()), calls, clients

    def test_inkscape_select_uses_default_snapshot_in_denial_and_control(self):
        for case in ('managed_deny', 'resource_allow'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as temporary:
                status, report, calls, clients = self.exercise(
                    Path(temporary), app='inkscape', case=case)
            self.assertEqual(status, 0, report.get('error'))
            default = {**report['plan']['target'], 'session': 'policy-proof'}
            bounded = {**default, 'max_elements': 2500}
            observed = {client.name: client.snapshots for client in clients}
            self.assertEqual(observed['control'], [default] + [bounded] * 5)
            self.assertEqual(observed['observer'],
                             [default, bounded] if case == 'managed_deny' else [])
            for index, (name, tool) in enumerate(calls):
                if tool not in ('press_key', 'hotkey'):
                    continue
                observer = 'observer' if name == 'denied' else name
                self.assertEqual(calls[index - 1], (observer, 'get_window_state'))
                self.assertEqual(calls[index + 1:index + 3],
                                 [('observer', 'list_windows'), (observer, 'get_window_state')])

    def test_inkscape_missing_grounding_stops_before_dispatch_without_retry(self):
        for missing, expected_actions in ((('observer', 'select'), []),
                (('control', 'select'), [('denied', 'hotkey')]),
                (('control', 'move'), [('denied', 'hotkey'), ('control', 'hotkey')])):
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as temporary:
                status, report, calls, clients = self.exercise(
                    Path(temporary), app='inkscape', missing_grounding=missing)
            self.assertEqual(status, 1)
            self.assertEqual(report['error']['type'], 'GroundingUnavailable')
            self.assertEqual([(name, tool) for name, tool in calls
                              if tool in ('press_key', 'hotkey')], expected_actions)
            self.assertNotIn('response', report['actions'][-1])

    def test_normal_direct_cell_checks_denial_then_saved_effect_and_closes_every_runtime(self):
        with tempfile.TemporaryDirectory() as temporary:
            status, report, calls, clients = self.exercise(Path(temporary))
        self.assertEqual(status, 0, report.get('error'))
        self.assertEqual(report['verification']['positive_control']['text'], 'a')
        self.assertEqual([client.name for client in clients], ['observer', 'denied', 'control'])
        self.assertTrue(all(client.profile['mode'] == 'unrestricted' for client in clients))
        actions = [(name, tool) for name, tool in calls if tool in ('press_key', 'hotkey')]
        self.assertEqual(actions, [('denied', 'press_key'), ('control', 'press_key'),
                                   ('control', 'press_key'), ('control', 'hotkey')])
        for index, (name, tool) in enumerate(calls):
            if tool not in ('press_key', 'hotkey'):
                continue
            observer = 'observer' if name == 'denied' else name
            self.assertEqual(calls[index - 1], (observer, 'get_window_state'))
            self.assertEqual(calls[index + 1:index + 3],
                             [('observer', 'list_windows'), (observer, 'get_window_state')])

    def test_unknown_or_wrong_refusal_stops_before_control_and_retains_after_snapshot(self):
        for failure in ('unknown', 'unexpected_dispatch'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temporary:
                status, report, calls, clients = self.exercise(Path(temporary), failure)
            self.assertEqual(status, 1)
            self.assertEqual([client.name for client in clients], ['observer', 'denied'])
            self.assertEqual(sum(tool == 'press_key' for _, tool in calls), 1)
            self.assertIn('after', report['actions'][0])
            self.assertEqual(calls[-1], ('trace', 'TRACE_STOP'))
            if failure == 'unknown':
                self.assertIn('unknown outcome', report['error']['message'])
            else:
                self.assertIn('denied call reached', report['error']['message'])

    def test_foreground_target_refuses_before_any_input(self):
        with tempfile.TemporaryDirectory() as temporary:
            status, report, calls, clients = self.exercise(Path(temporary), 'foreground_target')
        self.assertEqual(status, 1)
        self.assertFalse(any(tool in ('press_key', 'hotkey') for _, tool in calls))
        self.assertIn('target is the primary client', report['error']['message'])


if __name__ == '__main__':
    unittest.main()
