"""Collect and analyze the bounded, opt-in compositor trace.

Only counters and coordinates are recorded. A missing, truncated, overflowing,
or timed-out trace is inconclusive, never a pass. The parked/grab oracle rejects
even a cursor warp that returned before the next Driver snapshot.
"""
import argparse
import json
import math
from pathlib import Path
import socket


class Trace:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        self.socket.settimeout(5)
        self.socket.connect(str(path))
        assert self.exchange('HELLO')['ok']

    def exchange(self, packet):
        self.socket.sendall(packet.encode('ascii'))
        result = json.loads(self.socket.recv(65536))
        if result.get('ok') is not True:
            raise RuntimeError(result)
        return result

    def collect(self):
        status = self.exchange('TRACE_READ 0')
        rows = status.pop('events')
        while len(rows) < status['count']:
            page = self.exchange('TRACE_READ ' + str(len(rows)))
            if page['count'] != status['count'] or not page['events']:
                raise RuntimeError('trace changed while collecting')
            rows.extend(page['events'])
        return {**status, 'events': rows}

    def close(self):
        self.socket.close()


def analyze(trace, *, expected_motion=None):
    rows = trace.get('events', [])
    complete = (trace.get('hook') is True and trace.get('active') is False
                and trace.get('overflow') is False and trace.get('timed_out') is False
                and trace.get('count') == len(rows) and len(rows) >= 2)
    if complete:
        kinds = {'start', 'stop', 'cursor', 'pointer_focus', 'keyboard_focus', 'pointer_motion',
                 'pointer_enter', 'pointer_leave', 'pointer_button', 'pointer_axis',
                 'keyboard_enter', 'keyboard_leave', 'keyboard_key', 'agent_cancel',
                 'agent_approved', 'agent_action_end', 'agent_drag_start', 'agent_drag_end'}
        complete = all(isinstance(r, list) and len(r) == 7
                       and type(r[0]) is int and type(r[1]) is int and r[1] >= 0
                       and isinstance(r[2], str) and r[2] in kinds
                       and all(type(v) in (int, float) and math.isfinite(v) for v in r[3:5])
                       and type(r[5]) is int and r[5] in (0, 1, 2)
                       and type(r[6]) is int and r[6] in (0, 1) for r in rows)
    if complete:
        complete = (rows[0][2] == 'start' and rows[-1][2] == 'stop'
                    and [r[0] for r in rows] == list(range(1, len(rows) + 1))
                    and all(a[1] <= b[1] for a, b in zip(rows, rows[1:])))
    if not complete:
        return {'result': 'inconclusive', 'reason': 'incomplete_telemetry'}
    baseline = rows[0][3:5]
    motion = [r for r in rows if r[2] == 'cursor']
    max_displacement = max((math.dist(r[3:5], baseline) for r in rows), default=0)
    if expected_motion is None:
        unexpected_motion = [r for r in motion if math.dist(r[3:5], baseline) > 0.01]
        if max_displacement > 0.01 and not unexpected_motion:
            return {'result': 'inconclusive', 'reason': 'position_changed_without_motion_event'}
    else:
        # Ordered, externally issued primary-seat positions. No tolerance window
        # can hide a warp between two legitimate movements.
        actual = [r[3:5] for r in motion]
        unexpected_motion = [] if len(actual) == len(expected_motion) and all(
            math.dist(a, b) <= 0.01 for a, b in zip(actual, expected_motion)) else motion or [None]
    primary = [r for r in rows if r[5] == 0]
    focus = [r for r in primary if r[2] in ('pointer_focus', 'keyboard_focus',
              'pointer_enter', 'pointer_leave', 'keyboard_enter', 'keyboard_leave')]
    leaked = [r for r in primary if r[2] in ('keyboard_key', 'pointer_axis', 'pointer_button')]
    releases = [r for r in leaked if r[2] == 'pointer_button' and r[6] == 0]
    starts, intervals = {}, []
    for row in rows:
        if row[2] == 'agent_drag_start':
            starts[row[5]] = row[1]
        elif row[2] in ('agent_drag_end', 'agent_cancel') and row[5] in starts:
            intervals.append((row[5], starts.pop(row[5]), row[1]))
    overlap = max((min(a[2], b[2]) - max(a[1], b[1]) for a in intervals for b in intervals
                   if a[0] != b[0]), default=0)
    return {'result': 'passed' if not (unexpected_motion or focus or leaked) else 'failed',
            'telemetry_complete': True, 'events': len(rows),
            'primary_motion_events': len(motion), 'uncommanded_motion_events': len(unexpected_motion),
            'max_primary_displacement_px': max_displacement,
            'focus_events': len(focus), 'foreground_input_events': len(leaked),
            'unexpected_button_releases': len(releases),
            'agent_drag_overlap_ms': max(overlap, 0) / 1_000_000,
            'agent_action_completions': sum(r[2] in ('agent_action_end', 'agent_drag_end') for r in rows)}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('trace', type=Path)
    args = parser.parse_args()
    print(json.dumps(analyze(json.loads(args.trace.read_text())), indent=2))
