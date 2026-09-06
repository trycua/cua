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
from production_desktop_fault_proof import guest_identity, verify_status
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
    assert plan['purpose'] == 'session_fault' and plan['fault'] == {'kind': 'dpms'}, \
        'only DPMS is qualified; session-lock primary transition oracle is unsupported'
    bounds = plan['agents'][0]['bounds']
    geometry_plan({**plan, 'purpose': 'geometry_fault',
                   'compositor': {key: plan['compositor'][key] for key in ('pid', 'instance')},
                   'fault': {'kind': 'move', 'to': [bounds['x'] + 1, bounds['y']]}})
    assert plan['agents'][0]['app'] == 'calc', 'only the qualified synthetic Calc episode is supported'
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


def lanes(status, cleared=False):
    verify_status(status, True)
    result = {row['lane']: row for row in status['input']['lanes']}
    for row in result.values():
        assert isinstance(row.get('epoch'), str) and row['epoch']
        assert all(type(row.get(key)) is int and row[key] >= 0 for key in ('desktop_generation', 'dispatches'))
        if cleared:
            assert all(type(row.get(key)) is int and row[key] == 0 for key in ('held_button', 'held_keys'))
            assert all(row.get(key) is False for key in ('drag_active', 'lease_active', 'pointer_focus', 'keyboard_focus'))
    return result


def transition(before, after):
    old, new = lanes(before), lanes(after, cleared=True)
    for lane in old:
        assert old[lane]['epoch'] == new[lane]['epoch'], 'compositor lane replaced'
        assert new[lane]['desktop_generation'] > old[lane]['desktop_generation'], 'DPMS did not revoke authority'
        assert new[lane].get('reserved') is False, 'reservation survived DPMS transition'


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
        self.check_targets()
        power(self.config, True)
        self.record['before'] = production_status(self.config)
        lanes(self.record['before'], cleared=True)

    def check_targets(self):
        guard_guest(self.config)
        spec = self.plan['agents'][0]
        for identity in self.plan['identities'].values():
            assert identity['uid'] == os.getuid() and _identity(identity['pid']) == identity, 'process identity changed'
        app_process_identity('calc', spec['target']['pid'])
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
                assert 'cua-smoke-calc' in selected[0].get('title', ''), 'wrong synthetic Calc document'
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
            self.mutated = True  # Lost IPC reply may still mean power changed.
            assert _hypr(self.config['instance'], 'dispatch', _dpms_dispatch('off')) == 'ok', 'DPMS-off dispatcher unsupported'
            self.record['acknowledged_ns'] = time.monotonic_ns()
            def off():
                rows = power(self.config)
                return rows if all(row['dpmsStatus'] is False for row in rows) else None
            self.record['monitors_off'] = wait_for(off, timeout=2)
            self.record.update(after=production_status(self.config), observed_ns=time.monotonic_ns(), result='observed')
            transition(self.record['gate_status'], self.record['after'])
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


def verify_cancelled(boundary, record, action):
    assert record['result'] == 'observed'
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
    releases = [row for row in tail if row[2] == 'pointer_button']
    assert len(releases) == 1 and releases[0][6] == 0 and releases[0][0] > cancelled[0][0], 'missing own-seat release'
    isolation = analyze(stopped_prefix(boundary))
    assert isolation['result'] == 'passed' and released_synthetic_input(stopped_prefix(boundary)), isolation
    transition(record['gate_status'], record['after'])
    return {'result': 'verified', 'outcome': fault_outcome(action), 'continuous_isolation': isolation}


def verify_refusal(record):
    assert record['outcome'] == 'response' and record['replayed'] is False
    check_response(record['response'], {'kind': 'refused', 'reason': 'session_unavailable'})
    assert not [row for row in trace_interval(record['trace_before'], record['trace_after']) if row[5] in (1, 2)], 'refused action emitted synthetic events'
    before, after = lanes(record['before'], cleared=True), lanes(record['after'], cleared=True)
    for lane in before:
        assert all(before[lane][key] == after[lane][key] for key in ('epoch', 'desktop_generation', 'dispatches')), 'refused action dispatched or desktop changed'
    assert record['prepared_ns'] <= record['dispatch_ns'] <= record['observed_ns'] < record['deadline_ns']
    assert record['dispatch_ns'] - record['prepared_ns'] <= MAX_GROUNDING_AGE_NS
    assert all(row['dpmsStatus'] is False for row in record['monitors_before'] + record['monitors_after'])
    assert monitor_identity(record['monitors_before']) == monitor_identity(record['monitors_after'])
    assert analyze(stopped_prefix(record['trace_after']))['result'] == 'passed'
    return {'result': 'verified', 'reason': 'session_unavailable', 'no_dispatch': 'verified'}


def prepare_refusal(client, spec, stage):
    """Observe the exact target while powered, before the fault blocks capture."""
    fresh = {**spec, 'name': spec['name'] + '-unavailable', 'pointer_stage': stage}
    started_ns = time.monotonic_ns()
    snapshot = grounded_snapshot(client, spec['target'], fresh)
    arguments, _ = pointer_grounding.action(
        snapshot, pointer_grounding.read_pixels(snapshot['proof_image']), 'calc', stage)
    return {'snapshot': snapshot, 'arguments': arguments, 'session': fresh['name'],
            'prepared_ns': snapshot.get('proof_observation_started_ns', started_ns)}


def prepare_actions(clients, spec, stage, save):
    """Read through distinct runtimes in parallel; neither call supplies input.

    Sequential snapshots aged the first image past the five-second limit.
    Keep both original observation timestamps and the unchanged dispatch gates.
    """
    assert len(clients) == 2
    assert_distinct_runtimes(clients)
    with ThreadPoolExecutor(max_workers=2) as observations:
        drag = observations.submit(prepare_drag, clients[0], spec)
        refusal = observations.submit(prepare_refusal, clients[1], spec, stage)
        prepared, probe = drag.result(), refusal.result()
    save('drag-grounding.json', prepared)
    save('refusal-grounding.json', probe)
    return prepared, probe


def refuse(client, spec, prepared, fault, trace, guard, save):
    record = {**prepared, 'outcome': 'unknown', 'replayed': False, 'runtime_pid': client.process.pid,
              'deadline_ns': fault.config['deadline_ns'], 'before': production_status(fault.config),
              'monitors_before': fault.unavailable(), 'trace_before': trace.collect()}
    try:
        guard()
        fault.live_deadline()
        record['dispatch_ns'] = time.monotonic_ns()
        assert 0 <= record['dispatch_ns'] - record['prepared_ns'] <= MAX_GROUNDING_AGE_NS, 'refusal grounding expired'
        record['response'] = client.tool('click', {**prepared['arguments'], **spec['target'],
            'session': prepared['session'], 'delivery_mode': 'background'})
        record['outcome'] = 'response'
        record.update(after=production_status(fault.config), monitors_after=fault.unavailable(),
                      trace_after=trace.collect(), observed_ns=time.monotonic_ns())
        record['verification'] = verify_refusal(record)
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
        report['refusal'] = refuse(clients[1], spec, probe, fault, trace, guard, save)
        fault.unavailable()
        restoration = fault.restore()
        save('restoration.json', restoration)
        assert boundary['events'][-1][1] < restoration['started_ns']
        assert report['refusal']['observed_ns'] <= restoration['started_ns']
        assert restoration['observed_ns'] < fault.config['deadline_ns'] and not Path(fault.config['watchdog_path']).exists(), 'watchdog recovery is not proof'
        transition(report['refusal']['after'], restoration['status'])
        for client in clients:
            close_owned(client)
        boundary = trace.collect()
        save('pre-recovery-prefix.json', boundary)
        report['teardown'] = verify_recovery_cleanup(report['refusal']['trace_after'], stopped_prefix(boundary))
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
        parser.add_argument('--source-sha', required=True)
        raise SystemExit(run(parser.parse_args()))
