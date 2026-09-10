"""Independent parked-primary client evidence, never compositor attribution."""
import hashlib
import json
import math
import os
from pathlib import Path
import re
import socket
import stat
import struct
import subprocess
import time
import uuid

MAX_BYTES = 32 * 1024 * 1024
MAX_RECORDS = 100000
MAX_GAP_NS = 1_000_000_000
MAX_INTERVAL_NS = 60_000_000_000
WIRE = re.compile(r'^\[\s*(\d+\.\d+|(?:[01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]\.[0-9]+)\]'
                  r'\s*(?:\{[^}]+\}\s*)?(?P<out>->\s*)?'
                  r'(?P<interface>\w+)[#@](?P<object>\d+)\.(?P<event>\w+)\((?P<arguments>.*)\)$')


def wire_rows(data):
    assert len(data) <= MAX_BYTES and (not data or data.endswith(b'\n')), 'incomplete or oversized wire log'
    rows = []
    for line in data.decode('utf-8').splitlines():
        match = WIRE.fullmatch(line)
        assert match, 'unparseable Wayland wire record'
        rows.append({**match.groupdict(), 'object': int(match['object']), 'out': match['out'] is not None})
    assert len(rows) <= MAX_RECORDS, 'wire event limit exceeded'
    return rows


def sync_barrier(data):
    pending, completed = set(), 0
    for row in wire_rows(data):
        if row['out'] and row['interface'] == 'wl_display' and row['event'] == 'sync':
            match = re.fullmatch(r'new id wl_callback[#@](\d+)', row['arguments'])
            assert match, 'unidentified Wayland sync callback'
            callback = int(match[1])
            assert callback not in pending, 'overlapping callback identity'
            pending.add(callback)
        elif not row['out'] and row['interface'] == 'wl_callback' and row['event'] == 'done':
            if row['object'] in pending:
                pending.remove(row['object'])
                completed += 1
    assert completed == 2 and not pending, 'missing complete independent Wayland sync barriers'


def journal_rows(data):
    assert data and len(data) <= MAX_BYTES and data.endswith(b'\n'), 'incomplete or oversized journal'
    rows = [json.loads(line) for line in data.splitlines()]
    assert len(rows) <= MAX_RECORDS, 'journal record limit exceeded'
    for index, row in enumerate(rows):
        assert type(row['seq']) is int and row['seq'] == index + 1, 'journal sequence gap'
        assert type(row['time']) is int and row['time'] > 0
        assert row['instance'] == rows[0]['instance'], 'journal producer changed'
        if index:
            assert row['time'] >= rows[index - 1]['time'], 'journal clock regressed'
    assert rows[0]['kind'] == 'ready' and rows[0]['native_wayland'] is True, 'missing native observer startup'
    return rows


def pointer_position(row):
    values = row['arguments'].split(',')
    assert len(values) == 3, 'malformed primary pointer motion'
    point = [float(value) for value in values[1:]]
    assert all(math.isfinite(value) for value in point), 'nonfinite primary position'
    return point


def primary_wire_state(data):
    pointers, keyboards = {}, {}
    for row in wire_rows(data):
        if row['out']:
            continue
        interface, event, obj = row['interface'], row['event'], row['object']
        values = [value.strip() for value in row['arguments'].split(',')]
        if interface == 'wl_pointer':
            state = pointers.setdefault(obj, {'surface': None, 'buttons': set(), 'position': None})
            if event == 'enter':
                assert len(values) == 4 and re.fullmatch(r'wl_surface[#@]\d+', values[1])
                state.update(surface=int(re.split('[#@]', values[1])[1]),
                             position=[float(value) for value in values[2:]])
            elif event == 'leave':
                state['surface'] = None
            elif event == 'motion':
                state['position'] = pointer_position(row)
            elif event == 'button':
                assert len(values) == 4 and values[3] in ('0', '1'), 'malformed primary button event'
                if values[3] == '1':
                    state['buttons'].add(int(values[2]))
                else:
                    state['buttons'].discard(int(values[2]))
        elif interface == 'wl_keyboard':
            state = keyboards.setdefault(obj, {'surface': None, 'keys': set(), 'modifiers': None})
            if event == 'enter':
                assert len(values) == 3 and re.fullmatch(r'wl_surface[#@]\d+', values[1])
                assert values[2] == 'array[0]', 'primary keyboard entered with held keys'
                state['surface'] = int(re.split('[#@]', values[1])[1])
            elif event == 'leave':
                state['surface'] = None
            elif event == 'key':
                assert len(values) == 4 and values[3] in ('0', '1')
                if values[3] == '1':
                    state['keys'].add(int(values[2]))
                else:
                    state['keys'].discard(int(values[2]))
            elif event == 'modifiers':
                assert len(values) == 5
                state['modifiers'] = [int(value) for value in values[1:]]
    pointers = [(obj, row) for obj, row in pointers.items() if row['surface'] is not None]
    keyboards = [(obj, row) for obj, row in keyboards.items() if row['surface'] is not None]
    assert len(pointers) == len(keyboards) == 1, 'need one active primary pointer and keyboard'
    pointer_id, pointer = pointers[0]
    keyboard_id, keyboard = keyboards[0]
    assert pointer['surface'] == keyboard['surface'], 'primary pointer/keyboard focus differs'
    assert pointer['buttons'] == {272}, 'primary wire does not prove the held left-button grab'
    assert not keyboard['keys'] and keyboard['modifiers'] == [0, 0, 0, 0], 'primary keyboard is not idle'
    assert pointer['position'] is not None and all(math.isfinite(value) for value in pointer['position'])
    return {'pointer': pointer_id, 'keyboard': keyboard_id, 'surface': pointer['surface'],
            'position': pointer['position'], 'held_button': 272}


def analyze(before, after, journal, wire, intervals, primary_before, primary_after):
    """An incomplete observation raises; a complete observation can fail isolation."""
    assert before['identity'] == after['identity'], 'observer identity changed'
    start, end = before['marker'], after['marker']
    assert start['kind'] == end['kind'] == 'sync' and start['nonce'] != end['nonce']
    assert start['time'] < end['time'] and end['time'] - start['time'] <= MAX_INTERVAL_NS
    assert journal[start['seq'] - 1] == start and journal[end['seq'] - 1] == end, 'sync marker journal mismatch'
    assert end['seq'] == len(journal), 'end marker is not the final retained journal row'
    assert 0 <= start['wire_start'] < start['wire_end'] <= end['wire_start'] < end['wire_end'] == len(wire)
    sync_barrier(wire[start['wire_start']:start['wire_end']])
    sync_barrier(wire[end['wire_start']:end['wire_end']])
    assert start['held'] is True and start['buttons'] == [1] and not start['keys_down']
    assert start['window_active'] is True and start['canvas_focus'] is True, 'foreground fixture is not focused'
    assert intervals and all(start['time'] <= begin < finish <= end['time'] for begin, finish in intervals), \
        'observer does not cover the complete action/control interval'
    rows = journal[start['seq']:]
    assert all(row['kind'] != 'sync' for row in rows[:-1]), 'unexpected observer synchronization inside interval'
    heartbeats = [start['time']] + [row['time'] for row in rows if row['kind'] == 'state'] + [end['time']]
    assert all(0 <= right - left <= MAX_GAP_NS for left, right in zip(heartbeats, heartbeats[1:])), \
        'observer heartbeat gap or stalled event loop'
    primary = primary_wire_state(wire[:start['wire_end']])
    violations = []
    motions = []
    if primary_before != primary_after:
        violations.append({'kind': 'primary_endpoints'})
    for row in rows:
        if row['kind'] in ('state', 'sync'):
            for key in ('clicks', 'keys', 'scroll', 'held', 'buttons', 'keys_down', 'window_active', 'canvas_focus', 'motion'):
                if row[key] != start[key]:
                    violations.append({'kind': 'journal_state', 'field': key, 'seq': row['seq']})
        else:
            violations.append({'kind': 'journal_event', 'event': row['kind'], 'seq': row['seq']})
    events = wire_rows(wire[start['wire_end']:end['wire_end']])
    for row in events:
        if row['out']:
            continue
        interface, event = row['interface'], row['event']
        if interface == 'wl_pointer' and event == 'motion':
            motions.append({'object': row['object'], 'position': pointer_position(row)})
        forbidden = ((interface == 'wl_pointer' and event != 'frame') or interface == 'wl_keyboard'
                     or interface == 'zwp_relative_pointer_v1' or interface == 'wl_touch'
                     or interface == 'wl_seat' or (interface == 'wl_display' and event == 'error'))
        if forbidden:
            violations.append({'kind': 'wire_event', 'interface': interface, 'event': event, 'object': row['object']})
    return {'result': 'failed' if violations else 'passed',
            'scope': 'independent-parked-primary-client', 'compositor_attribution': False,
            'primary': primary, 'start_ns': start['time'], 'end_ns': end['time'],
            'action_intervals': intervals, 'journal_records': len(rows), 'wire_records': len(events),
            'complete': True, 'violations': violations, 'motions': motions}


def verify_negative_control(result):
    assert result['result'] == 'failed' and result['complete'] is True, 'control did not fail the normal detector'
    baseline = result['primary']['position']
    motions = result['motions']
    assert len(motions) >= 2 and all(row['object'] == result['primary']['pointer'] for row in motions)
    assert any(row['position'] != baseline for row in motions) and motions[-1]['position'] == baseline, \
        'wire control does not prove an excursion and return'
    for row in result['violations']:
        assert ((row['kind'] == 'wire_event' and row['interface'] == 'wl_pointer' and row['event'] == 'motion')
                or (row['kind'] == 'journal_event' and row['event'] == 'motion-notify')
                or (row['kind'] == 'journal_state' and row['field'] == 'motion')), 'control failed for an unrelated reason'
    return {'verified': True, 'detector_result': 'failed', 'wire_motion_events': len(motions)}


class PrimaryObserver:
    def __init__(self, control, foreground, journal, evidence):
        self.control = control.resolve(strict=True)
        self.foreground = foreground
        self.journal = journal.resolve(strict=True)
        self.evidence = evidence
        self.source = Path(__file__).with_name('primary_observer_fixture.py').resolve(strict=True)
        self.before = None
        self.prefixes = {}
        self.descriptors = {}
        self.process_start = self._process_identity()

    def _process_identity(self):
        pid = self.foreground['pid']
        process = Path('/proc') / str(pid)
        argv = (process / 'cmdline').read_bytes().split(b'\0')
        assert str(self.source).encode() in argv and str(self.journal).encode() in argv, 'foreground is not the reviewed observer fixture'
        clients = json.loads(subprocess.check_output(['hyprctl', '-j', 'clients'], text=True, timeout=5))
        matches = [row for row in clients if row.get('pid') == pid]
        assert len(matches) == 1 and matches[0].get('xwayland') is False
        assert int(matches[0]['address'], 16) == self.foreground['window_id'], 'observer native target changed'
        return (process / 'stat').read_text().rsplit(')', 1)[1].split()[19]

    def _sync(self):
        assert self._process_identity() == self.process_start, 'observer process changed'
        info = self.control.lstat()
        assert stat.S_ISSOCK(info.st_mode) and info.st_uid == os.getuid(), 'invalid observer control socket'
        nonce = uuid.uuid4().hex
        requested = time.monotonic_ns()
        with socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET) as client:
            client.settimeout(2)
            client.connect(str(self.control))
            pid, uid, _ = struct.unpack('3i', client.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            assert pid == self.foreground['pid'] and uid == os.getuid(), 'observer socket peer mismatch'
            client.sendall(json.dumps({'command': 'SYNC', 'nonce': nonce}).encode())
            data, _, flags, _ = client.recvmsg(16384)
            assert data and not flags & socket.MSG_TRUNC, 'incomplete observer acknowledgement'
        received = time.monotonic_ns()
        packet = json.loads(data)
        marker, identity = packet['marker'], packet['identity']
        assert re.fullmatch(r'[0-9a-f]{32}', identity['instance']) and marker['instance'] == identity['instance'], \
            'observer instance identity mismatch'
        assert marker['nonce'] == nonce and requested <= marker['time'] <= received, 'stale observer synchronization'
        assert received - requested <= 2_000_000_000, 'observer synchronization exceeded deadline'
        assert identity['pid'] == pid and identity['uid'] == uid and identity['native_wayland'] is True
        assert identity['source_sha256'] == hashlib.sha256(self.source.read_bytes()).hexdigest(), 'observer source identity mismatch'
        packet['controller_interval'] = [requested, received]
        return packet

    def _read(self, name, packet, size):
        metadata = packet['identity'][name]
        path = Path(metadata['path'])
        assert path.resolve(strict=True) == path, 'noncanonical observer log'
        if name == 'journal':
            assert path == self.journal, 'foreground journal identity mismatch'
        if name not in self.descriptors:
            self.descriptors[name] = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        descriptor = self.descriptors[name]
        current, opened = path.lstat(), os.fstat(descriptor)
        assert stat.S_ISREG(current.st_mode) and current.st_uid == os.getuid()
        assert (current.st_dev, current.st_ino) == (opened.st_dev, opened.st_ino) == (metadata['device'], metadata['inode']), \
            'observer log replaced'
        assert type(size) is int and 0 < size <= current.st_size <= MAX_BYTES, 'observer log truncated or overflowed'
        data = os.pread(descriptor, size, 0)
        assert len(data) == size, 'short observer log read'
        if name in self.prefixes:
            assert data.startswith(self.prefixes[name]), 'observer history changed'
        return data

    def start(self, primary):
        self.before = self._sync()
        self._save('primary-observer-begin.json', self.before)
        self.primary = primary
        self.prefixes['journal'] = self._read('journal', self.before, self.before['journal_end'])
        self.prefixes['wire'] = self._read('wire', self.before, self.before['marker']['wire_end'])
        (self.evidence / 'primary-observer-begin-journal.jsonl').write_bytes(self.prefixes['journal'])
        (self.evidence / 'primary-observer-begin-wire.log').write_bytes(self.prefixes['wire'])
        rows = journal_rows(self.prefixes['journal'])
        assert all(rows[0][key] == value for key, value in self.before['identity'].items()), 'observer startup identity mismatch'
        assert rows[-1] == self.before['marker'], 'baseline marker is not the retained journal tail'
        marker = self.before['marker']
        sync_barrier(self.prefixes['wire'][marker['wire_start']:marker['wire_end']])
        primary_wire_state(self.prefixes['wire'])
        assert marker['held'] is True and marker['buttons'] == [1] and not marker['keys_down']
        assert marker['window_active'] is True and marker['canvas_focus'] is True

    def _save(self, name, value):
        (self.evidence / name).write_text(json.dumps(value, indent=2) + '\n')

    def finish(self, intervals, primary):
        after = self._sync()
        self._save('primary-observer-end.json', after)
        journal = self._read('journal', after, after['journal_end'])
        wire = self._read('wire', after, after['marker']['wire_end'])
        (self.evidence / 'primary-observer-journal.jsonl').write_bytes(journal)
        (self.evidence / 'primary-observer-wire.log').write_bytes(wire)
        result = analyze(self.before, after, journal_rows(journal), wire, intervals, self.primary, primary)
        self._save('primary-observer-isolation.json', result)
        return result

    def close(self):
        for descriptor in self.descriptors.values():
            os.close(descriptor)
        self.descriptors.clear()
