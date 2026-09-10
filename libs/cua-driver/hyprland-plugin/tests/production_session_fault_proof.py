"""TEST ONLY: production v3 DPMS fault proof in one explicitly selected guest.

Use the geometry proof's CLI flags. Plan: purpose=session_fault,
disposable=true, fault={kind:dpms}, vm={machine_id,boot_id}, compositor with
pid,instance,uid,starttime,exe, foreground, primary_point, package_versions,
one Calc pointer agent and recovery={pointer_stage:click_b2|click_a1} as in
production_geometry_fault_proof. identities={foreground,target} contains exact
/proc identities (pid,uid,starttime,exe). foreground_fixture={sha256,journal}
binds the repository isolated-input/main.py hash and its journal's canonical
path,device,inode,uid. monitors is the exact list returned by monitor_identity.
The foreground process must run that source fixture with --actor Foreground.

Every mutation is test-owned IPC; normal Driver alone supplies app input.
The 20-second independent watchdog wakes only this disposable test desktop.
Watchdog restoration is emergency cleanup, never successful proof. Success
requires cancellation and own-seat release before explicit test restoration,
a fresh Driver action refused with session_unavailable and zero dispatch while
off, then a new runtime, fresh screenshot and successful new action after on.
DPMS is expected to change only monitor power and desktop generations: no
primary cursor, focus, held-button or input exceptions are suppressed.

Lock is deliberately unsupported: its real primary focus/grab transitions
need a separately qualified oracle. A lock request fails preflight, never
becomes a skipped/passing row. Portable tests are not native certification.
"""
import argparse
from production_app_smoke import add_provenance_arguments
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import select
import socket
import stat
import struct
import subprocess
import sys
import time

from desktop_faults import _identity, _same_compositor, _hypr, _dpms_dispatch
from driver_input_live import state, wait_for, wm
from primary_trace import Trace, analyze
from production_cancel_proof import (MAX_GROUNDING_AGE_NS, PROFILE, active_drags, call_drag,
    close_owned, grounded_snapshot, poll_active, prepare_drag, stopped_prefix, verify_recovery_cleanup)
from production_desktop_fault_proof import (guest_identity, idle_lanes, verify_status,
    pointer_cleanup, validate_min_motion, drag_motion_px, poll_fault_active, verify_retained_inert,
    verify_refusal_claim)
from production_geometry_fault_proof import recover, fault_outcome, validate_plan as geometry_plan
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
import production_pointer_grounding as pointer_grounding
from production_realapp_proof import (PRIMARY_LIFETIME_MS, app_process_identity, check_response,
    primary_acknowledgement, provenance, require_primary_active, trace_interval)
from realapp_proof import cleanup_all, released_synthetic_input


WATCHDOG_SECONDS = 20
PROCESS_KEYS = {'pid', 'uid', 'starttime', 'exe'}
MONITOR_KEYS = ('id', 'name', 'width', 'height', 'x', 'y', 'scale', 'transform')


def validate_plan(plan):
    assert plan['purpose'] == 'session_fault' and plan['fault']['kind'] == 'dpms', \
        'only DPMS is qualified; session-lock primary transition oracle is unsupported'
    validate_fault_options(plan['fault'])
    bounds = plan['agents'][0]['bounds']
    geometry_plan({**plan, 'purpose': 'geometry_fault',
                   'compositor': {key: plan['compositor'][key] for key in ('pid', 'instance')},
                   'fault': {'kind': 'move', 'to': [bounds['x'] + 1, bounds['y']]}})
    assert plan['agents'][0]['app'] == ('inkscape' if plan.get('app_profile') == 'inkscape-only' else 'calc'), \
        'app requires its explicit qualification profile'
    assert set(plan['vm']) == {'machine_id', 'boot_id'}
    assert re.fullmatch(r'[0-9a-f]{32}', plan['vm']['machine_id'])
    assert re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', plan['vm']['boot_id'])
    assert set(plan['compositor']) == PROCESS_KEYS | {'instance'}
    assert set(plan['identities']) == {'foreground', 'target'}
    for identity in [*plan['identities'].values(),
                     {key: plan['compositor'][key] for key in PROCESS_KEYS}]:
        assert set(identity) == PROCESS_KEYS
        assert type(identity['pid']) is int and identity['pid'] > 1
        assert type(identity['uid']) is int and identity['uid'] >= 0
        assert re.fullmatch(r'[0-9]+', identity['starttime'])
        assert Path(identity['exe']).is_absolute()
    assert Path(plan['compositor']['exe']).name == 'Hyprland'
    assert plan['identities']['target']['pid'] == plan['agents'][0]['target']['pid']
    assert plan['identities']['foreground']['pid'] == plan['foreground']['pid']
    fixture = plan['foreground_fixture']
    assert set(fixture) == {'sha256', 'journal'} and re.fullmatch(r'[0-9a-f]{64}', fixture['sha256'])
    journal = fixture['journal']
    assert set(journal) == {'path', 'device', 'inode', 'uid'} and Path(journal['path']).is_absolute()
    assert all(type(journal[key]) is int and journal[key] >= 0 for key in ('device', 'inode', 'uid'))
    assert plan['monitors'] == monitor_identity([{**row, 'dpmsStatus': True} for row in plan['monitors']])


def monitor_identity(rows):
    """Bind all outputs; never wake a newly attached, unreviewed output."""
    assert rows and len({r['name'] for r in rows}) == len(rows), 'missing or duplicate monitors'
    result = []
    for row in rows:
        assert type(row.get('dpmsStatus')) is bool, 'compositor does not expose DPMS acknowledgement'
        assert isinstance(row.get('name'), str) and row['name']
        assert all(type(row.get(k)) is int for k in ('id', 'width', 'height', 'x', 'y', 'transform'))
        assert row['width'] > 0 and row['height'] > 0 and row['id'] >= 0
        assert type(row.get('scale')) in (int, float) and 0 < row['scale'] <= 8
        result.append({key: row[key] for key in MONITOR_KEYS})
    return sorted(result, key=lambda row: row['name'])


def guard_guest(config):
    assert platform.system() == 'Linux' and guest_identity() == config['vm'], 'wrong disposable VM or boot'
    assert config['disposable'] is True
    assert subprocess.run(['systemd-detect-virt', '--vm', '--quiet'], timeout=2).returncode == 0, 'VM required'
    assert os.environ.get('HYPRLAND_INSTANCE_SIGNATURE') == config['instance'], 'wrong compositor session'
    assert config['compositor']['uid'] == os.getuid()
    assert Path(config['compositor']['exe']).name == 'Hyprland'
    _same_compositor(config['compositor'], config['instance'])


def power(config, expected=None):
    rows = json.loads(_hypr(config['instance'], '-j', 'monitors', 'all'))
    assert monitor_identity(rows) == config['monitors'], 'reviewed monitor identity or geometry changed'
    if expected is not None:
        assert all(row['dpmsStatus'] is expected for row in rows), 'DPMS state not acknowledged'
    return rows


def production_status(config):
    return verify_status(json.loads(_hypr(config['instance'], '-j', 'cua:status')), True)


def lanes(status, cleared=False, *, allow_passive=False):
    verify_status(status, True)
    result = {row['lane']: row for row in status['input']['lanes']}
    for row in result.values():
        assert isinstance(row.get('epoch'), str) and row['epoch']
        assert all(type(row.get(key)) is int and row[key] >= 0 for key in ('desktop_generation', 'dispatches'))
        if cleared:
            assert all(type(row.get(key)) is int and row[key] == 0 for key in ('held_button', 'held_keys'))
            assert all(row.get(key) is False for key in ('drag_active', 'lease_active', 'keyboard_focus'))
            assert type(row.get('pointer_focus')) is bool
            if not allow_passive:
                assert row['pointer_focus'] is False
    return result


def validate_fault_options(fault, *, motion=True):
    allowed = {'kind', 'pointer_cleanup'} | ({'min_motion_px'} if motion else set())
    assert {'kind'} <= set(fault) <= allowed, 'unsupported fault option'
    pointer_cleanup(fault)
    if 'min_motion_px' in fault:
        validate_min_motion(fault['min_motion_px'])


def transition(before, after, policy='cleared', lane=None):
    pointer_cleanup({'pointer_cleanup': policy})
    old, new = lanes(before), lanes(after, cleared=True, allow_passive=policy == 'retained_inert')
    if policy == 'retained_inert':
        verify_retained_inert(before, after, lane)
    for lane in old:
        assert old[lane]['epoch'] == new[lane]['epoch'], 'compositor lane replaced'
        assert new[lane]['desktop_generation'] > old[lane]['desktop_generation'], 'DPMS did not revoke authority'
        assert new[lane].get('reserved') is False, 'reservation survived DPMS transition'


def verify_held_gate(record):
    """Recheck the opt-in trace/status/trace gate from saved evidence."""
    first, page, lane = record['gate_first'], record['prefix'], record['lane']
    trace_interval(first, page)
    assert active_drags(first) == active_drags(page) and set(active_drags(page)) == {lane}, 'held lane changed across status'
    synthetic = [row for row in page['events'] if row[5] in (1, 2)]
    assert all(row[5] == lane for row in synthetic), 'held gate crossed lanes'
    press = next(row for row in synthetic if row[2] == 'pointer_button')
    assert not any(row[2] == 'pointer_leave' or (row[2] == 'pointer_enter' and row[0] > press[0])
                   for row in synthetic), 'held pointer left or retargeted'
    started, requested = record['status_started_ns'], record['requested_ns']
    assert first['events'][-1][1] <= started <= requested
    assert 0 <= requested - started <= 250_000_000, 'stale held-input status'
    assert 0 <= requested - page['events'][-1][1] <= 250_000_000, 'stale held-input trace'
    rows = lanes(record['gate_status'])
    for key, row in rows.items():
        assert type(row['held_keys']) is int and row['held_keys'] == 0
        if key == lane - 1:
            assert type(row['held_button']) is int and row['held_button'] == 272
            assert all(row.get(k) is True for k in ('drag_active', 'lease_active', 'pointer_focus', 'reserved'))
        else:
            assert type(row['held_button']) is int and row['held_button'] == 0
            assert all(row.get(k) is False for k in ('drag_active', 'lease_active', 'keyboard_focus', 'reserved'))
    if 'min_motion_px' in record:
        validate_min_motion(record['min_motion_px'])
        assert all(drag_motion_px(p, lane) >= record['min_motion_px'] for p in (first, page)), 'insufficient held motion'


def verify_inert_interval(before, after):
    assert not any(row[5] in (1, 2) for row in trace_interval(before, after)), 'synthetic activity while pointer must remain inert'


def verify_stable_inert(before, after, lane):
    old = idle_lanes(before)
    new = verify_retained_inert(before, after, lane)
    assert all(old[key][field] == row[field] for key, row in new.items()
               for field in ('epoch', 'desktop_generation')), 'desktop changed during inert interval'


@contextmanager
def control_lock(config):
    # One private lock serializes watchdog and controller, including the off IPC.
    with open(config['lock_path'], 'rb') as stream:
        info = os.fstat(stream.fileno())
        assert [info.st_dev, info.st_ino, info.st_uid] == config['lock_identity']
        fcntl.flock(stream, fcntl.LOCK_EX)
        yield


def restore_power(config, emergency=False):
    with control_lock(config):
        guard_guest(config)
        record = {'started_ns': time.monotonic_ns(), 'emergency': emergency, 'result': 'unproven'}
        before = power(config)
        # A marker prevents an expired watchdog from being followed by new off IPC.
        Path(config['restored_path']).touch(exist_ok=True)
        assert _hypr(config['instance'], 'dispatch', _dpms_dispatch('on')) == 'ok', 'DPMS restoration refused'
        def restored():
            rows = power(config)
            return rows if all(row['dpmsStatus'] is True for row in rows) else None
        record.update(before=before, after=wait_for(restored, timeout=2), status=production_status(config),
                      observed_ns=time.monotonic_ns(), result='restored')
        return record


def watchdog(config, reader):
    """Independent child restores at deadline or controller EOF, never app input."""
    print('ARMED', flush=True)
    delay = max(0, (config['deadline_ns'] - time.monotonic_ns()) / 1_000_000_000)
    ready = select.select([reader], [], [], delay)[0]
    if ready and os.read(reader, 1) == b'C':
        return
    try:
        record = restore_power(config, emergency=True)
    except Exception as error:
        record = {'result': 'failed', 'error': str(error), 'emergency': True}
    Path(config['watchdog_path']).write_text(json.dumps(record))


class SessionFault:
    def __init__(self, plan, args):
        self.plan, self.args = plan, args
        self.config = {'disposable': plan['disposable'], 'vm': plan['vm'],
                       'compositor': {key: plan['compositor'][key] for key in PROCESS_KEYS},
                       'instance': plan['compositor']['instance'], 'monitors': plan['monitors']}
        self.child = self.cancel_fd = None
        self.mutated = False
        self.record = {'result': 'unproven', 'kind': 'dpms'}
        for key in ('pointer_cleanup', 'min_motion_px'):
            if key in plan['fault']:
                self.config[key] = self.record[key] = plan['fault'][key]
        self.check_targets()
        power(self.config, True)
        self.record['before'] = production_status(self.config)
        (args.evidence / 'pre-fault-status.json').write_text(json.dumps(self.record['before']))
        idle_lanes(self.record['before'])

    def check_targets(self):
        guard_guest(self.config)
        spec = self.plan['agents'][0]
        for identity in self.plan['identities'].values():
            assert identity['uid'] == os.getuid() and _identity(identity['pid']) == identity, 'process identity changed'
        app_process_identity(spec['app'], spec['target']['pid'])
        fixture = self.args.source / 'libs/cua-driver/tests/fixtures/apps/linux/isolated-input/main.py'
        assert fixture.resolve(strict=True) == fixture, 'canonical source fixture required'
        assert hashlib.sha256(fixture.read_bytes()).hexdigest() == self.plan['foreground_fixture']['sha256']
        argv = [x.decode() for x in Path(f'/proc/{self.plan["foreground"]["pid"]}/cmdline').read_bytes().split(b'\0') if x]
        assert len(argv) == 6 and Path(argv[1]).resolve(strict=True) == fixture, 'wrong foreground fixture command'
        assert argv[2:4] == ['--actor', 'Foreground'] and argv[4] == '--journal'
        journal = self.plan['foreground_fixture']['journal']
        path = self.args.foreground_journal
        assert str(path) == journal['path'] == argv[5] and path.resolve(strict=True) == path
        info = path.lstat()
        assert stat.S_ISREG(info.st_mode) and [info.st_dev, info.st_ino, info.st_uid] == [journal[k] for k in ('device', 'inode', 'uid')]
        windows = json.loads(_hypr(self.config['instance'], '-j', 'clients'))
        for target in (spec['target'], self.plan['foreground']):
            selected = [w for w in windows if w.get('pid') == target['pid']]
            assert len(selected) == 1 and selected[0].get('xwayland') is False
            assert int(selected[0]['address'], 16) == target['window_id'], 'target window changed'
            if target == spec['target']:
                assert f'cua-smoke-{spec["app"]}' in selected[0].get('title', ''), 'wrong synthetic document'
                assert dict(zip(('x', 'y', 'width', 'height'), [*selected[0]['at'], *selected[0]['size']])) == spec['bounds']

    def arm(self):
        assert self.child is None, 'watchdog already armed'
        root = self.args.evidence.resolve(strict=True)
        lock = root / 'dpms-control.lock'
        with lock.open('xb'):
            pass
        info = lock.stat()
        self.config.update(lock_path=str(lock), lock_identity=[info.st_dev, info.st_ino, info.st_uid],
                           restored_path=str(root / 'dpms-restored'), watchdog_path=str(root / 'watchdog.json'),
                           deadline_ns=time.monotonic_ns() + WATCHDOG_SECONDS * 1_000_000_000)
        reader, self.cancel_fd = os.pipe()
        try:
            self.child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--watchdog',
                json.dumps(self.config), str(reader)], pass_fds=(reader,), stdout=subprocess.PIPE,
                text=True, start_new_session=True)
            assert select.select([self.child.stdout], [], [], 2)[0] and self.child.stdout.readline() == 'ARMED\n', 'watchdog did not arm'
        finally:
            os.close(reader)

    def live_deadline(self):
        assert self.child and self.child.poll() is None, 'watchdog exited early'
        assert time.monotonic_ns() + 1_000_000_000 < self.config['deadline_ns'], 'watchdog deadline too near'
        assert not Path(self.config['restored_path']).exists(), 'watchdog already restored'

    def inject(self, trace, initial, pending, guard):
        with control_lock(self.config):
            self.check_targets()
            self.record['monitors_before'] = power(self.config, True)
            gated = 'min_motion_px' in self.config or pointer_cleanup(self.config) == 'retained_inert'
            if gated:
                gate_deadline = time.monotonic() + 3
                first, _ = poll_fault_active(trace, initial, pending, self.config.get('min_motion_px'))
                self.record.update(gate_first=first, status_started_ns=time.monotonic_ns())
                self.record['gate_status'] = production_status(self.config)
                remaining = min(0.25, gate_deadline - time.monotonic())
                assert remaining > 0, 'held gate exceeded bounded wait'
                page, active = poll_fault_active(trace, first, pending, self.config.get('min_motion_px'), timeout=remaining)
                assert time.monotonic() <= gate_deadline, 'held gate exceeded bounded wait'
            else:
                self.record['gate_status'] = production_status(self.config)
                page, active = poll_active(trace, initial, None, [pending])
            lane = next(iter(active))
            assert set(active) == {lane}
            guard()
            self.live_deadline()
            assert not pending.done(), 'drag finished before fault'
            requested = time.monotonic_ns()
            assert 0 <= requested - page['events'][-1][1] <= 250_000_000, 'stale active drag gate'
            self.record.update(prefix=page, lane=lane, requested_ns=requested,
                               watchdog_deadline_ns=self.config['deadline_ns'])
            if gated:
                verify_held_gate(self.record)
                for key, row in lanes(self.record['before']).items():
                    assert all(row[field] == lanes(self.record['gate_status'])[key][field]
                               for field in ('epoch', 'desktop_generation', 'dispatches')), 'desktop changed before DPMS'
            self.mutated = True  # Lost IPC reply may still mean power changed.
            assert _hypr(self.config['instance'], 'dispatch', _dpms_dispatch('off')) == 'ok', 'DPMS-off dispatcher unsupported'
            self.record['acknowledged_ns'] = time.monotonic_ns()
            def off():
                rows = power(self.config)
                return rows if all(row['dpmsStatus'] is False for row in rows) else None
            self.record['monitors_off'] = wait_for(off, timeout=2)
            self.record.update(after=production_status(self.config), observed_ns=time.monotonic_ns(), result='observed')
            transition(self.record['gate_status'], self.record['after'], pointer_cleanup(self.config), lane)
            guard()
        return lane

    def unavailable(self):
        self.check_targets()
        self.live_deadline()
        return power(self.config, False)

    def restore(self):
        if not self.mutated:
            return {'result': 'not_needed'}
        result = restore_power(self.config)
        self.mutated = False
        # Explicit restoration has ended the fault. Reap the watchdog before
        # slow app observation/recovery can reach its former wake deadline.
        self.close()
        return result

    def close(self):
        # Cancel only after observed restoration. Otherwise EOF triggers recovery.
        if self.cancel_fd is not None:
            if not self.mutated:
                try:
                    os.write(self.cancel_fd, b'C')
                except BrokenPipeError:
                    pass
            os.close(self.cancel_fd)
            self.cancel_fd = None
        if self.child:
            self.child.wait(timeout=10)
            assert self.child.returncode == 0, 'watchdog failed'
            self.child.stdout.close()
            self.child = None


def verify_cancelled(boundary, record, action):
    assert record['result'] == 'observed'
    policy = pointer_cleanup(record)
    if policy == 'retained_inert' or 'min_motion_px' in record:
        verify_held_gate(record)
    prefix, lane = record['prefix'], record['lane']
    assert set(active_drags(prefix)) == {lane}
    assert prefix['events'][-1][1] <= record['requested_ns'] <= record['acknowledged_ns'] <= record['observed_ns']
    assert record['requested_ns'] - prefix['events'][-1][1] <= 250_000_000
    assert record['observed_ns'] < record['watchdog_deadline_ns'], 'fault outlived watchdog deadline'
    assert monitor_identity(record['monitors_before']) == monitor_identity(record['monitors_off'])
    assert all(row['dpmsStatus'] is True for row in record['monitors_before'])
    assert all(row['dpmsStatus'] is False for row in record['monitors_off']), 'DPMS-off not observed'
    tail = [row for row in trace_interval(prefix, boundary) if row[5] in (1, 2)]
    assert all(row[5] == lane for row in tail), 'cleanup crossed lanes'
    cancelled = [row for row in tail if row[2] == 'agent_cancel']
    assert len(cancelled) == 1 and cancelled[0][1] >= record['requested_ns'], 'missing post-fault cancellation'
    assert all(row[2] in ('agent_cancel', 'pointer_button', 'pointer_leave', 'keyboard_leave',
                          'pointer_motion', 'pointer_enter') for row in tail), 'unexpected action after fault'
    assert not any(row[2] in ('pointer_motion', 'pointer_enter') and row[0] > cancelled[0][0]
                   for row in tail), 'input continued after cancellation'
    if policy == 'retained_inert':
        assert not any(row[2] in ('pointer_enter', 'pointer_leave') for row in tail), 'retained pointer left or retargeted'
    releases = [row for row in tail if row[2] == 'pointer_button']
    assert len(releases) == 1 and releases[0][6] == 0 and releases[0][0] > cancelled[0][0], 'missing own-seat release'
    if policy == 'retained_inert':
        assert releases[0][1] <= record['observed_ns'], 'release was not observed before inert status'
    isolation = analyze(stopped_prefix(boundary))
    assert isolation['result'] == 'passed' and released_synthetic_input(stopped_prefix(boundary)), isolation
    transition(record['gate_status'], record['after'], policy, lane)
    return {'result': 'verified', 'outcome': fault_outcome(action), 'continuous_isolation': isolation}


def verify_refusal(record):
    assert record['outcome'] == 'response' and record['replayed'] is False
    check_response(record['response'], {'kind': 'refused', 'reason': 'session_unavailable'})
    assert not [row for row in trace_interval(record['trace_before'], record['trace_after']) if row[5] in (1, 2)], 'refused action emitted synthetic events'
    policy = pointer_cleanup(record)
    before = lanes(record['before'], cleared=True, allow_passive=policy == 'retained_inert')
    after = lanes(record['after'], cleared=True, allow_passive=policy == 'retained_inert')
    if policy == 'retained_inert':
        # A fresh CLAIM reserves capacity even when TARGET is refused. It is
        # not the cancelled actor's lease and must disappear when this probe closes.
        claim = verify_refusal_claim(record['before'], record['after'], record['response'],
                                     interrupted_lane=record['lane'])
    for lane in before:
        assert all(before[lane][key] == after[lane][key] for key in ('epoch', 'desktop_generation', 'dispatches')), 'refused action dispatched or desktop changed'
    assert record['prepared_ns'] <= record['dispatch_ns'] <= record['observed_ns'] < record['deadline_ns']
    assert record['dispatch_ns'] - record['prepared_ns'] <= MAX_GROUNDING_AGE_NS
    assert all(row['dpmsStatus'] is False for row in record['monitors_before'] + record['monitors_after'])
    assert monitor_identity(record['monitors_before']) == monitor_identity(record['monitors_after'])
    assert analyze(stopped_prefix(record['trace_after']))['result'] == 'passed'
    return {'result': 'verified', 'reason': 'session_unavailable', 'no_dispatch': 'verified',
            **({'claim': claim} if policy == 'retained_inert' else {})}


def verify_refusal_close(record):
    assert pointer_cleanup(record) == 'retained_inert'
    assert type(record['exit_code']) is int, 'refusal runtime was not reaped'
    assert record['observed_ns'] <= record['close_started_ns'] <= record['reaped_ns'] <= record['closed_ns'] < record['deadline_ns']
    verify_stable_inert(record['before'], record['after_close'], record['lane'])
    verify_inert_interval(record['trace_after'], record['trace_after_close'])
    assert all(row['dpmsStatus'] is False for row in record['monitors_after_close']), 'DPMS ended before probe close'
    assert monitor_identity(record['monitors_after']) == monitor_identity(record['monitors_after_close'])
    assert analyze(stopped_prefix(record['trace_after_close']))['result'] == 'passed'
    return {'result': 'verified', 'reservation_released': True, 'no_dispatch': 'verified'}


def prepare_refusal(prepared, spec, stage):
    """Derive the refused pixel probe from the same pre-fault target image.

    Both actions address this exact window, and neither uses a session-bound
    element token. A second simultaneous AT-SPI walk of the same app adds
    contention, not newer evidence. The refusal remains bounded by the first
    observation's original timestamp; powered-off capture is unavailable.
    """
    assert stage in ({'click_a1', 'click_b2'} if spec['app'] == 'calc' else {'scroll_down', 'scroll_up'})
    snapshot = prepared['snapshot']
    assert prepared['target'] == spec['target']
    assert {key: snapshot[key] for key in ('pid', 'window_id')} == spec['target']
    assert snapshot['window_bounds'] == spec['bounds']
    assert prepared['prepared_ns'] == snapshot['proof_observation_started_ns']
    arguments, _ = pointer_grounding.action(
        snapshot, pointer_grounding.read_pixels(snapshot['proof_image']), spec['app'], stage)
    return {'snapshot': snapshot, 'arguments': arguments, 'session': spec['name'] + '-unavailable',
            'prepared_ns': prepared['prepared_ns'], 'tool': pointer_grounding.STAGES[spec['app']][stage]}


def prepare_actions(clients, spec, stage, save):
    """Ground two distinct runtimes' same-window pixel actions without input."""
    assert len(clients) == 2
    assert_distinct_runtimes(clients)
    prepared = prepare_drag(clients[0], spec)
    probe = prepare_refusal(prepared, spec, stage)
    save('drag-grounding.json', prepared)
    save('refusal-grounding.json', probe)
    return prepared, probe


def refuse(client, spec, prepared, fault, trace, guard, save):
    record = {**prepared, 'outcome': 'unknown', 'replayed': False, 'runtime_pid': client.process.pid,
              'pointer_cleanup': pointer_cleanup(fault.config),
              'deadline_ns': fault.config['deadline_ns'], 'before': production_status(fault.config),
              'monitors_before': fault.unavailable(), 'trace_before': trace.collect()}
    try:
        if pointer_cleanup(record) == 'retained_inert':
            record['lane'] = fault.record['lane']
            verify_stable_inert(fault.record['after'], record['before'], record['lane'])
        guard()
        fault.live_deadline()
        record['dispatch_ns'] = time.monotonic_ns()
        assert 0 <= record['dispatch_ns'] - record['prepared_ns'] <= MAX_GROUNDING_AGE_NS, 'refusal grounding expired'
        record['response'] = client.tool(prepared.get('tool', 'click'), {**prepared['arguments'], **spec['target'],
            'session': prepared['session'], 'delivery_mode': 'background'})
        record['outcome'] = 'response'
        record.update(after=production_status(fault.config), monitors_after=fault.unavailable(),
                      trace_after=trace.collect(), observed_ns=time.monotonic_ns())
        record['verification'] = verify_refusal(record)
        if pointer_cleanup(record) == 'retained_inert':
            record['close_started_ns'] = time.monotonic_ns()
            close_owned(client)
            record.update(exit_code=client.process.poll(), reaped_ns=time.monotonic_ns())
            assert type(record['exit_code']) is int, 'refusal runtime was not reaped'
            record.update(after_close=production_status(fault.config), monitors_after_close=fault.unavailable(),
                          trace_after_close=trace.collect(), closed_ns=time.monotonic_ns())
            record['close_verification'] = verify_refusal_close(record)
        return record
    finally:
        save('unavailable-action.json', record)


def connect_trace(path, config):
    expected = Path(os.environ['XDG_RUNTIME_DIR']) / 'hypr' / config['instance'] / path.name
    assert path.name in ('cua-input-v3.sock', 'cua-input-v3-2.sock')
    assert path.is_absolute() and path == expected and path.resolve(strict=True) == path
    info = path.lstat()
    assert stat.S_ISSOCK(info.st_mode) and info.st_uid == config['compositor']['uid']
    trace = Trace(path)
    try:
        peer = struct.unpack('3i', trace.socket.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
        assert peer[:2] == (config['compositor']['pid'], config['compositor']['uid'])
        assert trace.hello['protocol'] == 3
        return trace
    except BaseException:
        trace.close()
        raise


def preserve_interrupted_state(observer, spec, action, restoration, guard, save):
    """Observe the partial app state after explicit wake, before recovery input."""
    assert restoration['result'] == 'restored' and restoration['emergency'] is False
    assert action['replayed'] is False
    guard()
    snapshot = grounded_snapshot(observer, spec['target'], spec, session=False)
    observed_ns = time.monotonic_ns()
    assert restoration['observed_ns'] <= snapshot['proof_observation_started_ns'] <= observed_ns
    record = {'snapshot': snapshot, 'action': action, 'replayed': False,
              'restoration_observed_ns': restoration['observed_ns'], 'observed_ns': observed_ns}
    save('interrupted-state.json', record)
    guard()
    return record


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2))
    report = {'result': 'failed', 'scope': 'native-session-dpms-fault', 'recovery': {'result': 'unproven'},
              'lock': 'unsupported_primary_transition_oracle', 'full_desktop_matrix': False, 'physical_hardware': False}
    clients, observer, grab, trace, fault, pool, future = [], None, None, None, None, None, None
    baseline = primary_before = deadline = prefix = None
    def guard():
        require_primary_active(grab, deadline)
    def launch(name):
        directory = args.evidence / name
        directory.mkdir()
        return DirectMCP(args.driver, directory, PROFILE)
    try:
        plan = json.loads(args.plan.read_text())
        save('plan.json', plan)
        validate_plan(plan)
        fault = SessionFault(plan, args)
        origin = provenance(args, plan)
        for name in ('production_session_fault_proof.py', 'production_session_fault_proof_test.py',
                     'production_desktop_fault_proof.py', 'production_geometry_fault_proof.py',
                     'production_cancel_proof.py', 'desktop_faults.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        save('provenance.json', origin)
        spec = plan['agents'][0]
        clients.append(launch('agent'))
        clients.append(launch('refusal'))
        observer = launch('observer')
        report['runtime_pids'] = assert_distinct_runtimes([*clients, observer])
        for client, name in ((clients[0], spec['name']), (clients[1], spec['name'] + '-unavailable')):
            assert not client.tool('start_session', {'session': name}).get('isError')
        foreground = grounded_snapshot(observer, plan['foreground'])['window_bounds']
        desktop = observer.tool('get_desktop_state', {})
        assert not desktop.get('isError')
        desktop = desktop['structuredContent']
        x, y = plan['primary_point']
        assert 0 < x < foreground['width'] and 0 < y < foreground['height']
        x, y = foreground['x'] + x, foreground['y'] + y
        assert 0 <= x < desktop['screen_width'] and 0 <= y < desktop['screen_height']
        deadline = time.monotonic_ns() + PRIMARY_LIFETIME_MS * 1_000_000
        grab = subprocess.Popen([str(args.primary_grab), str(x), str(y), str(desktop['screen_width']),
            str(desktop['screen_height']), str(PRIMARY_LIFETIME_MS)], stdout=subprocess.PIPE, text=True)
        assert primary_acknowledgement(grab.stdout) == 'HELD\n'
        wait_for(lambda: state(args.foreground_journal)['held'], timeout=3)
        primary_before, baseline = wm(), state(args.foreground_journal)
        assert primary_before['pid'] == plan['foreground']['pid'] and baseline['held']
        trace = connect_trace(args.trace_socket, fault.config)
        start_ns = time.monotonic_ns()
        trace.exchange('TRACE_START')
        initial = trace.collect()
        assert start_ns <= initial['events'][0][1] <= time.monotonic_ns()
        assert not any(row[5] in (1, 2) for row in initial['events'])
        prepared, probe = prepare_actions(clients, spec, plan['recovery']['pointer_stage'], save)
        fault.arm()
        guard()
        pool = ThreadPoolExecutor(max_workers=1)
        future = pool.submit(call_drag, clients[0], spec, prepared, guard)
        lane = fault.inject(trace, initial, future, guard)
        report['action'] = future.result(timeout=3)
        save('drag-action.json', report['action'])
        boundary = trace.collect()
        save('fault-prefix.json', boundary)
        report['fault'] = verify_cancelled(boundary, fault.record, report['action'])
        if pointer_cleanup(fault.config) == 'retained_inert':
            close_owned(clients[0])
            assert clients[0].process.poll() is not None, 'interrupted runtime was not reaped'
        report['refusal'] = refuse(clients[1], spec, probe, fault, trace, guard, save)
        if pointer_cleanup(fault.config) == 'retained_inert':
            verify_inert_interval(boundary, report['refusal']['trace_before'])
        fault.unavailable()
        restoration = fault.restore()
        save('restoration.json', restoration)
        assert boundary['events'][-1][1] < restoration['started_ns']
        assert report['refusal']['observed_ns'] <= restoration['started_ns']
        assert restoration['observed_ns'] < fault.config['deadline_ns'] and not Path(fault.config['watchdog_path']).exists(), 'watchdog recovery is not proof'
        if pointer_cleanup(fault.config) == 'retained_inert':
            assert report['refusal']['closed_ns'] <= restoration['started_ns']
            transition(report['refusal']['after_close'], restoration['status'], 'retained_inert', lane)
        else:
            transition(report['refusal']['after'], restoration['status'])
        for client in clients:
            close_owned(client)
        boundary = trace.collect()
        save('pre-recovery-prefix.json', boundary)
        report['teardown'] = verify_recovery_cleanup(report['refusal']['trace_after'], stopped_prefix(boundary))
        preserve_interrupted_state(observer, spec, report['action'], restoration, guard, save)
        if pointer_cleanup(fault.config) == 'retained_inert':
            # Include runtime teardown and read-only app observation, before any recovery input.
            before_recovery = production_status(fault.config)
            verify_stable_inert(restoration['status'], before_recovery, lane)
            boundary = trace.collect()
            verify_inert_interval(report['refusal']['trace_after'], boundary)
            save('pre-recovery-retained-status.json', before_recovery)
            save('pre-recovery-prefix.json', boundary)
        clients.append(launch('recovery'))
        assert clients[-1].process.pid not in report['runtime_pids'], 'reused prior runtime'
        prefix = recover(clients[-1], observer, clients[0], spec, plan['recovery']['pointer_stage'],
                         trace, boundary, lane, guard, save, report['recovery'])
        save('recovery-prefix.json', prefix)
        fault.check_targets()
        power(fault.config, True)
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        operations = []
        # Restore first on failure; a blocking runtime close must not delay wake.
        if fault:
            operations += [('preserve_fault', lambda: save('fault.json', fault.record)),
                           ('restore_dpms', lambda: save('cleanup-restoration.json', fault.restore())),
                           ('close_watchdog', fault.close)]
        operations += [(f'close_agent_{i}', lambda c=c: close_owned(c)) for i, c in enumerate(clients)]
        if future:
            def preserve_action():
                report['action'] = future.result(timeout=3)
                save('drag-action.json', report['action'])
            operations.append(('preserve_action', preserve_action))
        if pool:
            operations.append(('shutdown_pool', lambda: pool.shutdown(wait=False, cancel_futures=True)))
        if trace:
            def finish_trace():
                trace.exchange('TRACE_STOP')
                stopped = trace.collect()
                save('trace.json', stopped)
                guard()
                isolation = verify_recovery_cleanup(prefix, stopped) if prefix else analyze(stopped)
                assert isolation['result'] == 'passed' and released_synthetic_input(stopped), isolation
                report['continuous_isolation'] = isolation
                assert wm() == primary_before, 'primary cursor/focus/workspace changed'
                current = state(args.foreground_journal)
                assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held')), current
                guard()
            operations += [('finish_trace', finish_trace), ('close_trace', trace.close)]
        def release_primary():
            if grab:
                if grab.poll() is None:
                    grab.terminate()
                stop_process(grab)
                wait_for(lambda: not state(args.foreground_journal)['held'], timeout=3)
        operations.append(('release_primary', release_primary))
        if observer:
            operations.append(('close_observer', lambda: close_owned(observer)))
        errors = cleanup_all(operations)
        save('cleanup.json', {'errors': errors})
        if errors:
            report['result'] = 'failed'
        save('result.json', report)
    print(json.dumps(report), flush=True)
    return 0 if report['result'] == 'passed' else 1


if __name__ == '__main__':
    if not __debug__:
        raise SystemExit('assertions must be enabled')
    if len(sys.argv) == 4 and sys.argv[1] == '--watchdog':
        watchdog(json.loads(sys.argv[2]), int(sys.argv[3]))
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        for name in ('driver', 'plugin', 'source', 'primary-grab', 'plan', 'evidence', 'foreground-journal', 'trace-socket'):
            parser.add_argument('--' + name, required=True, type=Path)
        add_provenance_arguments(parser)
        raise SystemExit(run(parser.parse_args()))
