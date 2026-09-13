"""Mocked proof-contract tests only; these do not certify a native desktop."""
from contextlib import ExitStack
import copy
import io
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import production_idle_reconnect_proof as proof


BOUNDS = {'x': 0, 'y': 0, 'width': 800, 'height': 600}
RESPONSE = {'structuredContent': {'effect': 'unverifiable', 'route': 'synthetic_events',
                                   'delivery': {'mode': 'background'}}}


def plan():
    return {'purpose': 'idle_reconnect', 'package_versions': {},
            'foreground': {'pid': 10, 'window_id': 100}, 'primary_point': [100, 100],
            'agents': [{'app': 'calc', 'name': 'idle-proof', 'target': {'pid': 20, 'window_id': 200},
                        'bounds': dict(BOUNDS), 'document': '/synthetic/cua-smoke-calc.ods',
                        'profile': dict(proof.PROFILE)}]}


def status(reserved=True, dispatches=1):
    return {'state': 'input_v3_candidate', 'input': {
        'protocol': 3, 'test_only': False, 'transport_ready': True,
        'lanes': [{'lane': lane, 'epoch': f'epoch-{lane}', 'desktop_generation': 2,
                   'reserved': reserved if lane == 0 else False,
                   'pointer_focus': dispatches > 0 if lane == 0 else False,
                   'lease_active': False, 'drag_active': False, 'held_button': 0,
                   'held_keys': 0, 'keyboard_focus': False, 'dispatches': dispatches if lane == 0 else 0}
                  for lane in (0, 1)]}}


def client(pid=99):
    value = Mock(counter=7, failed=False, closed=False)
    value.process.pid = pid
    value.process.poll.return_value = None
    value.tool.return_value = copy.deepcopy(RESPONSE)
    return value


def runtime(value):
    return {'process': value.process, 'pid': value.process.pid}


class Clock:
    def __init__(self, seconds=0):
        self.ns = seconds * 1_000_000_000

    def now(self):
        return self.ns

    def sleep(self, seconds):
        self.ns += int(seconds * 1_000_000_000)


class ExpiryTests(unittest.TestCase):
    def poll(self, read_status, value=None, seconds=0):
        value = value or client()
        clock = Clock(seconds)
        return proof.wait_for_idle_expiry(value, runtime(value), status(), 1, 0, Mock(),
                                          read_status=read_status(clock, value),
                                          now=clock.now, sleep=clock.sleep)

    def test_actual_sixty_second_expiry_does_not_call_runtime(self):
        value = client()
        result = self.poll(lambda clock, _: lambda: status(clock.ns < 60_000_000_000), value)
        self.assertEqual(result['elapsed_ns'], 60_000_000_000)
        self.assertEqual(result['runtime_pid'], 99)
        value.tool.assert_not_called()
        value.close.assert_not_called()

    def test_expiry_timeout_is_bounded_and_never_sends_input(self):
        value = client()
        clock = Clock()
        with self.assertRaisesRegex(AssertionError, '85 seconds'):
            proof.wait_for_idle_expiry(value, runtime(value), status(), 1, 0, Mock(),
                                      read_status=lambda: status(), now=clock.now, sleep=clock.sleep)
        self.assertLessEqual(clock.ns + proof.STATUS_READ_BUDGET_NS - 1_000_000_000, 85_000_000_000)
        self.assertGreaterEqual(clock.ns, 60_000_000_000)
        value.tool.assert_not_called()

    def test_early_disconnect_and_ambiguous_expiry_fail_closed(self):
        cases = []
        early = status(False)
        cases.append((early, 59))
        for field, changed in [('pointer_focus', False), ('epoch', 'replacement'),
                               ('desktop_generation', 3), ('dispatches', 2), ('held_button', 272),
                               ('lease_active', True), ('reserved', 'false')]:
            malformed = status(False)
            malformed['input']['lanes'][0][field] = changed
            cases.append((malformed, 61))
        for observed, seconds in cases:
            with self.subTest(status=observed, seconds=seconds), self.assertRaises(AssertionError):
                self.poll(lambda _clock, _client: lambda: observed, seconds=seconds)

    def test_runtime_exit_or_mcp_activity_during_idle_is_not_expiry(self):
        def exited(_clock, value):
            def read():
                value.process.poll.return_value = 1
                return status(False)
            return read
        with self.assertRaisesRegex(AssertionError, 'survive'):
            self.poll(exited, seconds=61)
        def activity(_clock, value):
            def read():
                value.counter += 1
                return status()
            return read
        with self.assertRaisesRegex(AssertionError, 'another MCP'):
            self.poll(activity)


class ClickTests(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.client, self.observer = client(), client(100)
        self.spec = plan()['agents'][0]
        self.identity = {'pid': 20, 'start_ticks': 300}
        self.snapshot = {'proof_observation_started_ns': 1_000_000_000,
                         'proof_image': 'synthetic.png', 'window_bounds': BOUNDS}
        def mock(name, **kwargs):
            return self.stack.enter_context(patch.object(proof, name, **kwargs))
        self.identity_mock = mock('calc_identity', return_value=self.identity)
        self.birth = mock('process_birth', return_value=300)
        self.snapshot_mock = mock('grounded_snapshot', side_effect=[self.snapshot, self.snapshot])
        self.clock = self.stack.enter_context(patch.object(proof.time, 'monotonic_ns', return_value=2_000_000_000))
        self.stack.enter_context(patch.object(proof.grounding, 'read_pixels', return_value=object()))
        self.stack.enter_context(patch.object(proof.grounding, 'action', return_value=(
            {'x': 20, 'y': 30}, {'app': 'calc', 'stage': 'click_b2', 'selection': 'B2'})))
        self.verify = self.stack.enter_context(patch.object(proof.grounding, 'verify', return_value={'verified': True}))
        self.digest = mock('grid_digest', side_effect=['before-pixels', 'after-pixels'])
        mock('capacity_lane', return_value=1)
        mock('verify_recovery_trace', return_value={'result': 'verified'})
        self.trace, self.result = Mock(), {}

    def call(self):
        return proof.click_once(self.client, self.observer, self.spec, 'click_b2', runtime(self.client),
                                self.identity, self.trace, {}, Mock(), self.result, Mock())

    def test_one_normal_action_in_same_named_session(self):
        self.call()
        self.client.tool.assert_called_once_with('click', {
            'x': 20, 'y': 30, 'pid': 20, 'window_id': 200, 'session': 'idle-proof',
            'delivery_mode': 'background'})
        self.assertEqual(self.result['action']['attempts'], 1)
        self.assertFalse(self.result['replayed'])
        self.assertEqual(self.snapshot_mock.call_args_list[0].args[0], self.client)
        self.assertEqual(self.snapshot_mock.call_args_list[1].args[0], self.observer)

    def test_inkscape_scrolls_require_effects_and_use_scroll_trace_contract(self):
        self.spec.update(app='inkscape', document='/synthetic/cua-smoke-inkscape.svg')
        for stage, changed in (('scroll_down', True), ('scroll_up', True), ('scroll_down', False)):
            with self.subTest(stage=stage, changed=changed):
                self.snapshot_mock.side_effect = [self.snapshot, self.snapshot]
                self.digest.side_effect = ['before', 'after' if changed else 'before']
                self.client.tool.reset_mock()
                self.result.clear()
                if changed:
                    proof.click_once(self.client, self.observer, self.spec, stage, runtime(self.client),
                                     self.identity, self.trace, {}, Mock(), self.result, Mock())
                    proof.capacity_lane.assert_called_with({}, self.trace.collect.return_value, 'scroll')
                    proof.verify_recovery_trace.assert_called_with({}, self.trace.collect.return_value, 1, 'scroll')
                    self.assertTrue(self.result['app_effect']['verified'])
                else:
                    with self.assertRaisesRegex(AssertionError, 'pixels did not change'):
                        proof.click_once(self.client, self.observer, self.spec, stage, runtime(self.client),
                                         self.identity, self.trace, {}, Mock(), self.result, Mock())
                self.client.tool.assert_called_once()
                self.assertEqual(self.client.tool.call_args.args[0], 'scroll')
                self.assertFalse(self.result['action']['replayed'])

    def test_unknown_outcome_is_observed_but_never_replayed(self):
        self.client.tool.side_effect = RuntimeError('closed after possible delivery')
        with self.assertRaisesRegex(AssertionError, 'unknown; no replay'):
            self.call()
        self.client.tool.assert_called_once()
        self.assertEqual(self.snapshot_mock.call_count, 2)
        self.assertEqual(self.result['action']['outcome'], 'unknown')
        self.verify.assert_not_called()

    def test_partial_unknown_and_nonsynthetic_responses_fail_without_retry(self):
        for content in ({'effect': 'partial', 'route': 'synthetic_events'},
                        {'effect': 'unverifiable', 'route': 'synthetic_events', 'delivery': {'mode': 'unknown'}},
                        {'effect': 'confirmed', 'route': 'primary'}):
            with self.subTest(content=content):
                self.snapshot_mock.side_effect = [self.snapshot, self.snapshot]
                self.digest.side_effect = ['before', 'after']
                self.client.tool.reset_mock()
                self.client.tool.return_value = {'structuredContent': content}
                with self.assertRaises(AssertionError):
                    self.call()
                self.client.tool.assert_called_once()

    def test_freshness_is_anchored_to_observation_start_and_fixed_at_five_seconds(self):
        self.clock.return_value = self.snapshot['proof_observation_started_ns'] + 5_000_000_001
        with self.assertRaisesRegex(AssertionError, 'grounding expired'):
            self.call()
        self.client.tool.assert_not_called()

    def test_app_process_change_fails_before_dispatch(self):
        self.identity_mock.return_value = {'pid': 20, 'start_ticks': 301}
        with self.assertRaisesRegex(AssertionError, 'process identity changed'):
            self.call()
        self.client.tool.assert_not_called()

    def test_pid_reuse_after_snapshot_fails_before_dispatch(self):
        self.birth.return_value = 301
        with self.assertRaisesRegex(AssertionError, 'process identity changed'):
            self.call()
        self.client.tool.assert_not_called()

    def test_unchanged_grid_is_not_a_pass(self):
        self.digest.side_effect = ['same', 'same']
        with self.assertRaisesRegex(AssertionError, 'pixels did not change'):
            self.call()
        self.client.tool.assert_called_once()


class IdentityAndPlanTests(unittest.TestCase):
    def test_target_and_geometry_checks_use_real_snapshot_helper(self):
        spec = {**plan()['agents'][0], 'pointer_stage': 'click_b2'}
        for window, bounds in [({'pid': 20, 'window_id': 201}, BOUNDS),
                               ({'pid': 21, 'window_id': 200}, BOUNDS),
                               ({'pid': 20, 'window_id': 200}, {**BOUNDS, 'x': 1})]:
            value = client()
            value.tool.side_effect = [
                {'structuredContent': {'windows': [window]}},
                {'structuredContent': {'window_bounds': bounds, 'screenshot_width': 800, 'screenshot_height': 600}}]
            with self.subTest(window=window, bounds=bounds), self.assertRaises(AssertionError):
                proof.grounded_snapshot(value, spec['target'], spec)
            self.assertTrue(all(call.args[0] != 'click' for call in value.tool.call_args_list))

    def test_runtime_object_pid_and_liveness_are_fixed(self):
        original = client()
        expected = runtime(original)
        proof.require_runtime(original, expected)
        for replacement in (client(), client(101)):
            with self.assertRaisesRegex(AssertionError, 'process changed'):
                proof.require_runtime(replacement, expected)
        original.process.poll.return_value = 0
        with self.assertRaisesRegex(AssertionError, 'survive'):
            proof.require_runtime(original, expected)

    def test_plan_accepts_only_fixed_synthetic_calc_scenario(self):
        proof.validate_plan(plan())
        for key, value in [('app', 'inkscape'), ('document', '/private/work.ods'),
                           ('profile', {'mode': 'standard'}), ('name', '')]:
            modified = plan()
            modified['agents'][0][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                proof.validate_plan(modified)
        with self.assertRaises(AssertionError):
            proof.validate_plan({**plan(), 'idle_timeout': 1})


class RunnerTests(unittest.TestCase):
    def test_same_client_and_session_across_expiry_and_expiry_failure_blocks_second_action(self):
        for expires in (True, False):
            with self.subTest(expires=expires), tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                args = SimpleNamespace(plan=Mock(), evidence=Path(directory) / 'proof',
                                       driver=Path('/synthetic/cua-driver'),
                                       trace_socket=Path('/synthetic/cua-input-v3.sock'))
                import json
                args.plan.read_text.return_value = json.dumps(plan())
                first_client, observer = client(), client(100)
                stack.enter_context(patch.object(proof, 'DirectMCP', side_effect=[first_client, observer]))
                stack.enter_context(patch.object(proof, 'provenance', return_value={'files': {}}))
                stack.enter_context(patch.object(proof, 'calc_identity', return_value={'start_ticks': 300}))
                stack.enter_context(patch.object(proof, 'read_input_status', return_value=status(False, 0)))
                stack.enter_context(patch.object(proof, 'close_owned'))
                stack.enter_context(patch('sys.stdout', new_callable=io.StringIO))
                events = []
                def episode(*values, final=False):
                    events.append(values[6])
                    self.assertIs(values[2], first_client)
                    self.assertEqual(values[1]['agents'][0]['name'], 'idle-proof')
                    values[8].update(lane=1, action={'dispatch_ns': 0}, occupied_status=status(True, 2 if final else 1))
                episodes = stack.enter_context(patch.object(proof, 'episode', side_effect=episode))
                def expiry(*values):
                    events.append('expiry')
                    self.assertIs(values[0], first_client)
                    if not expires:
                        raise AssertionError('no expiry')
                    return {'status': status(False)}
                stack.enter_context(patch.object(proof, 'wait_for_idle_expiry', side_effect=expiry))
                self.assertEqual(proof.run(args), 0 if expires else 1)
                self.assertEqual(episodes.call_count, 2 if expires else 1)
                self.assertEqual(events, ['click_b2', 'expiry', 'click_a1'] if expires else ['click_b2', 'expiry'])
                first_client.tool.assert_called_once_with('start_session', {'session': 'idle-proof'})


if __name__ == '__main__':
    unittest.main()
