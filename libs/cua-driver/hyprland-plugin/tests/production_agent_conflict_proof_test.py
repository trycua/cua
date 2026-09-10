"""Portable adversarial orchestration tests; no native desktop is certified."""
from contextlib import ExitStack
from copy import deepcopy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_agent_conflict_proof as proof


BOUNDS = {'x': 10, 'y': 20, 'width': 800, 'height': 600}
REFUSED = {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'agent_target_busy', 'lane': 1}}
PRIMARY_REFUSED = {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'primary_target_busy', 'lane': 1}}
DELIVERED = {'structuredContent': {'effect': 'unverifiable', 'route': 'synthetic_events',
                                 'delivery': {'mode': 'background'}}}
RECTANGLE = {'x': 100, 'y': 200, 'w': 60, 'h': 40, 'center': [129, 219]}
GEOMETRY = {'X': 1.0, 'Y': 2.0, 'W': 3.0, 'H': 4.0}
ORACLE = {'app': 'inkscape', 'stage': 'scroll_down', 'rectangle': RECTANGLE, 'geometry': GEOMETRY}


def identity(pid, name='app'):
    return {'pid': pid, 'uid': 1000, 'starttime': '123', 'exe': '/usr/bin/' + name}


def plan(stage='scroll_down', recovery='scroll_visible'):
    return {'purpose': 'agent_conflict', 'case': 'passive_hover_refusal', 'app_profile': 'inkscape-only',
        'disposable': True,
        'vm': {'machine_id': 'a' * 32, 'boot_id': 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'},
        'compositor': {**identity(50, 'Hyprland'), 'instance': 'test_1'},
        'processes': {'target': identity(20, 'inkscape'), 'foreground': identity(10)},
        'foreground': {'pid': 10, 'window_id': 100}, 'primary_point': [20, 20], 'package_versions': {},
        'agents': [{'app': 'inkscape', 'name': 'agent-conflict', 'target': {'pid': 20, 'window_id': 200},
            'document': '/tmp/cua/cua-smoke-inkscape.svg', 'bounds': dict(BOUNDS), 'pointer_stage': stage, 'drag': {}}],
        'refused': {'pointer_stage': proof.OPPOSITE[stage]}, 'recovery': {'pointer_stage': recovery}}


def trace(events=(), active=True):
    rows = [(0, 'start', 0, 0), *events]
    if not active:
        rows += [(20, 'stop', 0, 0)]
    return {'hook': True, 'active': active, 'overflow': False, 'timed_out': False, 'count': len(rows),
        'events': [[i + 1, timestamp, kind, 30, 40, lane, value]
                   for i, (timestamp, kind, lane, value) in enumerate(rows)]}


def scroll_rows(lane, start=1):
    return [(start, 'agent_admitted', lane, 0), (start + 1, 'pointer_axis', lane, 1),
            (start + 2, 'agent_action_end', lane, 0)]


def status():
    return {'configured': True, 'transport': {'ready': True}, 'input': {
        'protocol': 3, 'test_only': False, 'transport_ready': True,
        'seat_lifetime': 'compositor', 'upgrade': 'desktop_restart', 'lanes': [
            {'lane': lane, 'held_button': 0, 'held_keys': 0, 'drag_active': False,
             'lease_active': False, 'keyboard_focus': False, 'pointer_focus': False, 'reserved': False}
            for lane in (0, 1)]}}


def owner_status(owner_lane, *, peer_reserved=False, orphan=False):
    value = status()
    value['input']['lanes'][owner_lane].update(pointer_focus=True, reserved=not orphan)
    value['input']['lanes'][1 - owner_lane]['reserved'] = peer_reserved
    return value


def client(pid):
    return Mock(directory=Path.cwd(), process=Mock(pid=pid, poll=Mock(return_value=None)))


class PlanTests(unittest.TestCase):
    def test_plan_requires_exact_narrow_same_client_scroll_cell(self):
        for stage in proof.SCROLL_STAGES:
            for recovery in proof.RECOVERY_STAGES:
                proof.validate_plan(plan(stage, recovery))
        updates = ({'purpose': 'primary_conflict'}, {'case': 'initial_refusal'}, {'app_profile': 'calc-inkscape'},
                   {'disposable': False}, {'fault': {'kind': 'move', 'to': [11, 20]}},
                   {'refused': {'pointer_stage': 'scroll_down'}}, {'refused': {'pointer_stage': 'scroll_visible'}},
                   {'refused': {'pointer_stage': 'scroll_up', 'extra': 1}}, {'refused': {}},
                   {'recovery': {'pointer_stage': 'move_rectangle'}}, {'recovery': {'pointer_stage': 'click_rectangle'}},
                   {'vm': {'machine_id': 'unknown', 'boot_id': 'unknown'}},
                   {'agents': [plan()['agents'][0], plan()['agents'][0]]})
        for update in updates:
            with self.subTest(update=update), self.assertRaises(AssertionError):
                proof.validate_plan({**plan(), **update})
        for key, value in (('pointer_stage', 'move_rectangle'), ('pointer_stage', 'click_rectangle'),
                           ('pointer_stage', 'scroll_visible'), ('drag', {'from_x': 1}), ('app', 'calc'),
                           ('document', '/tmp/other.svg'), ('document', 'cua-smoke-inkscape.svg')):
            candidate = plan()
            candidate['agents'][0][key] = value
            with self.subTest(key=key, value=value), self.assertRaises((AssertionError, KeyError)):
                proof.validate_plan(candidate)
        missing = plan()
        del missing['agents'][0]['document']
        with self.assertRaises((AssertionError, KeyError)):
            proof.validate_plan(missing)
        for owner, key, value in (('target', 'exe', '/usr/bin/calc'), ('target', 'pid', 21), ('target', 'uid', 1001),
                                  ('foreground', 'starttime', ''), ('foreground', 'exe', 'relative')):
            candidate = plan()
            candidate['processes'][owner][key] = value
            with self.subTest(owner=owner, key=key), self.assertRaises(AssertionError):
                proof.validate_plan(candidate)
        shared = plan()
        shared['foreground']['window_id'] = 200
        with self.assertRaises(AssertionError):
            proof.validate_plan(shared)


class StatusTests(unittest.TestCase):
    def test_owner_refusal_and_orphan_invariants_reject_every_mutation(self):
        for owner_lane in (0, 1):
            live = owner_status(owner_lane)
            self.assertIs(proof.verify_owner_status(live, owner_lane), live)
            with self.assertRaises(AssertionError):
                proof.verify_owner_status(live, 1 - owner_lane)
            refusing = owner_status(owner_lane, peer_reserved=True)
            response = {**REFUSED, 'structuredContent': {**REFUSED['structuredContent'], 'lane': 1 - owner_lane}}
            self.assertIs(proof.verify_refusal_status(refusing, response, owner_lane), refusing)
            with self.assertRaises(AssertionError):
                proof.verify_owner_status(refusing, owner_lane)  # peer reservation must clear after B's EOF
            with self.assertRaises(AssertionError):
                proof.verify_refusal_status(live, response, owner_lane)  # refused CLAIM must still be reserved
            for invalid in (owner_lane, None, True, -1, 2):
                with self.subTest(invalid=invalid), self.assertRaises(AssertionError):
                    proof.verify_refusal_status(refusing, {**response, 'structuredContent': {
                        **response['structuredContent'], 'lane': invalid}}, owner_lane)
            with self.assertRaises(AssertionError):
                proof.verify_refusal_status(refusing, {**PRIMARY_REFUSED, 'structuredContent': {
                    **PRIMARY_REFUSED['structuredContent'], 'lane': 1 - owner_lane}}, owner_lane)
            orphan = owner_status(owner_lane, orphan=True)
            self.assertIs(proof.verify_orphan_status(orphan, owner_lane), orphan)
            with self.assertRaises(AssertionError):
                proof.verify_orphan_status(live, owner_lane)  # reservation must clear
            with self.assertRaises(AssertionError):
                proof.verify_orphan_status(status(), owner_lane)  # hover must remain until a fresh TARGET
            with self.assertRaises(AssertionError):
                proof.verify_owner_status(orphan, owner_lane)
            mutations = ((owner_lane, 'pointer_focus', False), (1 - owner_lane, 'pointer_focus', True),
                         (owner_lane, 'lease_active', True), (1 - owner_lane, 'lease_active', True),
                         (owner_lane, 'drag_active', True), (owner_lane, 'keyboard_focus', True),
                         (owner_lane, 'held_button', 272), (owner_lane, 'held_keys', 1),
                         (1 - owner_lane, 'held_button', 272), (owner_lane, 'reserved', 1))
            for base, verify in ((live, lambda s: proof.verify_owner_status(s, owner_lane)),
                                 (refusing, lambda s: proof.verify_refusal_status(s, response, owner_lane)),
                                 (orphan, lambda s: proof.verify_orphan_status(s, owner_lane))):
                for index, field, value in mutations:
                    candidate = deepcopy(base)
                    candidate['input']['lanes'][index][field] = value
                    with self.subTest(owner_lane=owner_lane, index=index, field=field), self.assertRaises(AssertionError):
                        verify(candidate)
            for base in (live, refusing, orphan):
                broken = deepcopy(base)
                broken['input']['lanes'][1]['lane'] = 0
                with self.assertRaises(AssertionError):
                    proof.lane_rows(broken)


class OracleTests(unittest.TestCase):
    def test_exact_refusal_from_other_lane_with_zero_synthetic_events(self):
        before = trace(scroll_rows(1))
        proof.verify_refusal(before, before, REFUSED, 0)
        for response in (DELIVERED, PRIMARY_REFUSED,
                         {**REFUSED, 'structuredContent': {**REFUSED['structuredContent'], 'lane': 0}},
                         {**REFUSED, 'structuredContent': {**REFUSED['structuredContent'], 'lane': None}},
                         {**REFUSED, 'structuredContent': {**REFUSED['structuredContent'], 'delivery': {'mode': 'unknown'}}},
                         {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'lane_busy', 'lane': 1}}):
            with self.subTest(response=response), self.assertRaises(AssertionError):
                proof.verify_refusal(before, before, response, 0)
        for kind in ('agent_admitted', 'agent_cancel', 'pointer_enter', 'pointer_leave', 'pointer_button',
                     'pointer_motion', 'pointer_axis', 'keyboard_key', 'agent_drag_start', 'agent_action_end'):
            for lane in (1, 2):
                after = trace(scroll_rows(1) + [(9, kind, lane, 0)])
                with self.subTest(kind=kind, lane=lane), self.assertRaises(AssertionError):
                    proof.verify_refusal(before, after, REFUSED, 0)
        for key, value in (('hook', False), ('active', False), ('overflow', True), ('timed_out', True), ('count', 9)):
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_refusal(before, {**before, key: value}, REFUSED, 0)
        changed = deepcopy(before)
        changed['events'][1][1] += 1
        with self.assertRaisesRegex(AssertionError, 'history'):
            proof.verify_refusal(before, changed, REFUSED, 0)

    def test_refused_scroll_must_leave_rectangle_and_document_unchanged(self):
        after, image = {'proof_image': 'after.png'}, object()
        for failure in (None, 'moved', 'resized', 'geometry', 'stage', 'app', 'unreadable'):
            rectangle = dict(RECTANGLE)
            if failure == 'moved':
                rectangle['y'] -= 3
            elif failure == 'resized':
                rectangle['w'] += 2
            oracle = deepcopy(ORACLE)
            if failure == 'stage':
                oracle['stage'] = 'move_rectangle'
            elif failure == 'app':
                oracle['app'] = 'calc'
            with self.subTest(failure=failure), ExitStack() as stack:
                blue = stack.enter_context(patch.object(proof.pointer_grounding, 'blue_rectangle',
                    side_effect=proof.pointer_grounding.GroundingUnavailable('hidden') if failure == 'unreadable' else None,
                    return_value=rectangle))
                geometry = stack.enter_context(patch.object(proof.pointer_grounding, 'inkscape_geometry',
                    return_value={**GEOMETRY, 'X': 1.5} if failure == 'geometry' else dict(GEOMETRY)))
                if failure:
                    with self.assertRaises(Exception):
                        proof.verify_no_effect(after, image, oracle)
                else:
                    result = proof.verify_no_effect(after, image, oracle)
                    self.assertTrue(result['verified'])
                    blue.assert_called_once_with(after, image)
                    geometry.assert_called_once_with(after, allow_transform_center=True)
        within = {**RECTANGLE, 'x': RECTANGLE['x'] + 1}
        with patch.object(proof.pointer_grounding, 'blue_rectangle', return_value=within), \
                patch.object(proof.pointer_grounding, 'inkscape_geometry', return_value=dict(GEOMETRY)):
            proof.verify_no_effect(after, image, ORACLE)

    def test_runtime_close_may_only_tear_down(self):
        before = trace(scroll_rows(1))
        self.assertEqual(proof.verify_close_events(before, before, set())['synthetic_events'], [])
        cancel = trace(scroll_rows(1) + [(9, 'agent_cancel', 1, 0)])
        self.assertEqual(len(proof.verify_close_events(before, cancel, {'agent_cancel'})['synthetic_events']), 1)
        with self.assertRaises(AssertionError):
            proof.verify_close_events(before, cancel, set())
        for kind in ('pointer_leave', 'pointer_axis', 'pointer_button', 'agent_admitted', 'pointer_enter'):
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                proof.verify_close_events(before, trace(scroll_rows(1) + [(9, kind, 1, 0)]), {'agent_cancel'})
        with self.assertRaises(AssertionError):
            proof.verify_close_events(before, trace(scroll_rows(1) + [(9, 'agent_admitted', 1, 0)]), {'agent_admitted'})
        # Primary events on lane 0 are the trace oracle's job at TRACE_STOP; the
        # stopped trace must still fail on them.
        stopped = trace(scroll_rows(1) + [(9, 'keyboard_key', 0, 1)], active=False)
        self.assertEqual(proof.analyze(stopped)['result'], 'failed')


class ActionTests(unittest.TestCase):
    def test_single_normal_scroll_fresh_snapshot_unknown_never_replayed(self):
        cases = {'owner': (None, 'guard', 'stale', 'unknown', 'same_snapshot', 'cached', 'before_return',
                           'same_runtime', 'effect', 'pre_activity', 'dead_before', 'observation', 'lane_mismatch',
                           'refused_instead', 'two_lanes'),
                 'refused': (None, 'guard', 'stale', 'unknown', 'pre_activity', 'delivered', 'primary_reason',
                             'owner_lane', 'admitted', 'pointer_leave', 'moved', 'geometry', 'observation'),
                 'recovery': (None, 'guard', 'stale', 'unknown', 'effect', 'other_lane', 'visible', 'margin',
                              'refused_instead', 'observation')}
        for stage, failures in cases.items():
            for failure in failures:
                with self.subTest(stage=stage, failure=failure), ExitStack() as stack:
                    spec = plan()['agents'][0]
                    spec['pointer_stage'] = {'owner': 'scroll_down', 'refused': 'scroll_up',
                                             'recovery': 'scroll_visible' if failure in ('visible', 'margin') else 'scroll_down'}[stage]
                    owner_lane = None if stage == 'owner' else 0
                    actor, observer = client(101), client(102)
                    if failure == 'same_runtime':
                        observer = actor
                    if failure == 'dead_before':
                        actor.process.poll.return_value = 1
                    response = REFUSED if stage == 'refused' else DELIVERED
                    if failure == 'delivered':
                        response = DELIVERED
                    elif failure == 'primary_reason':
                        response = PRIMARY_REFUSED
                    elif failure == 'owner_lane':
                        response = {**REFUSED, 'structuredContent': {**REFUSED['structuredContent'], 'lane': 0}}
                    elif failure == 'refused_instead':
                        response = REFUSED
                    elif failure == 'lane_mismatch':
                        response = {**DELIVERED, 'structuredContent': {**DELIVERED['structuredContent'], 'lane': 1}}
                    actor.tool.side_effect = TimeoutError('lost reply') if failure == 'unknown' else None
                    actor.tool.return_value = response
                    before = {'snapshot_id': 's00000001', 'proof_image': 'before.png',
                        'proof_runtime': {'pid': 101, 'directory': str(Path.cwd())},
                        'proof_observation_started_ns': 10, 'proof_observation_finished_ns': 20}
                    after = {**before, 'proof_runtime': {'pid': 102, 'directory': str(Path.cwd())},
                        'proof_image': 'after.png', 'proof_observation_started_ns': 50, 'proof_observation_finished_ns': 60}
                    if failure == 'same_snapshot':
                        after = dict(before)
                    elif failure == 'cached':
                        after.update(proof_observation_started_ns=10, proof_observation_finished_ns=20)
                    elif failure == 'before_return':
                        after['proof_observation_started_ns'] = 39
                    snapshots = stack.enter_context(patch.object(proof, 'grounded_snapshot',
                        side_effect=[before, AssertionError('snapshot unavailable')] if failure == 'observation' else [before, after]))
                    stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=
                        [5, proof.MAX_GROUNDING_AGE_NS + 11] if failure == 'stale' else [5, 30, 40, 70]))
                    stack.enter_context(patch.object(proof.pointer_grounding, 'read_pixels', return_value='pixels'))
                    resolved = 'scroll_up' if spec['pointer_stage'] == 'scroll_visible' else spec['pointer_stage']
                    choose = stack.enter_context(patch.object(proof.pointer_grounding, 'visible_inkscape_scroll_stage',
                        side_effect=proof.pointer_grounding.GroundingUnavailable('margin') if failure == 'margin' else None,
                        return_value=resolved))
                    arguments = {'x': 129, 'y': 219, 'direction': resolved.split('_')[1], 'amount': 1, 'by': 'line'}
                    oracle = {**ORACLE, 'stage': resolved}
                    ground = stack.enter_context(patch.object(proof.pointer_grounding, 'action', return_value=(arguments, oracle)))
                    verify = stack.enter_context(patch.object(proof.pointer_grounding, 'verify',
                        side_effect=AssertionError('effect missing') if failure == 'effect' else None, return_value={'verified': True}))
                    rectangle = {**RECTANGLE, 'y': RECTANGLE['y'] - 5} if failure == 'moved' else dict(RECTANGLE)
                    stack.enter_context(patch.object(proof.pointer_grounding, 'blue_rectangle', return_value=rectangle))
                    stack.enter_context(patch.object(proof.pointer_grounding, 'inkscape_geometry',
                        return_value={**GEOMETRY, 'Y': 9.0} if failure == 'geometry' else dict(GEOMETRY)))
                    boundary = trace()
                    rows = [] if stage == 'refused' else scroll_rows(2 if failure == 'other_lane' else 1)
                    if failure == 'two_lanes':
                        rows = scroll_rows(1) + scroll_rows(2, 5)
                    elif failure == 'admitted':
                        rows = [(1, 'agent_admitted', 2, 0)]
                    elif failure == 'pointer_leave':
                        rows = [(1, 'pointer_leave', 1, 0)]
                    tracer = Mock(collect=Mock(side_effect=[trace(scroll_rows(1)) if failure == 'pre_activity' else trace(),
                                                           trace(rows)]))
                    guard = Mock(side_effect=AssertionError('primary changed') if failure == 'guard' else None)
                    save, record = Mock(), {'boundary': boundary}
                    passing = failure in (None, 'visible')
                    if not passing:
                        with self.assertRaises(Exception):
                            proof.action(actor, observer, spec, stage, tracer, guard, save, record, owner_lane=owner_lane)
                    else:
                        proof.action(actor, observer, spec, stage, tracer, guard, save, record, owner_lane=owner_lane)
                    attempted = failure not in ('stale', 'guard', 'pre_activity', 'dead_before', 'same_runtime', 'margin')
                    self.assertEqual(actor.tool.call_count, int(attempted))
                    self.assertFalse(record.get('replayed', False))
                    if spec['pointer_stage'] == 'scroll_visible':
                        choose.assert_called_once_with(before, 'pixels')
                    else:
                        choose.assert_not_called()
                    if failure == 'margin':
                        ground.assert_not_called()
                        self.assertEqual(record, {'boundary': boundary})
                    else:
                        ground.assert_called_once_with(before, 'pixels', 'inkscape', resolved)
                        self.assertEqual(record['tool'], 'scroll')
                        self.assertEqual(record['grounding']['requested_stage'], spec['pointer_stage'])
                    if attempted:
                        self.assertEqual(snapshots.call_args_list[0].args[:3], (actor, spec['target'], spec))
                        self.assertEqual(snapshots.call_args_list[1].args[:3], (observer, spec['target'], spec))
                        self.assertEqual(snapshots.call_args_list[1].kwargs, {'session': False})
                        self.assertEqual(actor.tool.call_args.args, ('scroll', {**arguments, 'pid': 20, 'window_id': 200,
                            'session': spec['name'], 'delivery_mode': 'background'}))
                    if passing:
                        self.assertEqual(record['outcome'], 'response')
                        if stage == 'refused':
                            self.assertTrue(record['no_effect']['verified'])
                            verify.assert_not_called()
                            self.assertNotIn('lane', record)
                        else:
                            verify.assert_called_once_with(after, 'pixels', oracle)
                            self.assertEqual((record['lane'], record['status_lane']), (1, 0))
                            self.assertEqual(record['trace_verification']['lane'], 1)
                    if failure == 'unknown':
                        self.assertEqual(record['outcome'], 'unknown')
                        self.assertIn('after', record)
                        self.assertEqual(record['observed_ns'], 40)
                        self.assertTrue(save.call_count >= 3)
                    if failure == 'observation':
                        self.assertEqual(record['observation_error'], 'snapshot unavailable')
                        self.assertIn('trace_after', record)
                        self.assertEqual(record['outcome'], 'response')
                    if failure == 'other_lane':
                        self.assertNotIn('trace_verification', record)


class RunTests(unittest.TestCase):
    def test_invalid_plan_preserves_raw_plan_and_cleanup_without_native_calls(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            path = directory / 'plan.json'
            candidate = {**plan(), 'case': 'initial_refusal'}
            path.write_text(json.dumps(candidate))
            args = SimpleNamespace(plan=path, evidence=directory / 'evidence')
            with patch.object(proof, 'ExactDesktop') as desktop, patch('builtins.print'):
                self.assertEqual(proof.run(args), 1)
            desktop.assert_not_called()
            self.assertEqual(json.loads((args.evidence / 'plan.json').read_text()), candidate)
            self.assertEqual(json.loads((args.evidence / 'cleanup.json').read_text()), {'errors': []})
            report = json.loads((args.evidence / 'result.json').read_text())
            self.assertEqual(report['result'], 'failed')
            self.assertEqual(report['active_lease_conflict'], 'unproven')
            self.assertEqual(report['other_lane_recovery'], 'unproven')

    def test_run_orders_owner_refusal_closes_and_recovery_reaping_all_children(self):
        for failure in (None, 'owner', 'refused', 'recovery', 'close', 'release', 'primary_event', 'reused',
                        'peer_reserved_after_close', 'hover_retired', 'close_side_effect', 'owner_hover_lost',
                        'start_lost', 'restarted_trace', 'dirty_preflight'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root, ExitStack() as stack:
                directory = Path(root)
                path = directory / 'plan.json'
                path.write_text(json.dumps(plan()))
                args = SimpleNamespace(plan=path, evidence=directory / 'evidence', driver=Path('/driver'),
                    primary_grab=Path('/grab'), trace_socket=Path('/cua-input-v3.sock'), foreground_journal=Path('/journal'))
                observer, owner, refused = client(101), client(102), client(103)
                recovery = client(103 if failure == 'reused' else 104)
                for actor in (owner, refused, recovery):
                    actor.tool.return_value = {}
                observer.tool.return_value = {'structuredContent': {'screen_width': 1600, 'screen_height': 900}}
                acted, closes, launched, rows = set(), [], [], []
                starts, tracing = [0], [False]
                def alive(value):
                    return value.process.poll() is None
                def raw_status():
                    value = status()
                    lanes = value['input']['lanes']
                    if failure == 'dirty_preflight' and not launched[1:]:
                        lanes[1]['pointer_focus'] = True
                    if 'owner' in acted:
                        lanes[0]['pointer_focus'] = not (failure == 'hover_retired' and not alive(owner))
                        lanes[0]['reserved'] = alive(owner) or ('recovery' in acted and alive(recovery))
                        if failure == 'owner_hover_lost' and 'refused' in acted:
                            lanes[0]['pointer_focus'] = False
                    if 'refused' in acted and (alive(refused) or failure == 'peer_reserved_after_close'):
                        lanes[1]['reserved'] = True
                    return value
                desktop = Mock()
                desktop.raw_status.side_effect = raw_status
                desktop.status.side_effect = lambda unreserved=False, allow_passive=False: proof.clear_status(
                    raw_status(), unreserved=unreserved, allow_passive=allow_passive)
                desktop.primary.side_effect = lambda target: {**target, 'cursor': {'x': 30, 'y': 40}, 'workspace': 1}
                stack.enter_context(patch.object(proof, 'ExactDesktop', return_value=desktop))
                stack.enter_context(patch.object(proof, 'app_process_identity'))
                stack.enter_context(patch.object(proof, 'provenance', return_value={'files': {}}))
                def launch(_driver, evidence_directory, profile):
                    self.assertEqual(profile, proof.PROFILE)
                    value = (observer, owner, refused, recovery)[len(launched)]
                    launched.append(evidence_directory.name)
                    return value
                stack.enter_context(patch.object(proof, 'DirectMCP', side_effect=launch))
                stack.enter_context(patch.object(proof, 'grounded_snapshot', return_value={'window_bounds': BOUNDS}))
                grab = Mock(poll=Mock(return_value=None))
                grabbed = [False]
                def start_grab(*_args, **_kwargs):
                    self.assertEqual(launched, ['observer'], 'agent runtime launched before the primary fixture')
                    self.assertEqual(starts[0], 0, 'primary fixture set up inside the trace')
                    grabbed[0] = True
                    return grab
                stack.enter_context(patch.object(proof.subprocess, 'Popen', side_effect=start_grab))
                grab.terminate.side_effect = lambda: setattr(grab.poll, 'return_value', 0)
                stack.enter_context(patch.object(proof, 'stop_process',
                    side_effect=RuntimeError('release failed') if failure == 'release' else None))
                stack.enter_context(patch.object(proof, 'primary_acknowledgement', return_value='HELD\n'))
                stack.enter_context(patch.object(proof, 'wait_for', side_effect=lambda check, **kwargs: check()))
                stack.enter_context(patch.object(proof, 'state', side_effect=lambda _: {
                    'held': grabbed[0] and grab.poll() is None, 'clicks': 0, 'keys': 0, 'scroll': 0}))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', return_value=0))
                def exchange(command):
                    if command == 'TRACE_START':
                        self.assertFalse(tracing[0])
                        self.assertTrue(grabbed[0], 'trace started before the primary fixture held')
                        starts[0] += 1
                        tracing[0] = True
                        if failure == 'start_lost':
                            raise TimeoutError('trace start acknowledgement lost')
                    elif command == 'TRACE_STOP':
                        tracing[0] = False
                def collect():
                    events = list(rows)
                    if not tracing[0] and failure == 'primary_event':
                        events.append((10, 'keyboard_key', 0, 1))
                    if not tracing[0] and failure == 'restarted_trace':
                        events.append((10, 'start', 0, 0))
                    return trace(events, active=tracing[0])
                tracer = Mock(exchange=Mock(side_effect=exchange), collect=Mock(side_effect=collect))
                desktop.trace.return_value = tracer
                def do_action(actor, obs, spec, stage, trace_obj, guard, save, record, owner_lane=None):
                    guard()
                    self.assertIs(obs, observer)
                    self.assertIn('boundary', record)
                    self.assertTrue(tracing[0])
                    if stage == 'owner':
                        self.assertIs(actor, owner)
                        self.assertIsNone(owner_lane)
                        self.assertEqual(spec['pointer_stage'], 'scroll_down')
                        self.assertEqual(launched, ['observer', 'owner'])
                    elif stage == 'refused':
                        self.assertIs(actor, refused)
                        self.assertEqual(owner_lane, 0)
                        self.assertTrue(alive(owner), 'owner closed before the refusal')
                        self.assertEqual(spec['pointer_stage'], 'scroll_up')
                        self.assertEqual(spec['name'], 'agent-conflict-refused')
                    else:
                        self.assertIs(actor, recovery)
                        self.assertEqual(owner_lane, 0)
                        self.assertEqual(closes, [refused, owner], 'recovery before B then A closed in order')
                        self.assertEqual(spec['pointer_stage'], 'scroll_visible')
                    acted.add(stage)
                    if stage != 'refused':
                        rows.extend(scroll_rows(1, len(rows) + 1))
                    record.update(trace_after=collect(), outcome='response', replayed=False,
                                  response=REFUSED if stage == 'refused' else DELIVERED)
                    if stage == 'owner':
                        record.update(lane=1, status_lane=0)
                    save(stage + '-action.json', record)
                    if failure == stage:
                        raise AssertionError(stage + ' failed')
                stack.enter_context(patch.object(proof, 'action', side_effect=do_action))
                def close(value):
                    value.process.poll.return_value = 0
                    closes.append(value)
                    if value is refused and failure == 'close_side_effect':
                        rows.append((9, 'pointer_leave', 1, 0))
                    if failure == 'close' and value is recovery:
                        raise RuntimeError('close failed')
                stack.enter_context(patch.object(proof, 'close_owned', side_effect=close))
                stack.enter_context(patch('builtins.print'))
                self.assertEqual(proof.run(args), 0 if failure is None else 1)
                report = json.loads((args.evidence / 'result.json').read_text())
                self.assertEqual(report['active_lease_conflict'], 'unproven')
                self.assertEqual(report['other_lane_recovery'], 'unproven')
                self.assertEqual(report['same_process_sibling_window'], 'unproven')
                self.assertTrue((args.evidence / 'provenance.json').exists(), report.get('error'))
                self.assertTrue((args.evidence / 'trace.json').exists() or failure == 'dirty_preflight', report.get('error'))
                if grabbed[0]:
                    self.assertEqual(grab.poll(), 0, 'primary grab not released')
                tracer.close.assert_called_once()
                self.assertFalse(tracing[0])
                self.assertTrue(all(not alive(value) for value in (owner, refused, recovery)[:max(len(launched) - 1, 0)]))
                self.assertEqual(closes[-1], observer)
                cleanup = json.loads((args.evidence / 'cleanup.json').read_text())['errors']
                if failure in ('release', 'close', 'primary_event', 'restarted_trace'):
                    self.assertTrue(report.get('error') or cleanup)
                if failure is None:
                    self.assertEqual(starts[0], 1)
                    self.assertEqual(launched, ['observer', 'owner', 'refused', 'recovery'])
                    # Cleanup re-closes every owned runtime; the ordered stage closes come first.
                    self.assertEqual(closes[:3], [refused, owner, recovery])
                    self.assertEqual(set(report['stages']),
                                     {'owner', 'refused', 'close_refused', 'close_owner', 'recovery', 'close_recovery'})
                    for name in ('owner-action.json', 'refused-action.json', 'recovery-action.json', 'close-refused.json',
                                 'close-owner.json', 'close-refused-trace.json', 'close-owner-trace.json', 'close-recovery.json'):
                        self.assertTrue((args.evidence / name).exists(), name)
                    self.assertEqual(report['stages']['close_refused']['teardown']['synthetic_events'], [])
                    self.assertTrue(report['stages']['close_owner']['status']['input']['lanes'][0]['pointer_focus'])
                    self.assertFalse(report['stages']['close_owner']['status']['input']['lanes'][0]['reserved'])
                    self.assertEqual(report['isolation']['result'], 'passed')
                    self.assertEqual(report['final_status']['input']['lanes'][0]['reserved'], False)
                if failure == 'owner':
                    self.assertEqual(launched, ['observer', 'owner'])
                    self.assertTrue((args.evidence / 'owner-action.json').exists())
                if failure in ('refused', 'owner_hover_lost'):
                    self.assertEqual(launched, ['observer', 'owner', 'refused'])
                    self.assertTrue((args.evidence / 'refused-action.json').exists())
                    self.assertEqual(report['stages']['refused']['response'], REFUSED)
                if failure == 'owner_hover_lost':
                    self.assertFalse(report['stages']['refused']['post_action_status']['input']['lanes'][0]['pointer_focus'])
                if failure in ('peer_reserved_after_close', 'close_side_effect', 'hover_retired'):
                    self.assertEqual(launched, ['observer', 'owner', 'refused'], 'recovery followed an unclean close')
                    self.assertEqual(closes[:2], [refused, owner], 'B must close before A')
                if failure == 'reused':
                    self.assertEqual(report['error']['message'], 'reused runtime process')
                    self.assertEqual(len(launched), 4)
                if failure == 'start_lost':
                    self.assertEqual(launched, ['observer'])
                    self.assertEqual(tracer.exchange.call_args_list[-1].args, ('TRACE_STOP',))
                if failure == 'dirty_preflight':
                    self.assertEqual(launched, ['observer'])
                    self.assertFalse(grabbed[0])
                    self.assertEqual(starts[0], 0)


if __name__ == '__main__':
    unittest.main()
