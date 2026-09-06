import unittest
from unittest.mock import Mock, patch
from primary_trace import Trace, analyze


def trace(*events):
    rows = [[i + 1, i * 1_000_000, kind, x, y, actor, state]
            for i, (kind, x, y, actor, state) in enumerate(events)]
    return dict(hook=True, active=False, overflow=False, timed_out=False,
                count=len(rows), events=rows)


START = ('start', 100, 200, 0, 0)
STOP = ('stop', 100, 200, 0, 0)


class TraceTest(unittest.TestCase):
    def test_handshake_is_sent_once_and_retained_for_version_validation(self):
        peer = Mock()
        peer.recv.return_value = b'{"ok":true,"protocol":3}'
        with patch('primary_trace.socket.socket', return_value=peer):
            connection = Trace('/synthetic/cua-input-v3.sock')
            self.assertEqual(connection.hello['protocol'], 3)
            peer.sendall.assert_called_once_with(b'HELLO')
            connection.close()

    def test_warp_and_return_is_not_hidden_by_equal_endpoints(self):
        result = analyze(trace(START, ('cursor', 140, 230, 0, 0), ('cursor', 100, 200, 0, 0), STOP))
        self.assertEqual(result['result'], 'failed')
        self.assertEqual(result['max_primary_displacement_px'], 50)
        self.assertEqual(result['uncommanded_motion_events'], 1)

    def test_synthetic_events_do_not_count_as_foreground_leakage(self):
        result = analyze(trace(START, ('pointer_button', 100, 200, 1, 0),
                               ('keyboard_key', 100, 200, 2, 1), STOP))
        self.assertEqual(result['result'], 'passed')

    def test_v3_admission_marker_is_recognized_but_unknown_events_stay_inconclusive(self):
        self.assertEqual(analyze(trace(START, ('agent_admitted', 100, 200, 1, 0), STOP))['result'], 'passed')
        self.assertEqual(analyze(trace(START, ('unknown', 100, 200, 1, 0), STOP))['result'], 'inconclusive')

    def test_missing_or_dropped_telemetry_is_inconclusive(self):
        for change in ({'hook': False}, {'overflow': True}, {'timed_out': True},
                       {'active': True}, {'count': 9}, {'events': []}):
            self.assertEqual(analyze({**trace(START, STOP), **change})['result'], 'inconclusive')
        broken = trace(START, STOP)
        broken['events'][1][0] = 3
        self.assertEqual(analyze(broken)['result'], 'inconclusive')
        for row in ([], [1], [1, 1, 'cursor', 'bad', 0, 0, 0],
                    [1, 1, 'cursor', float('nan'), 0, 0, 0]):
            bad = trace(START, STOP)
            bad['events'][0] = row
            self.assertEqual(analyze(bad)['result'], 'inconclusive')

    def test_primary_focus_keys_and_releases_fail(self):
        for kind in ('pointer_focus', 'keyboard_enter', 'keyboard_key', 'pointer_button'):
            self.assertEqual(analyze(trace(START, (kind, 100, 200, 0, 0), STOP))['result'], 'failed')

    def test_real_overlap_is_measured_from_dispatch_not_animation(self):
        result = analyze(trace(START, ('agent_drag_start', 100, 200, 1, 0),
                               ('agent_drag_start', 100, 200, 2, 0),
                               ('agent_drag_end', 100, 200, 1, 0),
                               ('agent_drag_end', 100, 200, 2, 0), STOP))
        self.assertEqual(result['agent_drag_overlap_ms'], 1)

    def test_controlled_motion_must_match_every_command_in_order(self):
        data = trace(START, ('cursor', 120, 200, 0, 0), ('cursor', 100, 200, 0, 0), STOP)
        self.assertEqual(analyze(data, expected_motion=[[120, 200], [100, 200]])['result'], 'passed')
        self.assertEqual(analyze(data, expected_motion=[[100, 200]])['result'], 'failed')

    def test_controlled_motion_reconciles_all_observed_positions(self):
        moved = ('cursor', 120, 200, 0, 0)
        stopped = ('stop', 120, 200, 0, 0)
        for events in ((START, moved, STOP),
                       (START, moved, ('pointer_motion', 140, 200, 0, 0), stopped),
                       (START, moved, ('agent_action_end', 140, 200, 1, 0), stopped)):
            with self.subTest(events=events):
                result = analyze(trace(*events), expected_motion=[[120, 200]])
                self.assertEqual(result['result'], 'inconclusive')
                self.assertEqual(result['reason'], 'position_changed_without_motion_event')
        self.assertEqual(analyze(trace(START, moved, ('pointer_motion', 120, 200, 0, 0), stopped),
                                 expected_motion=[[120, 200]])['result'], 'passed')


if __name__ == '__main__':
    unittest.main()
