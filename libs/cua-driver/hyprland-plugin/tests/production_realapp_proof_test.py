import copy
from contextlib import ExitStack
import io
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import zipfile

from production_mcp import DirectMCP, assert_distinct_runtimes, profile_environment, stop_process
from production_realapp_proof import (SMOKE_STEPS, app_process_identity, provenance,
                                    assert_no_dispatch, assert_primary_state, check_response,
                                    capacity_lane, verify_capacity, check_manifest_refusal,
                                    manifest_tool_messages, verify_policy_cache,
                                    passive_focus_evidence,
                                    expected_primary_motion, move_primary, primary_acknowledgement,
                                    parallel_actions, primary_trajectory, require_primary_active,
                                    PRIMARY_LIFETIME_MS, POINTER_EPISODES, run, validate_plan, verify_output)
from primary_trace import analyze
from primary_trace_test import START, STOP, trace
from production_app_smoke_test import INKSCAPE, INKSCAPE_SELECTED


class PassiveFocusTests(unittest.TestCase):
    def test_passive_focus_retains_presence_without_authority_after_close(self):
        rows = [[1, 0, 'start', 100, 100, 0, 0],
                [2, 1, 'pointer_enter', 100, 100, 1, 0, 10, 20],
                [3, 2, 'pointer_motion', 100, 100, 1, 0, 10, 20],
                [4, 3, 'agent_action_end', 100, 100, 1, 0]]
        page = {'hook': True, 'active': True, 'overflow': False, 'timed_out': False,
                'count': len(rows), 'events': rows}
        values = [{'lane': lane, 'epoch': f'lane-{lane}', 'desktop_generation': 1,
                   'reserved': lane == 0, 'pointer_focus': lane == 0,
                   'keyboard_focus': False, 'lease_active': False, 'drag_active': False,
                   'held_button': 0, 'held_keys': 0} for lane in (0, 1)]
        status = {'state': 'input_v3_candidate', 'input': {'protocol': 3, 'test_only': False,
                   'transport_ready': True, 'lanes': values}}
        before = {'status': status, 'trace': page}
        after = {'status': copy.deepcopy(status)}
        after['status']['input']['lanes'][0].update(reserved=False)
        stopped = {**page, 'active': False, 'count': len(rows) + 1,
                   'events': rows + [[5, 4, 'stop', 100, 100, 0, 0]]}
        def append_synthetic(t, kind):
            t['events'].insert(-1, [5, 4, kind, 100, 100, 1, 0])
            t['events'][-1][0] = 6
            t['count'] += 1
        mutations = {
            'grant': lambda b, a, t: b['status']['input']['lanes'][0].update(lease_active=True),
            'held': lambda b, a, t: b['status']['input']['lanes'][0].update(held_button=272),
            'keyboard': lambda b, a, t: b['status']['input']['lanes'][0].update(keyboard_focus=True),
            'not_retained': lambda b, a, t: b['status']['input']['lanes'][0].update(pointer_focus=False),
            'reservation': lambda b, a, t: a['status']['input']['lanes'][0].update(reserved=True),
            'hover_lost': lambda b, a, t: a['status']['input']['lanes'][0].update(pointer_focus=False),
            'held_after_close': lambda b, a, t: a['status']['input']['lanes'][0].update(held_keys=1),
            'epoch': lambda b, a, t: a['status']['input']['lanes'][0].update(epoch='replaced'),
            'unexpected_leave': lambda b, a, t: append_synthetic(t, 'pointer_leave'),
            'new_input': lambda b, a, t: append_synthetic(t, 'agent_admitted'),
            'incomplete': lambda b, a, t: t.update(overflow=True),
        }
        for mutation in (None, *mutations):
            b, a, t = copy.deepcopy((before, after, stopped))
            with self.subTest(mutation=mutation):
                if mutation:
                    mutations[mutation](b, a, t)
                    with self.assertRaises(AssertionError):
                        passive_focus_evidence(b, a, t)
                else:
                    self.assertEqual(passive_focus_evidence(b, a, t)['lanes'], [1])


def plan():
    return {'purpose': 'apps', 'foreground': {'pid': 10, 'window_id': 100},
            'agents': [{'app': 'calc', 'target': {'pid': 20, 'window_id': 200}},
                       {'app': 'inkscape', 'target': {'pid': 30, 'window_id': 300}}],
            'phases': [{'parallel': [{'agent': 0, 'tool': 'drag', 'arguments': {}},
                                     {'agent': 1, 'tool': 'drag', 'arguments': {}}]}],
            'outputs': [{'agent': 0, 'attributes': {'value': '96'}},
                        {'agent': 1, 'rect_translation': [[10, 20], [30, 40]]}]}


def capacity_plan():
    result = {**plan(), 'purpose': 'capacity', 'outputs': []}
    result['agents'].append({'app': 'inkscape', 'target': {'pid': 40, 'window_id': 400}})
    for index, spec in enumerate(result['agents']):
        spec.update(name='same-public-name', profile={'mode': 'standard'},
                    bounds={'x': 0, 'y': 0, 'width': 600, 'height': 600})
    result['phases'] = [{'agent': index, 'tool': 'click', 'arguments': {'x': 20, 'y': 30}}
                        for index in range(3)]
    result['phases'][2]['expect'] = {'kind': 'refused', 'reason': 'lane_busy'}
    return result


def capacity_events(lane):
    return [('agent_admitted', 100, 200, lane, 0),
            ('pointer_button', 100, 200, lane, 1),
            ('pointer_button', 100, 200, lane, 0),
            ('agent_action_end', 100, 200, lane, 0)]


def policy_cache_plan():
    result = capacity_plan()
    result.update(purpose='policy_cache', agents=result['agents'][:1])
    result['agents'][0]['profile'].update(manifest='reviewed.yaml', approve_manifest=True)
    result['phases'] = [
        {'agent': 0, 'tool': 'click', 'arguments': {'x': 20, 'y': 30}},
        {'agent': 0, 'tool': 'press_key', 'arguments': {'key': 'ESC'},
         'expect': {'kind': 'refused', 'reason': 'permission_denied',
                    'message': "Permission denied: capability manifest denies tool 'press_key'"}},
        {'agent': 0, 'tool': 'click', 'arguments': {'x': 30, 'y': 40}},
    ]
    return result


class ProvenanceTests(unittest.TestCase):
    def test_runtime_provenance_refusal_precedes_app_inspection(self):
        with patch('production_realapp_proof.runtime_provenance',
                   side_effect=AssertionError('loaded artifact mismatch')) as runtime, \
                patch('production_realapp_proof.subprocess.check_output') as command:
            args = SimpleNamespace()
            with self.assertRaisesRegex(AssertionError, 'loaded artifact mismatch'):
                provenance(args, {})
            runtime.assert_called_once_with(args)
            command.assert_not_called()

    def test_reviewed_packages_must_match_runtime_provenance(self):
        with patch('production_realapp_proof.runtime_provenance', return_value={'packages': {'a': '1'}}), \
                patch('production_realapp_proof.subprocess.check_output') as command:
            with self.assertRaisesRegex(AssertionError, 'package qualification mismatch'):
                provenance(SimpleNamespace(), {'package_versions': {'a': '2'}})
            command.assert_not_called()

    def test_canonical_app_identity_requires_owner_and_native_backend(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            executable = root / 'soffice.bin'
            executable.write_bytes(b'synthetic executable')
            process = root / '20'
            process.mkdir()
            (process / 'exe').symlink_to(executable)
            (process / 'maps').write_text('/usr/lib/libgtk-3.so\n/usr/lib/libvclplug_gtk3lo.so\n')
            with patch('production_realapp_proof.EXECUTABLES', {'calc': executable}), \
                    patch('production_realapp_proof.package_owner') as owner:
                identity = app_process_identity('calc', 20, root)
                owner.assert_called_once_with(executable, 'libreoffice-fresh')
                self.assertEqual(identity['executable'], str(executable))
                self.assertEqual(len(identity['gtk3_maps']), 2)
                self.assertEqual(len(identity['sha256']), 64)
                owner.side_effect = AssertionError('unowned executable')
                with self.assertRaisesRegex(AssertionError, 'unowned executable'):
                    app_process_identity('calc', 20, root)

            with patch('production_realapp_proof.EXECUTABLES', {'calc': root / 'canonical/soffice.bin'}), \
                    patch('production_realapp_proof.package_owner') as owner:
                with self.assertRaisesRegex(AssertionError, 'noncanonical running executable'):
                    app_process_identity('calc', 20, root)
                owner.assert_not_called()

            with patch('production_realapp_proof.EXECUTABLES', {'calc': executable}), \
                    patch('production_realapp_proof.package_owner'):
                for maps, reason in [('', 'did not load GTK3'),
                                     ('/usr/lib/libgtk-3.so', 'did not load its GTK3 backend')]:
                    (process / 'maps').write_text(maps)
                    with self.assertRaisesRegex(AssertionError, reason):
                        app_process_identity('calc', 20, root)


class PolicyCacheTests(unittest.TestCase):
    def test_plan_requires_one_fixed_target_session_and_exact_allow_deny_allow(self):
        validate_plan(policy_cache_plan())
        for mode in ('standard', 'bounded', 'unrestricted'):
            for message in manifest_tool_messages('press_key'):
                candidate = policy_cache_plan()
                candidate['agents'][0]['profile'].update(mode=mode, acknowledge_unrestricted=True)
                candidate['phases'][1]['expect']['message'] = message
                validate_plan(candidate)
        for mutate in (lambda p: p['agents'].append(copy.deepcopy(p['agents'][0])),
                       lambda p: p['agents'][0].update(app='fixture'),
                       lambda p: p['agents'][0].update(name=''),
                       lambda p: p['agents'][0]['target'].update(window_id=None),
                       lambda p: p['agents'][0]['profile'].pop('manifest'),
                       lambda p: p['agents'][0]['profile'].update(approve_manifest=False),
                       lambda p: p['agents'][0]['profile'].update(mode='unrestricted'),
                       lambda p: p['phases'].pop(),
                       lambda p: p['phases'][1].update(agent=1),
                       lambda p: p['phases'][1].update(parallel=[]),
                       lambda p: p['phases'][1].update(tool='click'),
                       lambda p: p['phases'][2].update(tool='scroll'),
                       lambda p: p['phases'][2].update(expect={'kind': 'unknown'}),
                       lambda p: p['phases'][1]['expect'].update(reason='lane_busy'),
                       lambda p: p['phases'][1]['expect'].update(reason='bounded_resource_outside_manifest'),
                       lambda p: p['phases'][1]['expect'].pop('message'),
                       lambda p: p['phases'][1]['expect'].update(message='Permission denied'),
                       lambda p: p['phases'][1]['arguments'].update(pid=99),
                       lambda p: p['phases'][1]['arguments'].update(session='new'),
                       lambda p: p.update(moving_primary=True),
                       lambda p: p.update(require_overlap=True)):
            bad = policy_cache_plan()
            mutate(bad)
            with self.subTest(plan=bad), self.assertRaises(AssertionError):
                validate_plan(bad)

    def test_exact_common_envelope_excludes_plugin_and_other_policy_refusals(self):
        expected = policy_cache_plan()['phases'][1]['expect']
        content = {'status': 'refused', 'refusal': {'code': expected['reason'], 'message': expected['message']}}
        check_manifest_refusal({'isError': True, 'structuredContent': content}, expected, 'press_key')
        for change in ({'effect': 'refused', 'reason': 'permission_denied'},
                       {**content, 'delivery': {'mode': 'background'}},
                       {**content, 'refusal': {**content['refusal'], 'message': "Permission denied: user policy: tool 'press_key' is explicitly denied"}},
                       {**content, 'refusal': {**content['refusal'], 'code': 'bounded_resource_outside_manifest'}},
                       {**content, 'refusal': {**content['refusal'], 'message': "Permission denied: capability manifest denies tool 'click'"}}):
            with self.subTest(content=change), self.assertRaises(AssertionError):
                check_manifest_refusal({'isError': True, 'structuredContent': change}, expected, 'press_key')

    def test_summary_requires_same_runtime_session_lane_and_fresh_success(self):
        expected = policy_cache_plan()['phases'][1]['expect']
        success = {'route': 'synthetic_events', 'effect': 'unverifiable', 'delivery': {'mode': 'background'}}
        actions = [dict(agent=0, tool=step['tool'], expected='dispatched', observed=success,
                        runtime_pid=101, session='fixed', compositor_lane=1)
                   for step in policy_cache_plan()['phases']]
        actions[1].update(expected='refused', expect=expected, no_dispatch='verified',
                          observed={'status': 'refused', 'refusal': {'code': expected['reason'], 'message': expected['message']}})
        self.assertEqual(verify_policy_cache(actions)['result'], 'verified')
        for mutate in (lambda a: a.pop(), lambda a: a[2].update(agent=1),
                       lambda a: a[2].update(compositor_lane=2),
                       lambda a: a[2].update(runtime_pid=102),
                       lambda a: a[2].update(session='replacement'),
                       lambda a: a[2].update(expected='unknown'),
                       lambda a: a[1].update(no_dispatch='unproven')):
            bad = copy.deepcopy(actions)
            mutate(bad)
            with self.subTest(actions=bad), self.assertRaises(AssertionError):
                verify_policy_cache(bad)

    def test_mcp_tool_admission_requires_exact_code_and_retained_text(self):
        expected = policy_cache_plan()['phases'][1]['expect']
        response = {'isError': True, 'structuredContent': {'code': 'permission_denied'},
                    'content': [{'type': 'text', 'text': expected['message']}]}
        result = check_manifest_refusal(response, expected, 'press_key')
        self.assertEqual(result['refusal_boundary'], 'mcp-tool-admission')
        self.assertEqual(result['mcp_content'], response['content'])
        for mutate in (lambda r: r.update(isError=False), lambda r: r.pop('content'),
                       lambda r: r['content'].append({'type': 'text', 'text': 'extra'}),
                       lambda r: r['content'][0].update(text='Permission denied'),
                       lambda r: r['content'][0].update(text=expected['message'].replace('press_key', 'hotkey')),
                       lambda r: r['structuredContent'].update(code='lane_busy'),
                       lambda r: r['structuredContent'].update(delivery={'mode': 'background'}),
                       lambda r: r['structuredContent'].update(effect='unverifiable')):
            bad = copy.deepcopy(response)
            mutate(bad)
            with self.subTest(response=bad), self.assertRaises(AssertionError):
                check_manifest_refusal(bad, expected, 'press_key')

    def test_missing_trace_fails_before_process_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / 'plan.json'
            path.write_text(json.dumps(policy_cache_plan()))
            args = SimpleNamespace(plan=path, evidence=root / 'evidence', trace_socket=None)
            with patch('production_realapp_proof.provenance') as provenance, \
                    patch('production_realapp_proof.DirectMCP') as spawn:
                self.assertEqual(run(args), 1)
                provenance.assert_not_called()
                spawn.assert_not_called()
            result = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(result['error'], 'policy_cache requires continuous trace')
            self.assertEqual(result['policy_cache']['result'], 'unproven')

    def test_runner_persistent_runtime_quiet_denial_and_failures_without_replay(self):
        for failure in (None, 'changed_lane', 'missing_input', 'wrong_refusal', 'plugin_refusal',
                        'admitted_on_denial', 'input_on_denial', 'denial_before_snapshot',
                        'denial_after_snapshot', 'next_snapshot_input', 'gap_input', 'dead_runtime', 'changed_runtime',
                        'transport', 'partial', 'unknown', 'missing_hook', 'reset_trace',
                        'cleanup_reset', 'foreground_input', 'cleanup_failure', 'stale_window'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                root = Path(directory)
                candidate = policy_cache_plan()
                path = root / 'plan.json'
                path.write_text(json.dumps(candidate))
                args = SimpleNamespace(plan=path, evidence=root / 'evidence',
                                       trace_socket=root / 'cua-input-v3.sock', driver=root / 'driver',
                                       primary_grab=root / 'primary-grab', foreground_journal=root / 'journal',
                                       record_video=False)
                calls, events, held = [], [START], [True]
                agent = Mock(process=Mock(pid=101, poll=Mock(return_value=None)))
                observer = Mock()
                targets = [candidate['foreground'], candidate['agents'][0]['target']]
                action_count = 0
                snapshot_count = 0

                def tool(name, arguments):
                    nonlocal action_count, snapshot_count
                    if name == 'list_windows':
                        windows = copy.deepcopy(targets)
                        if failure == 'stale_window' and action_count == 1:
                            windows[1]['window_id'] += 1
                        return {'structuredContent': {'windows': windows}}
                    if name == 'get_window_state':
                        calls.append(('snapshot', arguments))
                        snapshot_count += 1
                        if ((failure == 'denial_before_snapshot' and snapshot_count == 7)
                                or (failure == 'next_snapshot_input' and snapshot_count == 9)
                                or (failure == 'denial_after_snapshot' and action_count == 2)):
                            events.append(('agent_admitted', 100, 200, 1, 0))
                        return {'structuredContent': {'screenshot_width': 600,
                                'window_bounds': candidate['agents'][0]['bounds']}}
                    if name == 'get_desktop_state':
                        return {'structuredContent': {'screen_width': 800, 'screen_height': 800}}
                    if name not in ('click', 'press_key'):
                        return {'structuredContent': {}}
                    action_count += 1
                    calls.append((name, arguments))
                    self.assertEqual(arguments, {**candidate['phases'][action_count - 1]['arguments'],
                        **candidate['agents'][0]['target'], 'session': candidate['agents'][0]['name'],
                        'delivery_mode': 'background'})
                    if failure == 'transport' and action_count == 2:
                        raise TimeoutError('unknown effect')
                    if name == 'press_key':
                        if failure == 'dead_runtime':
                            agent.process.poll.return_value = 1
                        if failure == 'changed_runtime':
                            agent.process.pid = 102
                        if failure in ('admitted_on_denial', 'input_on_denial'):
                            kind = 'agent_admitted' if failure == 'admitted_on_denial' else 'keyboard_key'
                            events.append((kind, 100, 200, 1, 0))
                        if failure == 'plugin_refusal':
                            return {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'permission_denied'}}
                        expected = candidate['phases'][1]['expect']
                        return {'isError': True, 'structuredContent': {'code': expected['reason']},
                            'content': [{'type': 'text',
                                         'text': 'wrong reason' if failure == 'wrong_refusal' else expected['message']}]}
                    lane = 2 if failure == 'changed_lane' and action_count == 3 else 1
                    rows = capacity_events(lane)
                    events.extend([rows[0], rows[-1]] if failure == 'missing_input' else rows)
                    if failure == 'foreground_input':
                        events.append(('keyboard_key', 100, 200, 0, 0))
                    return {'structuredContent': {'route': 'synthetic_events',
                        'effect': 'partial' if failure == 'partial' else 'unverifiable',
                        'delivery': {'mode': 'unknown' if failure == 'unknown' else 'background'}}}

                agent.tool.side_effect = observer.tool.side_effect = tool
                trace_client = Mock(hello={'protocol': 3})
                def exchange(command):
                    if command == 'TRACE_STOP':
                        if failure == 'cleanup_reset':
                            events[:] = [START]
                        events.append(STOP)
                trace_client.exchange.side_effect = exchange
                def collect():
                    if failure == 'reset_trace' and trace_client.collect.call_count == 4:
                        events[:] = [START]
                    if failure == 'gap_input' and trace_client.collect.call_count == 7:
                        events.append(('agent_admitted', 100, 200, 1, 0))
                    return {**trace(*events), 'active': events[-1] != STOP, 'hook': failure != 'missing_hook'}
                trace_client.collect.side_effect = collect
                if failure == 'cleanup_failure':
                    agent.close.side_effect = RuntimeError('cleanup failed')
                grab = Mock(poll=Mock(return_value=None))
                replacements = {'provenance': Mock(return_value={}),
                    'DirectMCP': Mock(side_effect=[agent, observer]), 'Trace': Mock(return_value=trace_client),
                    'subprocess.Popen': Mock(return_value=grab),
                    'primary_acknowledgement': Mock(return_value='HELD\n'),
                    'wait_for': lambda predicate: self.assertTrue(predicate()),
                    'state': lambda path: {'held': held[0], 'clicks': 0, 'keys': 0, 'scroll': 0},
                    'wm': lambda: {'pid': 10, 'address': '0x10', 'workspace': 1, 'cursor': {'x': 100, 'y': 200}},
                    'stop_process': lambda process: held.__setitem__(0, False)}
                for name, replacement in replacements.items():
                    stack.enter_context(patch('production_realapp_proof.' + name, replacement))
                self.assertEqual(run(args), 0 if failure is None else 1)
                mutations = [name for name, _ in calls if name in ('click', 'press_key')]
                self.assertEqual(mutations, ['click', 'press_key', 'click'][:len(mutations)])
                for index, (name, arguments) in enumerate(calls):
                    if name in ('click', 'press_key'):
                        self.assertEqual(calls[index - 1][0], 'snapshot')
                        if failure != 'stale_window':
                            self.assertEqual(calls[index + 1][0], 'snapshot')
                agent.close.assert_called_once()
                observer.close.assert_called_once()
                trace_client.close.assert_called_once()
                self.assertFalse(held[0])
                self.assertEqual(replacements['DirectMCP'].call_count, 2)
                self.assertEqual(replacements['DirectMCP'].call_args.args[2],
                                 {'mode': 'unrestricted', 'acknowledge_unrestricted': True})
                result = json.loads((args.evidence / 'result.json').read_text())
                if failure is None:
                    self.assertEqual(mutations, ['click', 'press_key', 'click'])
                    self.assertEqual(result['policy_cache']['compositor_lane'], 1)
                    self.assertEqual(result['continuous_isolation'], 'passed')
                    self.assertEqual(result['synthetic_cleanup'], 'verified')
                    for index in range(3):
                        self.assertTrue((args.evidence / f'policy-cache-phase-{index}-trace.json').is_file())
                if failure in ('transport', 'dead_runtime', 'changed_runtime', 'gap_input', 'next_snapshot_input'):
                    self.assertEqual(mutations, ['click', 'press_key'])


class CapacityTests(unittest.TestCase):
    def test_plan_requires_three_serial_independent_qualified_targets(self):
        validate_plan(capacity_plan())
        for mutate in (lambda p: p.update(phases=[]),
                       lambda p: p.update(phases=p['phases'][:2]),
                       lambda p: p.update(agents=p['agents'][:2]),
                       lambda p: p['phases'][1].update(agent=0),
                       lambda p: p['phases'][0].update(parallel=[]),
                       lambda p: p['phases'][0].update(expect={'kind': 'unknown'}),
                       lambda p: p['phases'][2].pop('expect'),
                       lambda p: p['phases'][2].update(expect={'kind': 'refused', 'reason': 'permission_denied'}),
                       lambda p: p['agents'][2].update(target=p['agents'][1]['target']),
                       lambda p: p['agents'][2]['target'].update(window_id=None),
                       lambda p: p['agents'][2].update(app='fixture'),
                       lambda p: p.update(moving_primary=True),
                       lambda p: p.update(require_overlap=True)):
            bad = capacity_plan()
            mutate(bad)
            with self.subTest(plan=bad), self.assertRaises(AssertionError):
                validate_plan(bad)
        # Capacity does not redefine the normal reviewed permission contract.
        for mode in ('standard', 'bounded', 'unrestricted'):
            candidate = capacity_plan()
            candidate['agents'][0]['profile']['mode'] = mode
            validate_plan(candidate)
        for purpose in ('apps', 'policy', 'negative_control'):
            with self.assertRaises(AssertionError):
                validate_plan({**capacity_plan(), 'purpose': purpose})

    def test_lane_needs_actual_ordered_input_and_completion(self):
        before = {**trace(START), 'active': True}
        events = capacity_events(1)
        after = {**trace(START, *events), 'active': True}
        self.assertEqual(capacity_lane(before, after, 'click'), 1)
        for rows in ([], events[:1], events[1:], events[:-1],
                     [events[0], events[-1]], list(reversed(events)),
                     [events[0], *capacity_events(2)],
                     [(kind, x, y, 0, state) for kind, x, y, lane, state in events]):
            with self.subTest(rows=rows), self.assertRaises(AssertionError):
                capacity_lane(before, {**trace(START, *rows), 'active': True}, 'click')
        for malformed in (None, {}, {**after, 'events': []}, {**after, 'hook': False},
                          {**after, 'overflow': True}, {**after, 'count': 0},
                          {**after, 'timed_out': True}, {**after, 'events': [[1]]}):
            with self.subTest(trace=malformed), self.assertRaises(AssertionError):
                capacity_lane(before, malformed, 'click')
        with self.assertRaises(AssertionError):
            capacity_lane(before, after, 'press_key')

    def test_summary_rejects_incomplete_wrong_lane_and_wrong_refusal(self):
        observed = {'route': 'synthetic_events', 'effect': 'unverifiable', 'delivery': {'mode': 'background'}}
        actions = [{'agent': i, 'expected': 'dispatched', 'observed': observed, 'compositor_lane': i + 1}
                   for i in range(2)] + [{'agent': 2, 'expected': 'refused', 'no_dispatch': 'verified',
                                         'observed': {'effect': 'refused', 'reason': 'lane_busy'}}]
        self.assertEqual(verify_capacity(actions)['result'], 'verified')
        for mutate in (lambda a: a.clear(), lambda a: a.pop(),
                       lambda a: a[1].update(compositor_lane=1),
                       lambda a: a[1].pop('compositor_lane'),
                       lambda a: a[0].update(observed={}),
                       lambda a: a[2].update(no_dispatch='unproven'),
                       lambda a: a[2]['observed'].update(reason='permission_denied'),
                       lambda a: a[2]['observed'].update(delivery={'mode': 'background'})):
            bad = copy.deepcopy(actions)
            mutate(bad)
            with self.subTest(actions=bad), self.assertRaises(AssertionError):
                verify_capacity(bad)

    def test_missing_trace_fails_before_process_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source_plan = root / 'plan.json'
            source_plan.write_text(json.dumps(capacity_plan()))
            args = SimpleNamespace(plan=source_plan, evidence=root / 'evidence', trace_socket=None)
            with patch('production_realapp_proof.provenance') as provenance, \
                    patch('production_realapp_proof.DirectMCP') as spawn:
                self.assertEqual(run(args), 1)
                provenance.assert_not_called()
                spawn.assert_not_called()
            result = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(result['error'], 'capacity requires continuous trace')
            self.assertEqual(result['capacity']['result'], 'unproven')

    def test_runner_serial_capacity_and_failures_without_replay(self):
        for failure in (None, 'same_lane', 'missing_input', 'wrong_refusal', 'dispatch_on_refusal',
                        'dead_runtime', 'stale_window', 'transport', 'missing_hook', 'reset_trace'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                root = Path(directory)
                candidate = capacity_plan()
                path = root / 'plan.json'
                path.write_text(json.dumps(candidate))
                args = SimpleNamespace(plan=path, evidence=root / 'evidence',
                                       trace_socket=root / 'cua-input-v3.sock', driver=root / 'driver',
                                       primary_grab=root / 'primary-grab', foreground_journal=root / 'journal',
                                       record_video=False)
                calls, events, held = [], [START], [True]
                agents = [Mock(process=Mock(pid=101 + i, poll=Mock(return_value=None))) for i in range(3)]
                observer = Mock()
                targets = [candidate['foreground']] + [spec['target'] for spec in candidate['agents']]

                def tool(index, name, arguments):
                    if name == 'list_windows':
                        windows = copy.deepcopy(targets)
                        if failure == 'stale_window':
                            windows[-1]['window_id'] += 1
                        return {'structuredContent': {'windows': windows}}
                    if name == 'get_window_state':
                        calls.append(('snapshot', index))
                        return {'structuredContent': {'screenshot_width': 600,
                                'window_bounds': candidate['agents'][0]['bounds']}}
                    if name == 'get_desktop_state':
                        return {'structuredContent': {'screen_width': 800, 'screen_height': 800}}
                    if name != 'click':
                        return {'structuredContent': {}}
                    calls.append(('click', index))
                    self.assertEqual(arguments['delivery_mode'], 'background')
                    self.assertEqual({key: arguments[key] for key in ('pid', 'window_id')},
                                     candidate['agents'][index]['target'])
                    if failure == 'transport':
                        raise TimeoutError('unknown effect')
                    if index < 2:
                        lane = 1 if failure == 'same_lane' else index + 1
                        rows = capacity_events(lane)
                        events.extend([rows[0], rows[-1]] if failure == 'missing_input' else rows)
                        if failure == 'dead_runtime':
                            agents[0].process.poll.return_value = 1
                        return {'structuredContent': {'route': 'synthetic_events', 'effect': 'unverifiable',
                                                      'delivery': {'mode': 'background'}}}
                    if failure == 'dispatch_on_refusal':
                        events.extend(capacity_events(1))
                    return {'isError': True, 'structuredContent': {'effect': 'refused',
                            'reason': 'permission_denied' if failure == 'wrong_refusal' else 'lane_busy'}}

                for i, mcp in enumerate(agents + [observer]):
                    mcp.tool.side_effect = lambda name, arguments, i=i: tool(i, name, arguments)
                trace_client = Mock(hello={'protocol': 3})
                trace_client.exchange.side_effect = lambda command: events.append(STOP) if command == 'TRACE_STOP' else None
                def collect():
                    if failure == 'reset_trace' and trace_client.collect.call_count == 3:
                        events[:] = [START]
                    return {**trace(*events), 'active': events[-1] != STOP, 'hook': failure != 'missing_hook'}
                trace_client.collect.side_effect = collect
                grab = Mock(poll=Mock(return_value=None))
                replacements = {'provenance': Mock(return_value={}),
                    'DirectMCP': Mock(side_effect=agents + [observer]), 'Trace': Mock(return_value=trace_client),
                    'subprocess.Popen': Mock(return_value=grab),
                    'primary_acknowledgement': Mock(return_value='HELD\n'),
                    'wait_for': lambda predicate: self.assertTrue(predicate()),
                    'state': lambda path: {'held': held[0], 'clicks': 0, 'keys': 0, 'scroll': 0},
                    'wm': lambda: {'pid': 10, 'address': '0x10', 'workspace': 1, 'cursor': {'x': 100, 'y': 200}},
                    'stop_process': lambda process: held.__setitem__(0, False)}
                for name, replacement in replacements.items():
                    stack.enter_context(patch('production_realapp_proof.' + name, replacement))
                self.assertEqual(run(args), 0 if failure is None else 1)
                action_indexes = [index for name, index in calls if name == 'click']
                self.assertEqual(action_indexes, list(range(len(action_indexes))))
                if failure == 'missing_hook':
                    self.assertEqual(action_indexes, [])
                if failure == 'reset_trace':
                    self.assertEqual(action_indexes, [0])
                for i, call in enumerate(calls):
                    if call[0] == 'click':
                        self.assertEqual(calls[i - 1], ('snapshot', call[1]))
                        self.assertEqual(calls[i + 1][0], 'snapshot')
                for mcp in agents:
                    mcp.close.assert_called_once()
                if failure != 'stale_window':
                    observer.close.assert_called_once()
                    self.assertFalse(held[0])
                result = json.loads((args.evidence / 'result.json').read_text())
                if failure is None:
                    self.assertEqual(action_indexes, [0, 1, 2])
                    self.assertEqual(result['capacity']['lanes'], [1, 2])
                    self.assertEqual(result['continuous_isolation'], 'passed')
                    self.assertEqual(result['synthetic_cleanup'], 'verified')
                    for i in range(3):
                        self.assertTrue((args.evidence / f'capacity-agent-{i}-trace.json').is_file())


class PlanTests(unittest.TestCase):
    def test_smoke_stages_allow_only_the_exact_app_keyboard_steps(self):
        for index, app in enumerate(('calc', 'inkscape')):
            for stage, (tool, arguments) in SMOKE_STEPS[app].items():
                with self.subTest(app=app, stage=stage):
                    candidate = plan()
                    candidate['phases'] = [{'agent': index, 'smoke_stage': stage,
                                            'tool': tool, 'arguments': arguments}]
                    validate_plan(candidate)
                    for change in ({'smoke_stage': 'after'}, {'smoke_stage': None},
                                   {'smoke_stage': []}, {'tool': 'click'},
                                   {'arguments': {**arguments, 'x': 1}},
                                   {'arguments': {'key': 'Escape'}}):
                        bad = copy.deepcopy(candidate)
                        bad['phases'][0].update(change)
                        with self.assertRaises(AssertionError):
                            validate_plan(bad)
                    candidate['agents'][index]['app'] = 'unknown'
                    with self.assertRaises(AssertionError):
                        validate_plan(candidate)

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


class SmokeStageTests(unittest.TestCase):
    def test_parallel_prebarrier_failure_preserves_root_cause_and_aborts_sibling(self):
        steps = [{'agent': i, 'tool': 'press_key'} for i in range(2)]
        entered = threading.Event()
        calls = []
        def action(step, barrier):
            calls.append(step['agent'])
            if step['agent'] == 1:
                self.assertTrue(entered.wait(1))
                raise RuntimeError('selection grounding unavailable')
            entered.set()
            barrier.wait(timeout=2)
            self.fail('sibling must not dispatch')
        results, errors = parallel_actions(steps, action)
        self.assertEqual(results, [])
        self.assertCountEqual(calls, [0, 1])
        self.assertEqual(errors, [
            {'agent': 0, 'tool': 'press_key', 'error_type': 'BrokenBarrierError', 'message': ''},
            {'agent': 1, 'tool': 'press_key', 'error_type': 'RuntimeError',
             'message': 'selection grounding unavailable'}])

    def test_parallel_postbarrier_failure_retains_successful_sibling_without_replay(self):
        steps = [{'agent': i, 'tool': 'press_key'} for i in range(2)]
        calls = []
        def action(step, barrier):
            barrier.wait(timeout=2)
            calls.append(step['agent'])
            if step['agent'] == 0:
                raise RuntimeError('unconfirmed effect')
            return {'agent': 1, 'expected': 'dispatched'}
        results, errors = parallel_actions(steps, action)
        self.assertEqual(results, [{'agent': 1, 'expected': 'dispatched'}])
        self.assertCountEqual(calls, [0, 1])
        self.assertEqual(errors, [{'agent': 0, 'tool': 'press_key', 'error_type': 'RuntimeError',
                                   'message': 'unconfirmed effect'}])

    def test_runner_grounds_full_snapshots_and_never_recovers_or_replays(self):
        for app, stage, elements in (
                ('calc', 'insert', [{'role': 'table cell', 'label': 'A1', 'selected': True}]),
                ('inkscape', 'select', INKSCAPE['elements']),
                ('inkscape', 'move', INKSCAPE_SELECTED['elements'])):
            for failure in (None, 'no_elements', 'dialog', 'extra_window', 'after_dialog', 'transport', 'unmarked'):
                with self.subTest(app=app, stage=stage, failure=failure), \
                        tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                    root = Path(directory)
                    candidate = plan()
                    index = 0 if app == 'calc' else 1
                    tool_name, arguments = SMOKE_STEPS[app][stage]
                    candidate['phases'] = [{'agent': index, 'smoke_stage': stage,
                                            'tool': tool_name, 'arguments': arguments}]
                    if failure == 'unmarked':
                        del candidate['phases'][0]['smoke_stage']
                    bounds = {'x': 0, 'y': 0, 'width': 600, 'height': 600}
                    for i, spec in enumerate(candidate['agents']):
                        spec.update(bounds=bounds, name=f'agent-{i}', profile={'mode': 'standard'})
                        path = root / f'{i}.xml'
                        path.write_text('<synthetic/>')
                        candidate['outputs'][i]['path'] = str(path)
                    path = root / 'plan.json'
                    path.write_text(json.dumps(candidate))
                    args = SimpleNamespace(plan=path, evidence=root / 'evidence', trace_socket=None,
                                           driver=root / 'driver', primary_grab=root / 'primary-grab',
                                           foreground_journal=root / 'journal', record_video=False)
                    calls, held = [], [True]
                    agents = [Mock(process=Mock(pid=101 + i, poll=Mock(return_value=None))) for i in range(2)]
                    observer = Mock()
                    targets = [candidate['foreground']] + [spec['target'] for spec in candidate['agents']]

                    def tool(name, parameters):
                        if name == 'list_windows':
                            windows = copy.deepcopy(targets)
                            if failure == 'extra_window':
                                windows.append({**candidate['agents'][index]['target'], 'window_id': 999})
                            return {'structuredContent': {'windows': windows}}
                        if name == 'get_window_state':
                            calls.append(('snapshot', parameters))
                            sent = any(row[0] == 'input' for row in calls)
                            rows = elements
                            if failure in ('no_elements', 'unmarked'):
                                rows = []
                            if failure == 'dialog' or (failure == 'after_dialog' and sent):
                                rows = [{'role': 'dialog'}]
                            return {'structuredContent': {'screenshot_width': 600, 'window_bounds': bounds,
                                                          'tree_markdown': (INKSCAPE_SELECTED if stage == 'move'
                                                                            else INKSCAPE)['tree_markdown'],
                                                          'elements': rows}}
                        if name == 'get_desktop_state':
                            return {'structuredContent': {'screen_width': 800, 'screen_height': 800}}
                        if name != tool_name:
                            return {'structuredContent': {}}
                        calls.append(('input', parameters))
                        if failure == 'transport':
                            raise TimeoutError('unknown input outcome')
                        return {'structuredContent': {'route': 'synthetic_events', 'effect': 'unverifiable',
                                                      'delivery': {'mode': 'background'}}}

                    for mcp in agents + [observer]:
                        mcp.tool.side_effect = tool
                    replacements = {'provenance': Mock(return_value={}),
                        'DirectMCP': Mock(side_effect=agents + [observer]),
                        'subprocess.Popen': Mock(return_value=Mock(poll=Mock(return_value=None))),
                        'primary_acknowledgement': Mock(return_value='HELD\n'),
                        'verify_output': Mock(return_value={'verified': True}),
                        'wait_for': lambda predicate: self.assertTrue(predicate()),
                        'state': lambda path: {'held': held[0], 'clicks': 0, 'keys': 0, 'scroll': 0},
                        'wm': lambda: {'pid': 10, 'address': '0x10', 'workspace': 1,
                                       'cursor': {'x': 100, 'y': 200}},
                        'stop_process': lambda process: held.__setitem__(0, False)}
                    for name, replacement in replacements.items():
                        stack.enter_context(patch('production_realapp_proof.' + name, replacement))
                    self.assertEqual(run(args), 0 if failure in (None, 'unmarked') else 1)
                    inputs = [i for i, row in enumerate(calls) if row[0] == 'input']
                    self.assertEqual(len(inputs), 0 if failure in ('no_elements', 'dialog', 'extra_window') else 1)
                    if inputs:
                        i = inputs[0]
                        self.assertEqual(calls[i - 1][0], 'snapshot')
                        self.assertEqual(calls[i + 1][0], 'snapshot')
                        self.assertEqual(calls[i][1]['delivery_mode'], 'background')
                        self.assertNotIn('smoke_stage', calls[i][1])
                        for _, parameters in (calls[i - 1], calls[i + 1]):
                            self.assertEqual('max_elements' in parameters, failure == 'unmarked')
                            self.assertEqual('max_depth' in parameters, failure == 'unmarked')
                    for mcp in agents + [observer]:
                        mcp.close.assert_called_once()
                    self.assertFalse(held[0])
                    report = json.loads((args.evidence / 'result.json').read_text())
                    if failure is None:
                        self.assertEqual(report['actions'][0]['smoke_stage'], stage)


class PointerStageTests(unittest.TestCase):
    def test_only_declared_intermediate_episodes_may_omit_saved_outputs(self):
        for index, name in enumerate(POINTER_EPISODES):
            candidate = plan()
            candidate.update(pointer_episode={'name': name, 'index': index, 'count': 5},
                             require_overlap=name == 'drags')
            if name != 'save':
                candidate['outputs'] = []
            validate_plan(candidate)
            moving = copy.deepcopy(candidate)
            moving['moving_primary'] = True
            if name == 'drags':
                validate_plan(moving)
            else:
                with self.assertRaises(AssertionError):
                    validate_plan(moving)
            for mutation in (lambda p: p['pointer_episode'].update(count=4),
                             lambda p: p['pointer_episode'].update(index=True),
                             lambda p: p['pointer_episode'].update(name='unknown'),
                             lambda p: p['pointer_episode'].update(extra=True),
                             lambda p: p.update(purpose='policy'),
                             lambda p: p.update(require_overlap=name != 'drags')):
                invalid = copy.deepcopy(candidate)
                mutation(invalid)
                with self.assertRaises(AssertionError):
                    validate_plan(invalid)
        for metadata in ({}, {'pointer_episode': None},
                         {'pointer_episode': {'name': 'save', 'index': 4, 'count': 5}}):
            with self.subTest(metadata=metadata), self.assertRaises(AssertionError):
                validate_plan({**plan(), **metadata, 'outputs': []})

    def test_primary_guard_fails_at_deadline_or_any_helper_exit(self):
        helper = Mock(poll=Mock(return_value=None))
        require_primary_active(helper, 100, now_ns=99)
        for now in (100, 101):
            with self.assertRaisesRegex(AssertionError, 'deadline expired'):
                require_primary_active(helper, 100, now_ns=now)
        for code in (0, 1, -15):
            helper.poll.return_value = code
            with self.assertRaisesRegex(AssertionError, 'helper exited'):
                require_primary_active(helper, 100, now_ns=99)

    def test_pointer_stages_reject_supplied_coordinates_or_wrong_tool(self):
        candidate = plan()
        candidate['phases'] = [{'agent': 0, 'pointer_stage': 'click_b2', 'tool': 'click', 'arguments': {}}]
        validate_plan(candidate)
        for change in ({'arguments': {'x': 1, 'y': 2}}, {'tool': 'press_key'},
                       {'pointer_stage': 'unknown'}, {'smoke_stage': 'insert'},
                       {'expect': {'kind': 'partial'}}):
            changed = copy.deepcopy(candidate)
            changed['phases'][0].update(change)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                validate_plan(changed)

    def test_pointer_actions_use_the_same_response_image_and_never_replay(self):
        for failure in (None, 'before', 'after', 'after_recovers', 'missing_image', 'transport',
                        'helper_before', 'deadline_before', 'helper_after', 'deadline_after'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                root = Path(directory)
                candidate = plan()
                candidate['phases'] = [{'agent': 0, 'pointer_stage': 'click_b2', 'tool': 'click', 'arguments': {}}]
                bounds = {'x': 0, 'y': 0, 'width': 600, 'height': 600}
                for index, spec in enumerate(candidate['agents']):
                    spec.update(bounds=bounds, name=f'agent-{index}', profile={'mode': 'standard'})
                    output = root / f'{index}.xml'
                    output.write_text('<synthetic/>')
                    candidate['outputs'][index]['path'] = str(output)
                plan_path = root / 'plan.json'
                plan_path.write_text(json.dumps(candidate))
                args = SimpleNamespace(plan=plan_path, evidence=root / 'evidence', trace_socket=None,
                                       driver=root / 'driver', primary_grab=root / 'primary-grab',
                                       foreground_journal=root / 'journal', record_video=False)
                targets = [candidate['foreground']] + [spec['target'] for spec in candidate['agents']]
                clients, calls, held = [], [], [True]
                clock_now = [1]
                grab = Mock(poll=Mock(return_value=None))
                def expire_primary(phase):
                    if failure == 'helper_' + phase:
                        grab.poll.return_value = 0
                    if failure == 'deadline_' + phase:
                        clock_now[0] = 1 + PRIMARY_LIFETIME_MS * 1_000_000
                def client(driver, folder, profile):
                    mcp = Mock(directory=folder, counter=0,
                               process=Mock(pid=100 + len(clients), poll=Mock(return_value=None)))
                    clients.append(mcp)
                    def tool(name, parameters):
                        mcp.counter += 1
                        if name == 'list_windows':
                            return {'structuredContent': {'windows': targets}}
                        if name == 'get_window_state':
                            if any(row[0] == 'input' for row in calls):
                                expire_primary('after')
                            calls.append(('snapshot', parameters, folder.name, mcp.counter))
                            image = folder / f'{mcp.counter}.png'
                            image.write_bytes(b'synthetic image read by a stub')
                            return {'structuredContent': {'screenshot_width': 600, 'window_bounds': bounds},
                                    'content': [] if failure == 'missing_image' else
                                    [{'type': 'image', 'image_file': image.name}]}
                        if name == 'get_desktop_state':
                            return {'structuredContent': {'screen_width': 800, 'screen_height': 800}}
                        if name == 'click':
                            calls.append(('input', parameters))
                            if failure == 'transport':
                                raise TimeoutError('unknown outcome')
                            return {'structuredContent': {'route': 'synthetic_events', 'effect': 'unverifiable'}}
                        return {'structuredContent': {}}
                    mcp.tool.side_effect = tool
                    return mcp
                def ground_pointer(*_):
                    if failure == 'before':
                        raise RuntimeError('missing grounding')
                    expire_primary('before')
                    return {'x': 103, 'y': 205}, {'app': 'calc', 'stage': 'click_b2'}
                grounding = Mock(side_effect=ground_pointer)
                verification = Mock(return_value={'verified': True},
                                    side_effect=AssertionError('unchanged app') if failure == 'after' else None)
                if failure == 'after_recovers':
                    verification.side_effect = [AssertionError('not settled'), {'verified': True}, {'verified': True}]
                replacements = {'provenance': Mock(return_value={}), 'DirectMCP': client,
                    'subprocess.Popen': Mock(return_value=grab),
                    'time.monotonic_ns': lambda: clock_now[0],
                    'primary_acknowledgement': Mock(return_value='HELD\n'),
                    'verify_output': Mock(return_value={'verified': True}),
                    'wait_for': lambda predicate: self.assertTrue(predicate()),
                    'state': lambda path: {'held': held[0], 'clicks': 0, 'keys': 0, 'scroll': 0},
                    'wm': lambda: {'pid': 10, 'address': '0x10', 'workspace': 1, 'cursor': {'x': 100, 'y': 200}},
                    'stop_process': lambda process: held.__setitem__(0, False),
                    'pointer_grounding.read_pixels': lambda path: path,
                    'pointer_grounding.action': grounding, 'pointer_grounding.verify': verification}
                for name, replacement in replacements.items():
                    stack.enter_context(patch('production_realapp_proof.' + name, replacement))
                self.assertEqual(run(args), 0 if failure is None else 1)
                inputs = [index for index, row in enumerate(calls) if row[0] == 'input']
                self.assertEqual(len(inputs), 0 if failure in
                                 ('before', 'missing_image', 'helper_before', 'deadline_before') else 1)
                for index in inputs:
                    self.assertEqual(calls[index - 1][0], 'snapshot')
                    self.assertEqual(calls[index + 1][0], 'snapshot')
                    self.assertEqual(calls[index][1], {**candidate['agents'][0]['target'],
                                                      'session': 'agent-0', 'delivery_mode': 'background',
                                                      'x': 103, 'y': 205})
                    before = calls[index - 1]
                    self.assertTrue(grounding.call_args.args[1].endswith(f'{before[2]}/{before[3]}.png'))
                    self.assertEqual(grounding.call_args.args[0]['proof_image'], grounding.call_args.args[1])
                if failure is None:
                    after = calls[inputs[0] + 1]
                    self.assertTrue(verification.call_args.args[1].endswith(f'{after[2]}/{after[3]}.png'))
                    report = json.loads((args.evidence / 'result.json').read_text())
                    self.assertTrue(report['actions'][0]['app_effect_verified'])
                    self.assertEqual(report['actions'][0]['pointer_stage'], 'click_b2')
                    self.assertEqual(report['actions'][0]['pointer_evidence'],
                                     {'before_request': before[3], 'after_request': after[3]})
                if inputs:
                    self.assertEqual(len(list(args.evidence.glob('pointer-agent-*.json'))), 1)
                if failure in ('after', 'after_recovers'):
                    diagnostic = json.loads((args.evidence / 'pointer-effect-failure-agent-0.json').read_text())
                    self.assertFalse(diagnostic['replayed'])
                    self.assertEqual(len(diagnostic['samples']), 2)
                    self.assertEqual(verification.call_count, 3)
                    if failure == 'after_recovers':
                        self.assertTrue(all(row['effect']['verified'] for row in diagnostic['samples']))
                    report = json.loads((args.evidence / 'result.json').read_text())
                    self.assertEqual(report['result'], 'failed')
                    self.assertEqual(report['actions'], [])
                if failure and failure.startswith(('helper_', 'deadline_')):
                    report = json.loads((args.evidence / 'result.json').read_text())
                    self.assertEqual(report['result'], 'failed')
                    self.assertIn('primary grab', report['error'])
                    self.assertEqual(report['actions'], [])
                for mcp in clients:
                    mcp.close.assert_called_once()
                self.assertFalse(held[0])


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
        for kind in ('pointer_button', 'agent_admitted', 'agent_approved', 'agent_drag_start'):
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
                grab = Mock(stdout=io.StringIO('HELD\n'), poll=Mock(return_value=None))
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

    def test_unrestricted_direct_launch_uses_environment_not_serve_only_flags(self):
        process = Mock()
        process.poll.return_value = 0
        with tempfile.TemporaryDirectory() as directory, \
                patch('production_mcp.subprocess.Popen', return_value=process) as spawn, \
                patch('driver_input_live.MCP.rpc', return_value={}):
            mcp = DirectMCP(Path('/synthetic/driver'), Path(directory),
                            {'mode': 'unrestricted', 'acknowledge_unrestricted': True})
            mcp.close()
            self.assertEqual(spawn.call_args.args[0], [
                '/synthetic/driver', 'mcp', '--direct'])
            self.assertEqual(spawn.call_args.kwargs['env']['CUA_DRIVER_PERMISSION_MODE'], 'unrestricted')
            self.assertEqual(spawn.call_args.kwargs['env']['CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS'], '1')

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
