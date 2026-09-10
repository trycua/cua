"""Portable contracts only: no native VM, compositor, policy or input is touched."""
from contextlib import ExitStack, nullcontext
from copy import deepcopy
from itertools import count
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


def plan(kind='config_disable'):
    candidate = geometry_plan()
    candidate.update(purpose='desktop_fault', fault={'kind': 'config_disable'},
        vm={'machine_id': 'a' * 32, 'boot_id': 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'},
        compositor={'pid': 50, 'instance': 'test_1', 'uid': 1000, 'starttime': '77', 'exe': '/usr/bin/Hyprland'},
        config={'path': '/guest/input.lua', 'device': 1, 'inode': 2, 'uid': 1000,
                'mode': 0o600, 'sha256': proof.digest(proof.ENABLED.encode())})
    candidate['fault'] = {'kind': kind}
    candidate['config']['sha256'] = proof.digest(proof.fixed_bytes(kind)[0].encode())
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


def options(restored=True):
    values = {'kb_rules': 'evdev', 'kb_model': 'pc105', 'kb_layout': 'us' if restored else 'de',
              'kb_variant': '', 'kb_options': '', 'kb_file': ''}
    return {key: {'option': 'input:' + key, 'str': value, 'set': True} for key, value in values.items()}


def keymap_status(generation=2):
    result = status()
    for row in result['input']['lanes']:
        row.update(epoch='lane-' + str(row['lane']), desktop_generation=generation,
                   dispatches=1, reserved=False, pointer_focus=False, keyboard_focus=False)
    return result


def layout_refusal():
    return {'outcome': 'response', 'replayed': False, 'runtime_pid': 103, 'previous_runtime_pid': 100,
            'tool': 'click', 'prepared_ns': 10_100_000, 'dispatch_ns': 10_500_000, 'observed_ns': 11_000_000,
            'snapshot': {'window_bounds': dict(BOUNDS)}, 'after_snapshot': {'window_bounds': dict(BOUNDS)},
            'response': {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'unsupported_layout',
                'code': 'background_unavailable', 'detail': 'unsupported_layout', 'route': 'synthetic_events'}},
            'before': keymap_status(), 'after': keymap_status(), 'keymap_options': options(False),
            'trace_before': trace(CANCEL), 'trace_after': trace(CANCEL)}


def keymap_record():
    return {**record(), 'kind': 'keymap', 'config': {'sha256': proof.digest(proof.KEYMAP_DE.encode())},
            'before': keymap_status(1), 'gate_status': keymap_status(1), 'after': keymap_status(2),
            'keymap_before': options(), 'keymap_after': options(False), 'wrong_layout': layout_refusal()}


def keymap_restoration():
    return {**restoration(), 'config': {'sha256': proof.digest(proof.KEYMAP_US.encode())},
            'keymap_options': options(), 'status': keymap_status(3)}


def motion_trace(distance=12):
    page = trace(ACTIVE[:2] + [(2, 'pointer_enter', 1, 0)] + ACTIVE[2:])
    page['events'][2].extend([20, 30])
    page['events'][-1].extend([20 + distance, 30])
    return page


def target_snapshot(identity, *, runtime=100, observed_ns=1_000_000):
    return {'pid': 20, 'window_id': 200, 'window_bounds': dict(BOUNDS), 'snapshot_id': identity,
            'proof_runtime': {'pid': runtime, 'directory': f'/synthetic/runtime-{runtime}'},
            'proof_observation_started_ns': observed_ns, 'proof_observation_finished_ns': observed_ns + 100_000}


def retained_evidence(kind='keymap'):
    candidate = keymap_record() if kind == 'keymap' else record()
    restored = keymap_restoration() if kind == 'keymap' else restoration()
    candidate.update(pointer_cleanup='retained_inert', prefix=motion_trace(),
                     target={'pid': 20, 'window_id': 200}, bounds=dict(BOUNDS),
                     target_before=target_snapshot('before'), target_after=target_snapshot('after', runtime=101, observed_ns=14_000_000),
                     before=keymap_status(1), gate_status=keymap_status(1), after=keymap_status(2))
    candidate['gate_status']['input']['lanes'][0].update(
        held_button=272, drag_active=True, lease_active=True, pointer_focus=True, reserved=True)
    candidate['after']['input']['lanes'][0]['pointer_focus'] = True
    restored['status'] = keymap_status(3)
    restored['status']['input']['lanes'][0]['pointer_focus'] = True
    boundary = deepcopy(candidate['prefix'])
    boundary['events'] += [[7, 8_000_000, 'agent_cancel', 100, 100, 1, 0],
                           [8, 9_000_000, 'pointer_button', 100, 100, 1, 0]]
    boundary['count'] = len(boundary['events'])
    if kind == 'keymap':
        refused = candidate['wrong_layout']
        refused.update(pointer_cleanup='retained_inert', lane=1,
                       target=dict(candidate['target']), bounds=dict(BOUNDS),
                       snapshot=target_snapshot('refusal-before', runtime=103, observed_ns=10_200_000),
                       after_snapshot=target_snapshot('refusal-after', runtime=101, observed_ns=10_700_000),
                       before=deepcopy(candidate['after']), after=deepcopy(candidate['after']),
                       trace_before=deepcopy(boundary), trace_after=deepcopy(boundary))
    else:
        candidate['after']['configured'] = False
        candidate['after']['transport']['ready'] = False
        candidate['after']['input']['transport_ready'] = False
        for row in restored['status']['input']['lanes']:
            row['epoch'] += '-restored'
    candidate['target_status'] = deepcopy(restored['status'])
    return boundary, candidate, restored


class RetainedPointerTests(unittest.TestCase):
    def test_live_injection_retains_only_the_active_lane_for_both_faults(self):
        for kind in ('config_disable', 'keymap'):
            for failure in (None, 'gate_absent', 'after_absent', 'after_held'):
                with self.subTest(kind=kind, failure=failure), ExitStack() as stack:
                    _, candidate, _ = retained_evidence(kind)
                    fault = object.__new__(proof.ConfigFault)
                    fault.config = {'kind': kind, 'pointer_cleanup': 'retained_inert', 'instance': 'exact',
                                    'path': '/unused', 'deadline_ns': 10_000_000_000}
                    fault.child = Mock(poll=Mock(return_value=None))
                    fault.record = {'kind': kind, 'pointer_cleanup': 'retained_inert', 'before': candidate['before']}
                    fault.mutated = False
                    gate, after = candidate['gate_status'], candidate['after']
                    if failure == 'gate_absent':
                        gate['input']['lanes'][0]['pointer_focus'] = False
                    if failure == 'after_absent':
                        after['input']['lanes'][0]['pointer_focus'] = False
                    if failure == 'after_held':
                        after['input']['lanes'][0]['held_button'] = 272
                    stack.enter_context(patch.object(proof, 'production_status', return_value=gate))
                    stack.enter_context(patch.object(proof, 'keymap_options', side_effect=[options(), options(False)]))
                    stack.enter_context(patch.object(proof, '_locked', side_effect=lambda _: nullcontext()))
                    stack.enter_context(patch.object(proof, 'poll_fault_active', return_value=(candidate['prefix'], {1: 2})))
                    stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[5_000_000, 6_000_000, 10_000_000]))
                    atomic = stack.enter_context(patch.object(proof, '_replace', side_effect=lambda *_args, **kwargs: kwargs['before_replace']()))
                    stack.enter_context(patch.object(proof, '_reload', return_value=after))
                    stack.enter_context(patch.object(proof, 'file_identity', return_value=candidate['config']))
                    if failure:
                        with self.assertRaises(AssertionError):
                            fault.inject(Mock(), trace(ACTIVE[:1]), Mock(done=Mock(return_value=False)), Mock())
                    else:
                        fault.inject(Mock(), trace(ACTIVE[:1]), Mock(done=Mock(return_value=False)), Mock())
                        self.assertEqual(fault.config['lane'], 1)
                        self.assertEqual(fault.record['after']['input']['lanes'][0]['pointer_focus'], True)
                    self.assertEqual(atomic.call_count, int(failure != 'gate_absent'))

    def test_fresh_refusal_probe_threads_retained_policy_and_exact_target(self):
        for missing_focus in (False, True):
            with self.subTest(missing_focus=missing_focus), ExitStack() as stack:
                boundary, candidate, _ = retained_evidence()
                spec = plan('keymap')['agents'][0]
                fresh, observer, victim = client(103), client(101), client(100, alive=False)
                fresh.tool.side_effect = [{}, layout_refusal()['response']]
                before = deepcopy(candidate['after'])
                if missing_focus:
                    before['input']['lanes'][0]['pointer_focus'] = False
                stack.enter_context(patch.object(proof, 'production_status', side_effect=[before, candidate['after']]))
                stack.enter_context(patch.object(proof, 'app_process_identity'))
                stack.enter_context(patch.object(proof, 'grounded_snapshot', side_effect=[
                    {**target_snapshot('refusal-before', runtime=103, observed_ns=10_200_000), 'proof_image': '/synthetic/image'},
                    target_snapshot('refusal-after', runtime=101, observed_ns=10_700_000)]))
                stack.enter_context(patch.object(proof.pointer_grounding, 'read_pixels', return_value=[]))
                stack.enter_context(patch.object(proof.pointer_grounding, 'action', return_value=({'x': 20, 'y': 20}, {})))
                stack.enter_context(patch.object(proof, 'keymap_options', return_value=options(False)))
                stack.enter_context(patch.object(proof, '_guard'))
                stack.enter_context(patch.object(proof, 'file_identity', return_value={'inode': 20}))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[10_100_000, 10_500_000, 11_000_000]))
                config = {'instance': 'exact', 'path': '/unused', 'deadline_ns': 12_000_000_000,
                          'pointer_cleanup': 'retained_inert', 'lane': 1,
                          'files': {'disabled': {'identity': {'inode': 20}}}}
                args = (fresh, observer, victim, spec, 'click_b2', Mock(collect=Mock(return_value=boundary)), config, Mock(), Mock())
                if missing_focus:
                    with self.assertRaises(AssertionError):
                        proof.refuse_new_action(*args)
                    self.assertEqual(fresh.tool.call_count, 1)
                else:
                    observed = proof.refuse_new_action(*args)
                    self.assertEqual(observed['pointer_cleanup'], 'retained_inert')
                    self.assertEqual(observed['target'], spec['target'])
                    self.assertEqual(observed['verification']['result'], 'verified')

    def test_opt_in_and_legacy_default_are_distinct(self):
        for kind in ('config_disable', 'keymap'):
            for policy in ('cleared', 'retained_inert', None, True, 'retained', {}, []):
                candidate = plan(kind)
                candidate['fault']['pointer_cleanup'] = policy
                with self.subTest(kind=kind, policy=policy):
                    if policy in ('cleared', 'retained_inert'):
                        proof.validate_plan(candidate)
                    else:
                        with self.assertRaises(AssertionError):
                            proof.validate_plan(candidate)
            boundary, candidate, restored = retained_evidence(kind)
            result = proof.verify_fault(boundary, candidate, restored, action())
            self.assertEqual(result['pointer_cleanup']['presence_continuity'], 'verified')
            self.assertEqual(result['pointer_cleanup']['wayland_surface_identity'], 'not_exposed')
        boundary, candidate, restored = retained_evidence()
        del candidate['pointer_cleanup']
        with self.assertRaises(AssertionError):
            proof.verify_fault(boundary, candidate, restored, action())
        # The original saved evidence still uses its strict keymap interpretation.
        self.assertEqual(proof.verify_fault(trace(CANCEL), keymap_record(), keymap_restoration(), action())['result'], 'verified')

    def test_every_cleanup_status_requires_inert_authority_and_exact_pointer_presence(self):
        fields = [('held_button', 272), ('held_button', False), ('held_keys', 1), ('held_keys', True),
                  ('drag_active', True), ('lease_active', True), ('keyboard_focus', True),
                  ('reserved', True), ('reserved', None), ('pointer_focus', False),
                  ('pointer_focus', 1), ('pointer_focus', None), ('dispatches', 2)]
        for kind in ('config_disable', 'keymap'):
            stages = ['after', 'restored', 'target_status'] + (['refusal-before', 'refusal-after'] if kind == 'keymap' else [])
            for stage in stages:
                for key, value in fields:
                    boundary, candidate, restored = retained_evidence(kind)
                    observed = (restored['status'] if stage == 'restored' else
                                candidate['wrong_layout'][stage.split('-')[1]] if stage.startswith('refusal-') else candidate[stage])
                    observed['input']['lanes'][0][key] = value
                    with self.subTest(kind=kind, stage=stage, key=key, value=value), self.assertRaises(AssertionError):
                        proof.verify_fault(boundary, candidate, restored, action())
            boundary, candidate, restored = retained_evidence(kind)
            candidate['after']['input']['lanes'][1]['pointer_focus'] = True
            with self.assertRaisesRegex(AssertionError, 'presence'):
                proof.verify_fault(boundary, candidate, restored, action())

    def test_retained_trace_cannot_hide_leave_retarget_motion_new_input_or_missing_release(self):
        for kind in ('config_disable', 'keymap'):
            for event in ('pointer_leave', 'pointer_enter', 'pointer_motion', 'keyboard_enter',
                          'keyboard_key', 'pointer_axis', 'agent_admitted', 'agent_drag_end', 'agent_action_end'):
                boundary, candidate, restored = retained_evidence(kind)
                boundary['events'].append([9, 11_000_000, event, 100, 100, 1, 0] +
                                          ([32, 30] if event in ('pointer_enter', 'pointer_motion') else []))
                boundary['count'] += 1
                with self.subTest(kind=kind, event=event), self.assertRaises(AssertionError):
                    proof.verify_fault(boundary, candidate, restored, action())
            for missing in ('release', 'cancel', 'coordinates'):
                boundary, candidate, restored = retained_evidence(kind)
                if missing == 'coordinates':
                    del candidate['prefix']['events'][-1][7:9]
                    del boundary['events'][5][7:9]
                else:
                    boundary['events'].pop(-1 if missing == 'release' else -2)
                    boundary['count'] -= 1
                with self.subTest(kind=kind, missing=missing), self.assertRaises(AssertionError):
                    proof.verify_fault(boundary, candidate, restored, action())

    def test_target_process_window_geometry_and_snapshot_identity_are_required(self):
        for kind in ('config_disable', 'keymap'):
            for stage in ('target_before', 'target_after', 'snapshot', 'after_snapshot'):
                if kind == 'config_disable' and stage in ('snapshot', 'after_snapshot'):
                    continue
                for key, value in (('pid', 21), ('window_id', 201), ('window_id', None),
                                   ('window_bounds', {**BOUNDS, 'x': 999}), ('snapshot_id', '')):
                    boundary, candidate, restored = retained_evidence(kind)
                    snapshot = candidate[stage] if stage.startswith('target_') else candidate['wrong_layout'][stage]
                    snapshot[key] = value
                    with self.subTest(kind=kind, stage=stage, key=key), self.assertRaises(AssertionError):
                        proof.verify_fault(boundary, candidate, restored, action())
            boundary, candidate, restored = retained_evidence(kind)
            candidate['target_after']['snapshot_id'] = candidate['target_before']['snapshot_id']
            self.assertEqual(proof.verify_fault(boundary, candidate, restored, action())['result'], 'verified')
            candidate['target_after']['proof_runtime'] = deepcopy(candidate['target_before']['proof_runtime'])
            with self.assertRaisesRegex(AssertionError, 'reused'):
                proof.verify_fault(boundary, candidate, restored, action())

    def test_refusal_snapshot_ids_are_scoped_to_the_observer_and_require_new_observations(self):
        for same_runtime in (False, True):
            for same_id in (False, True):
                _, candidate, _ = retained_evidence()
                refusal = candidate['wrong_layout']
                if same_runtime:
                    refusal['after_snapshot']['proof_runtime'] = deepcopy(refusal['snapshot']['proof_runtime'])
                if same_id:
                    refusal['after_snapshot']['snapshot_id'] = refusal['snapshot']['snapshot_id']
                with self.subTest(same_runtime=same_runtime, same_id=same_id):
                    if same_runtime and same_id:
                        with self.assertRaisesRegex(AssertionError, 'reused'):
                            proof.verify_layout_refusal(refusal)
                    else:
                        self.assertEqual(proof.verify_layout_refusal(refusal)['result'], 'verified')

    def test_cross_runtime_counter_collision_cannot_hide_stale_or_invalid_timing(self):
        for key, value in (('proof_observation_started_ns', None), ('proof_observation_started_ns', True),
                           ('proof_observation_started_ns', -1), ('proof_observation_started_ns', 1_000_000),
                           ('proof_observation_started_ns', 1_050_000),
                           ('proof_observation_finished_ns', 13_000_000), ('proof_runtime', {}),
                           ('proof_runtime', {'pid': True}), ('proof_runtime', {'pid': 0})):
            boundary, candidate, restored = retained_evidence()
            candidate['target_after']['snapshot_id'] = candidate['target_before']['snapshot_id']
            candidate['target_after'][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(AssertionError):
                proof.verify_fault(boundary, candidate, restored, action())
        _, candidate, _ = retained_evidence()
        refusal = candidate['wrong_layout']
        refusal['after_snapshot']['snapshot_id'] = refusal['snapshot']['snapshot_id']
        refusal['after_snapshot']['proof_observation_started_ns'] = refusal['snapshot']['proof_observation_started_ns']
        with self.assertRaisesRegex(AssertionError, 'out-of-order'):
            proof.verify_layout_refusal(refusal)

    def test_refusal_has_no_synthetic_events_even_with_unchanged_status(self):
        for event in ('pointer_enter', 'pointer_leave', 'pointer_motion', 'pointer_button',
                      'agent_admitted', 'keyboard_leave'):
            _, candidate, _ = retained_evidence()
            refusal = candidate['wrong_layout']
            refusal['trace_after']['events'].append([9, 10_800_000, event, 100, 100, 1, 0])
            refusal['trace_after']['count'] += 1
            with self.subTest(event=event), self.assertRaisesRegex(AssertionError, 'synthetic input'):
                proof.verify_layout_refusal(refusal)

    def test_gate_epoch_generation_refusal_policy_and_lane_cannot_be_substituted(self):
        for change in ('gate', 'epoch', 'generation', 'policy', 'lane', 'target', 'dispatch'):
            boundary, candidate, restored = retained_evidence()
            if change == 'gate':
                candidate['gate_status']['input']['lanes'][0]['held_button'] = 0
            elif change in ('epoch', 'generation'):
                candidate['after']['input']['lanes'][0][
                    'epoch' if change == 'epoch' else 'desktop_generation'] = 'replacement' if change == 'epoch' else 1
            elif change == 'dispatch':
                candidate['gate_status']['input']['lanes'][0]['dispatches'] = 2
            else:
                candidate['wrong_layout'][{'policy': 'pointer_cleanup', 'lane': 'lane', 'target': 'target'}[change]] = {
                    'policy': 'cleared', 'lane': 2, 'target': {'pid': 21, 'window_id': 201}}[change]
            with self.subTest(change=change), self.assertRaises(AssertionError):
                proof.verify_fault(boundary, candidate, restored, action())


class MotionGateTests(unittest.TestCase):
    def test_plan_optional_positive_finite_threshold_for_both_faults(self):
        for kind in ('config_disable', 'keymap'):
            proof.validate_plan(plan(kind))
            for value in (12, 12.5, 0, -1, True, None, '12', float('nan'), float('inf')):
                candidate = plan(kind)
                candidate['fault']['min_motion_px'] = value
                with self.subTest(kind=kind, value=value):
                    if type(value) in (int, float) and value in (12, 12.5):
                        proof.validate_plan(candidate)
                    else:
                        with self.assertRaises(AssertionError):
                            proof.validate_plan(candidate)

    def test_omitted_threshold_preserves_held_only_gate(self):
        page = trace(ACTIVE[:4])
        transport = Mock(collect=Mock(return_value=page))
        self.assertEqual(proof.poll_fault_active(transport, trace(ACTIVE[:1]),
                         Mock(done=Mock(return_value=False))), (page, {1: 2_000_000}))

    def test_exact_threshold_waits_for_same_lane_surface_motion(self):
        first = motion_trace(11.999)
        enough = deepcopy(first)
        enough['events'].append([7, 4_500_000, 'pointer_motion', 100, 100, 1, 0, 32, 30])
        enough['count'] += 1
        transport = Mock(collect=Mock(side_effect=[first, enough]))
        pending = Mock(done=Mock(return_value=False))
        with patch.object(proof.time, 'sleep') as sleep:
            self.assertEqual(proof.poll_fault_active(transport, trace(ACTIVE[:1]), pending, 12),
                             (enough, {1: 2_000_000}))
        self.assertEqual(transport.collect.call_count, 2)
        self.assertEqual(pending.done.call_count, 2)
        sleep.assert_not_called()
        diagonal = motion_trace()
        diagonal['events'][-1][7:9] = [23, 34]
        self.assertEqual(proof.drag_motion_px(diagonal, 1), 5)

    def test_missing_or_insufficient_post_press_motion_expires_without_replay(self):
        for page in (motion_trace(11.999), {**motion_trace(), 'events': motion_trace()['events'][:-1], 'count': 5}):
            with self.subTest(page=page), patch.object(proof.time, 'monotonic', side_effect=lambda: next(ticks) / 1000):
                ticks = count()
                transport = Mock(collect=Mock(return_value=page))
                with self.assertRaisesRegex(AssertionError, 'bounded wait'):
                    proof.poll_fault_active(transport, trace(ACTIVE[:1]), Mock(done=Mock(return_value=False)), 12, timeout=.02)
                self.assertGreater(transport.collect.call_count, 1)

    def test_rejects_missing_coordinates_wrong_lane_changed_identity_and_ended_drag(self):
        for failure in ('coordinates', 'initial', 'foreign_lane', 'wrong_lane', 'leave', 'enter', 'release', 'cancel'):
            page = motion_trace()
            if failure == 'coordinates':
                del page['events'][-1][7:9]
            elif failure == 'initial':
                page['events'][2] = [3, 2_000_000, 'cursor', 100, 100, 0, 0]
            elif failure == 'foreign_lane':
                page['events'][-1][5] = 2
            elif failure not in ('wrong_lane',):
                kind, value = {'leave': ('pointer_leave', 0), 'enter': ('pointer_enter', 0),
                               'release': ('pointer_button', 0), 'cancel': ('agent_cancel', 0)}[failure]
                page['events'].append([7, 4_500_000, kind, 100, 100, 1, value] + ([32, 30] if failure == 'enter' else []))
                page['count'] += 1
            with self.subTest(failure=failure), self.assertRaises(AssertionError):
                proof.drag_motion_px(page, 2 if failure == 'wrong_lane' else 1)

    def test_poll_rejects_changed_trace_history_incomplete_and_stale_reads(self):
        first = motion_trace(1)
        changed = motion_trace(12)
        with self.assertRaisesRegex(AssertionError, 'history'):
            proof.poll_fault_active(Mock(collect=Mock(side_effect=[first, changed])), trace(ACTIVE[:1]),
                                    Mock(done=Mock(return_value=False)), 12)
        for field, value in (('overflow', True), ('timed_out', True), ('active', False), ('hook', False), ('count', 0)):
            with self.subTest(field=field), self.assertRaises(AssertionError):
                proof.poll_fault_active(Mock(collect=Mock(return_value={**motion_trace(), field: value})),
                                        trace(ACTIVE[:1]), Mock(done=Mock(return_value=False)), 12)
        with patch.object(proof.time, 'monotonic', side_effect=[0, .01, .02, .03, .04, .30]), \
             self.assertRaisesRegex(AssertionError, 'stale'):
            proof.poll_fault_active(Mock(collect=Mock(return_value=motion_trace())), trace(ACTIVE[:1]),
                                    Mock(done=Mock(return_value=False)), 12)

    def test_saved_fault_gate_rechecks_threshold_from_trace(self):
        candidate = {**record(), 'prefix': motion_trace(), 'min_motion_px': 12}
        boundary = motion_trace()
        boundary['events'] += [[i + 7, ms * 1_000_000, kind, 100, 100, lane, value]
                               for i, (ms, kind, lane, value) in enumerate(CANCEL[len(ACTIVE):])]
        boundary['count'] = len(boundary['events'])
        proof.verify_fault(boundary, candidate, restoration(), action())
        for threshold in (12.001, True, float('nan')):
            with self.subTest(threshold=threshold), self.assertRaises(AssertionError):
                proof.verify_fault(boundary, {**candidate, 'min_motion_px': threshold}, restoration(), action())


class KeymapTests(unittest.TestCase):
    def test_idle_baseline_allows_only_unreserved_inert_hover_on_either_lane(self):
        for lane in (0, 1):
            passive = keymap_status(1)
            passive['input']['lanes'][lane]['pointer_focus'] = True
            proof.idle_lanes(passive)
            proof.verify_keymap_transition(passive, keymap_status(2))
            with self.assertRaises(AssertionError):
                proof.keymap_lanes(passive, cleared=True)
            for key, value in (('held_button', 272), ('held_button', False), ('held_keys', 1),
                               ('drag_active', True), ('lease_active', True), ('keyboard_focus', True),
                               ('reserved', True), ('reserved', None), ('pointer_focus', 1),
                               ('pointer_focus', None)):
                changed = deepcopy(passive)
                changed['input']['lanes'][lane][key] = value
                with self.subTest(lane=lane, key=key, value=value), self.assertRaises(AssertionError):
                    proof.idle_lanes(changed)

    def test_keymap_constructor_preserves_and_checks_passive_baseline_before_staging(self):
        for reserved in (False, True):
            with self.subTest(reserved=reserved), tempfile.TemporaryDirectory() as root, ExitStack() as stack:
                directory = Path(root).resolve()
                path = directory / 'keymap.lua'
                path.write_text(proof.KEYMAP_US)
                path.chmod(0o600)
                candidate = plan('keymap')
                candidate['config'] = {'path': str(path), **proof.file_identity(path)}
                candidate['compositor']['uid'] = os.getuid()
                identity = {k: v for k, v in candidate['compositor'].items() if k != 'instance'}
                before = keymap_status(1)
                before['input']['lanes'][0].update(pointer_focus=True, reserved=reserved)
                stack.enter_context(patch.object(proof.platform, 'system', return_value='Linux'))
                stack.enter_context(patch.object(proof.subprocess, 'run', return_value=Mock(returncode=0)))
                stack.enter_context(patch.object(proof, '_identity', return_value=identity))
                stack.enter_context(patch.object(proof, '_guard'))
                stack.enter_context(patch.object(proof, 'production_status', return_value=before))
                stack.enter_context(patch.object(proof, 'keymap_options', return_value=options()))
                if reserved:
                    with patch.object(proof.tempfile, 'mkstemp') as stage, self.assertRaisesRegex(AssertionError, 'authority'):
                        proof.ConfigFault(candidate, directory)
                    stage.assert_not_called()
                else:
                    fault = proof.ConfigFault(candidate, directory)
                    fault.close()
                self.assertEqual(json.loads((directory / 'pre-fault-status.json').read_text()), before)
                self.assertEqual(path.read_text(), proof.KEYMAP_US)

    def test_keymap_plan_requires_its_own_exact_include(self):
        proof.validate_plan(plan('keymap'))
        for kind, data in (('keymap', proof.ENABLED), ('config_disable', proof.KEYMAP_US),
                           ('keymap', proof.KEYMAP_DE), ('keymap', proof.KEYMAP_US + '-- extra\n')):
            candidate = plan(kind)
            candidate['config']['sha256'] = proof.digest(data.encode())
            with self.subTest(kind=kind, data=data), self.assertRaises(AssertionError):
                proof.validate_plan(candidate)

    def test_compiled_map_gate_refusal_and_original_map_restoration_pass(self):
        self.assertEqual(proof.verify_fault(trace(CANCEL), keymap_record(), keymap_restoration(), action())['result'], 'verified')
        result = proof.verify_layout_refusal(layout_refusal())
        self.assertEqual(result['no_dispatch'], 'verified')
        self.assertEqual(result['keymap_hash'], 'not_exposed')

    def test_exact_option_readback_rejects_labels_variants_options_and_override(self):
        for key in options():
            candidate = options()
            candidate[key]['str'] = 'wrong'
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_keymap_options(candidate, True)
        for candidate in ({}, {'active_keymap': 'English (US)'}, {**options(), 'other': {}}):
            with self.assertRaises(AssertionError):
                proof.verify_keymap_options(candidate, True)
        with patch.object(proof, '_hypr', side_effect=lambda _, *_args: json.dumps(options(False)[_args[-1].split(':')[1]])) as hypr:
            self.assertEqual(proof.keymap_options('exact', False), options(False))
            self.assertEqual(hypr.call_count, 6)

    def test_transition_requires_real_generation_same_epoch_and_retired_authority(self):
        for key, value in (('desktop_generation', 1), ('desktop_generation', True), ('epoch', 'replaced'),
                           ('reserved', True), ('held_button', 272), ('held_button', False), ('held_keys', 1),
                           ('drag_active', True), ('lease_active', True), ('pointer_focus', True), ('keyboard_focus', True)):
            after = keymap_status(2)
            after['input']['lanes'][0][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_keymap_transition(keymap_status(1), after)

    def test_refusal_rejects_generic_error_unknown_replay_and_stale_runtime(self):
        for key, value in (('outcome', 'unknown'), ('replayed', True), ('runtime_pid', 100), ('tool', 'drag'),
                           ('dispatch_ns', 9_000_000), ('dispatch_ns', 99_000_000_000)):
            candidate = layout_refusal()
            candidate[key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_layout_refusal(candidate)
        for key, value in (('reason', 'unsupported_operation'), ('detail', 'wrong layout'), ('route', 'foreground'),
                           ('effect', 'partial'), ('delivery', {'mode': 'background'}), ('code', 'unsupported_layout')):
            candidate = layout_refusal()
            candidate['response']['structuredContent'][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_layout_refusal(candidate)

    def test_refusal_requires_no_dispatch_and_no_primary_disturbance(self):
        candidate = layout_refusal()
        candidate['after_snapshot']['window_bounds']['x'] += 1
        with self.assertRaises(AssertionError):
            proof.verify_layout_refusal(candidate)
        for kind, lane in (('agent_admitted', 1), ('pointer_button', 1), ('pointer_axis', 2),
                           ('keyboard_key', 0), ('pointer_focus', 0)):
            candidate = layout_refusal()
            candidate['trace_after'] = trace(CANCEL + [(11, kind, lane, 0)])
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                proof.verify_layout_refusal(candidate)
        for key, value in (('dispatches', 2), ('desktop_generation', 3), ('epoch', 'replaced'), ('held_button', 1)):
            candidate = layout_refusal()
            candidate['after']['input']['lanes'][1][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.verify_layout_refusal(candidate)
        candidate = layout_refusal()
        candidate['trace_after']['events'][0][3] += 1
        with self.assertRaises(AssertionError):
            proof.verify_layout_refusal(candidate)

    def test_restoration_requires_original_options_and_new_generation(self):
        for mutate in (lambda r: r.update(keymap_options=options(False)),
                       lambda r: r.update(status=keymap_status(2)),
                       lambda r: r.update(config={'sha256': proof.digest(proof.ENABLED.encode())})):
            restored = keymap_restoration()
            mutate(restored)
            with self.assertRaises(AssertionError):
                proof.verify_fault(trace(CANCEL), keymap_record(), restored, action())
        for mutate in (lambda r: r['wrong_layout'].update(observed_ns=13_000_000),
                       lambda r: r['wrong_layout'].update(keymap_options=options()),
                       lambda r: r.update(after=keymap_status(1))):
            candidate = keymap_record()
            mutate(candidate)
            with self.assertRaises(AssertionError):
                proof.verify_fault(trace(CANCEL), candidate, keymap_restoration(), action())

    def test_new_driver_action_is_fresh_guarded_and_preserves_failed_evidence(self):
        for failure in (None, 'runtime', 'stale', 'watchdog', 'file', 'held', 'unknown', 'generic', 'dispatch',
                        'expired_during_snapshot', 'restored_during_snapshot'):
            with self.subTest(failure=failure), ExitStack() as stack:
                candidate = plan('keymap')
                spec = candidate['agents'][0]
                fresh, observer, victim = client(103), client(101), client(100, alive=False)
                if failure == 'runtime':
                    victim.process.poll.return_value = None
                response = layout_refusal()['response']
                if failure == 'generic':
                    response['structuredContent']['reason'] = 'unsupported_operation'
                fresh.tool.side_effect = [{}, TimeoutError('lost response') if failure == 'unknown' else response]
                page = trace(CANCEL + [(11, 'pointer_button', 1, 1)]) if failure == 'dispatch' else trace(CANCEL)
                transport = Mock(collect=Mock(side_effect=[trace(CANCEL), page]))
                before = keymap_status()
                if failure == 'held':
                    before['input']['lanes'][0]['held_button'] = 272
                stack.enter_context(patch.object(proof, 'production_status', side_effect=[before, keymap_status()]))
                stack.enter_context(patch.object(proof, 'app_process_identity'))
                snapshot = stack.enter_context(patch.object(proof, 'grounded_snapshot', return_value={
                    'proof_image': '/synthetic/image', 'window_bounds': dict(BOUNDS)}))
                stack.enter_context(patch.object(proof.pointer_grounding, 'read_pixels', return_value=[]))
                stack.enter_context(patch.object(proof.pointer_grounding, 'action', return_value=({'x': 20, 'y': 20}, {})))
                stack.enter_context(patch.object(proof, 'keymap_options', side_effect=[options(False),
                    AssertionError('watchdog restored keymap') if failure == 'restored_during_snapshot' else options(False)]))
                stack.enter_context(patch.object(proof, '_guard'))
                stack.enter_context(patch.object(proof, 'file_identity', return_value={} if failure == 'file' else {'inode': 20}))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[10_100_000,
                    99_000_000_000 if failure == 'stale' else 10_500_000,
                    13_000_000_000 if failure == 'expired_during_snapshot' else 11_000_000]))
                config = {'instance': 'exact', 'path': '/unused', 'deadline_ns': 11_000_000 if failure == 'watchdog' else 12_000_000_000,
                          'files': {'disabled': {'identity': {'inode': 20}}}}
                saved = {}
                def save(name, record):
                    saved[name] = deepcopy(record)
                if failure:
                    with self.assertRaises((AssertionError, TimeoutError)):
                        proof.refuse_new_action(fresh, observer, victim, spec, 'click_b2', transport, config, Mock(), save)
                else:
                    result = proof.refuse_new_action(fresh, observer, victim, spec, 'click_b2', transport, config, Mock(), save)
                    self.assertEqual(result['verification']['no_dispatch'], 'verified')
                    self.assertEqual(fresh.tool.call_args.args[0], 'click')
                    self.assertEqual(fresh.tool.call_args.args[1]['session'], spec['name'] + '-wrong-layout')
                    self.assertEqual(snapshot.call_args_list[0].args[0], fresh)
                    self.assertEqual(snapshot.call_args_list[1].args[0], observer)
                    self.assertEqual(snapshot.call_args_list[1].kwargs, {'session': False})
                    self.assertEqual(result['snapshot']['window_bounds'], result['after_snapshot']['window_bounds'])
                if failure in ('runtime', 'stale', 'watchdog', 'file', 'held'):
                    self.assertFalse(any(call.args[0] == 'click' for call in fresh.tool.call_args_list))
                if failure in ('unknown', 'generic', 'dispatch', 'expired_during_snapshot', 'restored_during_snapshot'):
                    self.assertIn('wrong-layout-action.json', saved)
                    self.assertNotIn('verification', saved['wrong-layout-action.json'])


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

    def prepare_files(self, directory, kind='config_disable'):
        config = {'kind': kind, 'path': str(directory / 'input.lua'), 'files': {}}
        original, changed = proof.fixed_bytes(kind)
        for name, data in (('original', original), ('disabled', changed), ('restored', original)):
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

    def test_keymap_atomic_replacement_restoration_and_guarded_failures(self):
        for failure in (None, 'unrelated', 'wrong_stage', 'reload'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as root, ExitStack() as stack:
                config = self.prepare_files(Path(root).resolve(), 'keymap')
                config['instance'] = 'exact'
                stack.enter_context(patch.object(proof, '_guard'))
                stack.enter_context(patch.object(proof, '_locked', side_effect=lambda _: nullcontext()))
                stack.enter_context(patch.object(proof, '_reload', side_effect=TimeoutError('lost') if failure == 'reload' else lambda *_: keymap_status(3)))
                readback = stack.enter_context(patch.object(proof, 'keymap_options', return_value=options()))
                proof._replace(config, False)
                self.assertEqual(Path(config['path']).read_bytes(), proof.KEYMAP_DE.encode())
                if failure == 'unrelated':
                    Path(config['path']).write_bytes(proof.DISABLED.encode())
                if failure == 'wrong_stage':
                    Path(config['files']['restored']['path']).write_bytes(proof.ENABLED.encode())
                if failure:
                    with self.assertRaises((AssertionError, TimeoutError)):
                        proof.restore_config(config)
                    readback.assert_not_called()
                else:
                    restored = proof.restore_config(config)
                    self.assertEqual(restored['keymap_options'], options())
                    self.assertEqual(Path(config['path']).read_bytes(), proof.KEYMAP_US.encode())
                    proof._replace(config, True)

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
            candidate['fault']['min_motion_px'] = 12
            candidate['fault']['pointer_cleanup'] = 'retained_inert'
            candidate['config'] = {'path': str(path), **proof.file_identity(path)}
            candidate['compositor']['uid'] = os.getuid()
            identity = {k: v for k, v in candidate['compositor'].items() if k != 'instance'}
            stack.enter_context(patch.object(proof.platform, 'system', return_value='Linux'))
            stack.enter_context(patch.object(proof.subprocess, 'run', return_value=Mock(returncode=0)))
            stack.enter_context(patch.object(proof, '_identity', return_value=identity))
            stack.enter_context(patch.object(proof, '_guard'))
            stack.enter_context(patch.object(proof, 'production_status', return_value=keymap_status(1)))
            reload = stack.enter_context(patch.object(proof, '_reload'))
            fault = proof.ConfigFault(candidate, directory)
            self.assertEqual(fault.config['min_motion_px'], 12)
            self.assertEqual(fault.record['min_motion_px'], 12)
            self.assertEqual(fault.config['pointer_cleanup'], 'retained_inert')
            self.assertEqual(fault.record['pointer_cleanup'], 'retained_inert')
            self.assertEqual(fault.record['target'], candidate['agents'][0]['target'])
            original = proof.file_identity(path)
            for name, data in (('disabled', proof.DISABLED), ('restored', proof.ENABLED)):
                staged = Path(fault.config['files'][name]['path'])
                self.assertEqual(staged.read_bytes(), data.encode())
                self.assertEqual(staged.suffix, '.stage')
            fault.close()
            self.assertEqual(proof.file_identity(path), original)
            self.assertEqual(set(directory.iterdir()), {path, directory / 'pre-fault-status.json'})
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

    def test_watchdog_has_fixed_bounded_budget_for_each_fault(self):
        for kind, seconds in (('config_disable', 12), ('keymap', 30)):
            with self.subTest(kind=kind), ExitStack() as stack:
                fault = self.controller()
                fault.config['kind'] = kind
                fault.child = None
                child = Mock()
                child.stdout.readline.return_value = 'ARMED\n'
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', return_value=100))
                stack.enter_context(patch.object(proof.os, 'pipe', return_value=(7, 8)))
                close = stack.enter_context(patch.object(proof.os, 'close'))
                launch = stack.enter_context(patch.object(proof.subprocess, 'Popen', return_value=child))
                stack.enter_context(patch.object(proof.select, 'select', return_value=([child.stdout], [], [])))
                fault.arm()
                self.assertEqual(fault.config['deadline_ns'], 100 + seconds * 1_000_000_000)
                self.assertLess(seconds * 1000, proof.PRIMARY_LIFETIME_MS)
                self.assertEqual(json.loads(launch.call_args.args[0][3])['deadline_ns'], fault.config['deadline_ns'])
                close.assert_called_once_with(7)
                with self.assertRaises(AssertionError):
                    fault.arm()

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

    def test_motion_gate_controls_both_faults_without_replay(self):
        for kind in ('config_disable', 'keymap'):
            for failure in (None, 'done', 'done_after_motion', 'coordinates', 'identity', 'insufficient'):
                with self.subTest(kind=kind, failure=failure), ExitStack() as stack:
                    fault = self.controller()
                    fault.config.update(kind=kind, min_motion_px=12, instance='exact')
                    page = motion_trace(1 if failure == 'insufficient' else 12)
                    if failure == 'coordinates':
                        del page['events'][-1][7:9]
                    if failure == 'identity':
                        page['events'][-1][5] = 2
                    ticks = count()
                    stack.enter_context(patch.object(proof.time, 'monotonic', side_effect=lambda: next(ticks) / 100))
                    stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[5_000_000, 6_000_000, 10_000_000]))
                    stack.enter_context(patch.object(proof, '_locked', side_effect=lambda _: nullcontext()))
                    gate = keymap_status(1)
                    gate['input']['lanes'][0].update(drag_active=True, lease_active=True, held_button=272)
                    fault.record['before'] = keymap_status(1)
                    stack.enter_context(patch.object(proof, 'production_status', return_value=gate))
                    stack.enter_context(patch.object(proof, 'keymap_options', side_effect=[options(), options(False)]))
                    stack.enter_context(patch.object(proof, 'file_identity', return_value={}))
                    # Match _replace's authorization boundary, without touching a file.
                    atomic = Mock()
                    def replace(*_args, **kwargs):
                        kwargs['before_replace']()
                        atomic()
                    stack.enter_context(patch.object(proof, '_replace', side_effect=replace))
                    reload = stack.enter_context(patch.object(proof, '_reload', return_value=keymap_status(2)
                                                             if kind == 'keymap' else status(False)))
                    pending = Mock(done=Mock(side_effect=[False, True]) if failure == 'done_after_motion'
                                   else Mock(return_value=failure == 'done'))
                    if failure:
                        with self.assertRaises(AssertionError):
                            fault.inject(Mock(collect=Mock(return_value=page)), trace(ACTIVE[:1]), pending, Mock())
                        atomic.assert_not_called()
                        reload.assert_not_called()
                        self.assertFalse(fault.mutated)
                    else:
                        fault.inject(Mock(collect=Mock(return_value=page)), trace(ACTIVE[:1]), pending, Mock())
                        atomic.assert_called_once()
                        reload.assert_called_once()
                        self.assertTrue(fault.mutated)
                        self.assertEqual(fault.record['prefix'], page)

    def test_keymap_injection_checks_active_native_gate_and_observed_generation(self):
        for failure in (None, 'gate_changed', 'gate_released', 'not_compiled', 'wrong_options'):
            with self.subTest(failure=failure), ExitStack() as stack:
                fault = self.controller()
                fault.config.update(kind='keymap', instance='exact')
                fault.record = {'kind': 'keymap', 'result': 'unproven', 'before': keymap_status(1)}
                gate = keymap_status(2 if failure == 'gate_changed' else 1)
                gate['input']['lanes'][0].update(drag_active=True, lease_active=True,
                                                held_button=0 if failure == 'gate_released' else 272)
                stack.enter_context(patch.object(proof, 'production_status', return_value=gate))
                stack.enter_context(patch.object(proof, 'keymap_options', side_effect=[options(),
                    AssertionError('wrong options') if failure == 'wrong_options' else options(False)]))
                stack.enter_context(patch.object(proof, '_locked', side_effect=lambda _: nullcontext()))
                stack.enter_context(patch.object(proof, 'poll_active', return_value=(trace(ACTIVE), {1: 2})))
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[5, 6, 7]))
                replace = stack.enter_context(patch.object(proof, '_replace', side_effect=lambda *_args, **kwargs: kwargs['before_replace']()))
                stack.enter_context(patch.object(proof, '_reload', return_value=keymap_status(1 if failure == 'not_compiled' else 2)))
                stack.enter_context(patch.object(proof, 'file_identity', return_value={'sha256': proof.digest(proof.KEYMAP_DE.encode())}))
                if failure:
                    with self.assertRaises(AssertionError):
                        fault.inject(Mock(), trace(ACTIVE[:1]), Mock(done=Mock(return_value=False)), Mock())
                else:
                    fault.inject(Mock(), trace(ACTIVE[:1]), Mock(done=Mock(return_value=False)), Mock())
                    self.assertEqual(fault.record['result'], 'observed')
                    self.assertEqual(fault.record['keymap_after'], options(False))
                self.assertEqual(replace.call_count, int(failure not in ('gate_changed', 'gate_released')))
                self.assertEqual(fault.mutated, failure not in ('gate_changed', 'gate_released'))

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
        cases = [(kind, failure, 'cleared') for kind in ('config_disable', 'keymap') for failure in (None, 'inject', 'restore', 'recovery')]
        cases += [('keymap', 'refusal', 'cleared'), ('keymap', 'cancel', 'cleared')]
        cases += [(kind, failure, 'retained_inert') for kind in ('config_disable', 'keymap')
                  for failure in (None, 'recovery', 'snapshot_leave')]
        for kind, failure, policy in cases:
            with self.subTest(kind=kind, failure=failure, policy=policy), tempfile.TemporaryDirectory() as root, ExitStack() as stack:
                directory = Path(root)
                path = directory / 'plan.json'
                candidate = plan(kind)
                if policy == 'retained_inert':
                    candidate['fault']['pointer_cleanup'] = policy
                path.write_text(json.dumps(candidate))
                args = SimpleNamespace(plan=path, evidence=directory / 'evidence', driver=Path('/driver'),
                    primary_grab=Path('/grab'), foreground_journal=Path('/journal'), trace_socket=Path('/cua-input-v3.sock'))
                order = []
                cancelled = trace(CANCEL)
                fault_record = keymap_record() if kind == 'keymap' else record()
                restored = keymap_restoration() if kind == 'keymap' else restoration()
                if policy == 'retained_inert':
                    cancelled, fault_record, restored = retained_evidence(kind)
                fault = Mock(record=fault_record, config={'instance': 'exact', 'pointer_cleanup': policy, 'lane': 1})
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
                    return restored
                fault.restore.side_effect = restore
                stack.enter_context(patch.object(proof, 'ConfigFault', return_value=fault))
                stack.enter_context(patch.object(proof, 'provenance', return_value={'files': {}}))
                agent, observer, fresh, refused = client(100), client(101), client(102), client(103)
                agent.tool.return_value = {}
                observer.tool.return_value = {'structuredContent': {'screen_width': 1920, 'screen_height': 1080}}
                stack.enter_context(patch.object(proof, 'DirectMCP', side_effect=[agent, observer, refused, fresh] if kind == 'keymap' else [agent, observer, fresh]))
                def snapshot(*_args, **_kwargs):
                    order.append('snapshot')
                    return target_snapshot('snapshot-' + str(len(order)), runtime=101, observed_ns=14_000_000)
                stack.enter_context(patch.object(proof, 'grounded_snapshot', side_effect=snapshot))
                stack.enter_context(patch.object(proof, 'prepare_drag', return_value={'snapshot': target_snapshot('prepared')}))
                stack.enter_context(patch.object(proof, 'production_status', return_value=restored['status']))
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
                stack.enter_context(patch.object(proof.time, 'monotonic_ns', side_effect=[0, 0, 0, 11_500_000]))
                first = Mock(hello={'protocol': 3}, collect=Mock(side_effect=[trace(ACTIVE[:1]), trace(ACTIVE) if failure == 'cancel' else cancelled]))
                page = deepcopy(cancelled)
                for ms, event, value in ((14, 'agent_admitted', 0), (15, 'pointer_button', 1),
                                         (16, 'pointer_button', 0), (17, 'agent_action_end', 0)):
                    page['events'].append([len(page['events']) + 1, ms * 1_000_000, event, 100, 100, 1, value])
                page['count'] = len(page['events'])
                last = proof.stopped_prefix(page if failure is None else cancelled)
                observations = [cancelled, cancelled, last]
                if policy == 'retained_inert':
                    observed = deepcopy(cancelled)
                    if failure == 'snapshot_leave':
                        observed['events'].append([9, 14_000_000, 'pointer_leave', 100, 100, 1, 0])
                        observed['count'] += 1
                    observations = [cancelled, observed, cancelled, last]
                second = Mock(hello={'protocol': 3}, collect=Mock(side_effect=observations))
                stack.enter_context(patch.object(proof, 'connect_trace', side_effect=[first, second]))
                def refuse(*args):
                    order.append('refusal')
                    self.assertEqual(agent.process.poll(), 0)
                    if failure == 'refusal':
                        raise AssertionError('wrong layout did not refuse')
                    return fault_record['wrong_layout'] if policy == 'retained_inert' else layout_refusal()
                probe = stack.enter_context(patch.object(proof, 'refuse_new_action', side_effect=refuse))
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
                if failure not in ('inject', 'restore', 'refusal', 'cancel', 'snapshot_leave'):
                    self.assertLess(order.index('restore'), len(order) - 1 - order[::-1].index('snapshot'))
                    self.assertLess(order.index('restore'), order.index('recovery'))
                    self.assertTrue((args.evidence / 'pre-recovery-prefix.json').is_file())
                if kind == 'keymap' and failure not in ('inject', 'cancel'):
                    self.assertLess(order.index('refusal'), order.index('restore'))
                    self.assertEqual(refused.process.poll(), 0)
                if kind == 'config_disable' or failure in ('inject', 'cancel'):
                    probe.assert_not_called()
                if policy == 'retained_inert' and failure != 'snapshot_leave':
                    self.assertEqual(fault.record['target_before']['snapshot_id'], 'prepared')
                    self.assertEqual(fault.record['target_after']['pid'], candidate['agents'][0]['target']['pid'])
                    self.assertTrue((args.evidence / 'interrupted-status.json').is_file())
                report = json.loads((args.evidence / 'result.json').read_text())
                self.assertFalse(report['full_desktop_matrix'])
                self.assertFalse(report['physical_hardware'])


if __name__ == '__main__':
    unittest.main()
