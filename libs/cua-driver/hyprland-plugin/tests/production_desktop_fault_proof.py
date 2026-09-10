"""Production v3 config/keymap fault proof for an exact disposable VM.

Use the geometry proof CLI flags. The plan has purpose=desktop_fault, one
Calc/Inkscape pointer agent and new-action recovery as in the geometry proof.
fault={kind:config_disable|keymap}; vm={machine_id,boot_id}; compositor includes
pid,instance,uid,starttime,exe; config includes absolute path,device,inode,uid,
mode,sha256. Config-disable requires the already sourced, exact ENABLED Lua
include from input_config_toggle. Main configuration and policy are never edited.
Optional fault.min_motion_px requires that much Euclidean surface-local motion
from the pre-press pointer position on the same live lane before fault injection.
Omitting it retains the held-drag gate without a minimum motion requirement.
Optional fault.pointer_cleanup=retained_inert certifies continuous inert hover
on the interrupted lane; omitted or cleared retains the original cleanup proof.
The retained proof checks presence, trace continuity, and grounded app identity;
the protocol does not expose a Wayland pointer surface identity.

Config suspension disconnects the trace transport. Restore the fixture, reconnect
without TRACE_START, and require unchanged trace history with cancellation and
owned release BEFORE restoration. Normal Driver snapshots and one NEW action
then prove recovery. No replay, signing, module swap, automatic wake or unlock.
Keymap uses the separate exact KEYMAP_US include, changes only to KEYMAP_DE,
and restores the same original US map. Exact option readback, compositor lane
generation changes and the compiled-map-gated unsupported_layout refusal from
a fresh normal Driver action prove invalidation; no keymap hash is exposed.
The keymap fixture has a fixed 30-second restoration watchdog to cover the
fresh-runtime refusal and both app snapshots; config-disable keeps 12 seconds.
Watchdog restoration, stale grounding, or readback crossing either deadline
still fails the episode. Neither duration is a product cancellation limit.
DPMS and lock are deliberately unsupported here. Portable tests prepare
this proof; only execution on the exact native candidate can certify a row.
"""
import argparse
from production_app_smoke import add_provenance_arguments
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import hashlib
import json
import math
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
import tempfile
import time

from desktop_faults import _identity, _same_compositor, _hypr
from input_config_toggle import ENABLED, DISABLED
from driver_input_live import state, wait_for, wm
from primary_trace import Trace, analyze
from production_cancel_proof import (MAX_GROUNDING_AGE_NS, PROFILE, active_drags, call_drag, close_owned,
    grounded_snapshot, poll_active, prepare_drag, stopped_prefix, verify_recovery_cleanup)
from production_geometry_fault_proof import recover, fault_outcome, validate_plan as geometry_plan
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
from production_realapp_proof import (PRIMARY_LIFETIME_MS, app_process_identity, check_response, primary_acknowledgement,
    provenance, require_primary_active, trace_interval)
from realapp_proof import cleanup_all, released_synthetic_input
import production_pointer_grounding as pointer_grounding


KEYMAP_US = 'hl.config({input = {kb_rules = "evdev", kb_model = "pc105", kb_layout = "us", kb_variant = "", kb_options = "", kb_file = ""}})\n'
KEYMAP_DE = 'hl.config({input = {kb_rules = "evdev", kb_model = "pc105", kb_layout = "de", kb_variant = "", kb_options = "", kb_file = ""}})\n'
WATCHDOG_SECONDS = {'config_disable': 12, 'keymap': 30}


def fixed_bytes(kind):
    assert kind in ('config_disable', 'keymap'), 'unsupported desktop fault'
    return (ENABLED, DISABLED) if kind == 'config_disable' else (KEYMAP_US, KEYMAP_DE)


def keymap_options(instance, restored):
    expected = {'kb_rules': 'evdev', 'kb_model': 'pc105', 'kb_layout': 'us' if restored else 'de',
                'kb_variant': '', 'kb_options': '', 'kb_file': ''}
    replies = {key: json.loads(_hypr(instance, '-j', 'getoption', 'input:' + key)) for key in expected}
    verify_keymap_options(replies, restored)
    return replies


def verify_keymap_options(replies, restored):
    expected = {'kb_rules': 'evdev', 'kb_model': 'pc105', 'kb_layout': 'us' if restored else 'de',
                'kb_variant': '', 'kb_options': '', 'kb_file': ''}
    assert set(replies) == set(expected), 'incomplete keymap option readback'
    for key, value in expected.items():
        assert replies[key].get('option') == 'input:' + key and replies[key].get('str') == value, \
            'exact sourced keymap option not observed'


def pointer_cleanup(record):
    policy = record.get('pointer_cleanup', 'cleared')
    assert policy in ('cleared', 'retained_inert'), 'unknown pointer cleanup policy'
    return policy


def keymap_lanes(status, *, cleared=False, enabled=True):
    verify_status(status, enabled)
    lanes = {row['lane']: row for row in status['input']['lanes']}
    for row in lanes.values():
        assert isinstance(row.get('epoch'), str) and row['epoch'], 'missing compositor epoch'
        assert type(row.get('desktop_generation')) is int and row['desktop_generation'] >= 0
        assert type(row.get('dispatches')) is int and row['dispatches'] >= 0
        if cleared:
            assert all(type(row.get(key)) is int and row[key] == 0 for key in ('held_button', 'held_keys'))
            assert all(row.get(key) is False for key in ('drag_active', 'lease_active', 'pointer_focus', 'keyboard_focus'))
    return lanes


def verify_retained_inert(before, after, lane, *, enabled=True):
    """Presence continuity, not an invented surface/resource identity."""
    assert type(lane) is int and lane in (1, 2), 'missing interrupted lane'
    old = keymap_lanes(before, enabled=before['configured'])
    new = keymap_lanes(after, enabled=enabled)
    assert old[lane - 1].get('pointer_focus') is True, 'interrupted pointer was not present'
    for index, row in new.items():
        assert type(old[index].get('pointer_focus')) is bool
        assert row.get('pointer_focus') is old[index]['pointer_focus'], 'pointer presence changed'
        assert row['dispatches'] == old[index]['dispatches'], 'fault or refusal dispatched new input'
        assert all(type(row.get(key)) is int and row[key] == 0 for key in ('held_button', 'held_keys'))
        assert all(row.get(key) is False for key in
                   ('drag_active', 'lease_active', 'keyboard_focus', 'reserved')), 'retained hover owns authority'
    return new


def verify_keymap_transition(before, after, policy='cleared', lane=None):
    assert pointer_cleanup({'pointer_cleanup': policy}) == policy
    old, new = keymap_lanes(before), keymap_lanes(after, cleared=policy == 'cleared')
    if policy == 'retained_inert':
        verify_retained_inert(before, after, lane)
    for lane in old:
        assert old[lane]['epoch'] == new[lane]['epoch'], 'compositor lane replaced'
        assert new[lane]['desktop_generation'] > old[lane]['desktop_generation'], 'keymap did not invalidate authority'
        assert new[lane].get('reserved') is False, 'pre-transition reservation survived'


def idle_lanes(status):
    """Before a new episode, only unreserved inert pointer focus may remain.

    Runtime close retires input authority without removing passive hover.
    This is not the stronger post-keymap/DPMS/lock transition assertion.
    """
    rows = keymap_lanes(status)
    for row in rows.values():
        assert all(type(row.get(key)) is int and row[key] == 0
                   for key in ('held_button', 'held_keys')), 'pre-fault input still held'
        assert all(row.get(key) is False for key in
                   ('drag_active', 'lease_active', 'keyboard_focus', 'reserved')), 'pre-fault authority remains'
        assert type(row.get('pointer_focus')) is bool, 'missing pre-fault pointer state'
    return rows


def digest(data):
    return hashlib.sha256(data).hexdigest()


def guest_identity():
    return {'machine_id': Path('/etc/machine-id').read_text().strip(),
            'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip()}


def file_identity(path):
    """Reject links, aliases, shared files and non-private parent directories."""
    path = Path(path)
    assert path.is_absolute() and path.resolve(strict=True) == path, 'canonical config path required'
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        assert stat.S_ISREG(info.st_mode) and info.st_nlink == 1, 'regular unlinked config required'
        assert info.st_uid == os.getuid() and not info.st_mode & 0o022, 'config ownership or permissions'
        assert info.st_size <= 4096, 'bounded dedicated include required'
        data = os.read(fd, 4097)
        assert len(data) == info.st_size and len(data) <= 4096, 'config changed during read'
        fields = ('st_dev', 'st_ino', 'st_uid', 'st_gid', 'st_mode', 'st_size', 'st_mtime_ns', 'st_ctime_ns', 'st_nlink')
        assert all(getattr(observed, key) == getattr(info, key)
                   for observed in (path.lstat(), os.fstat(fd)) for key in fields), 'config changed during read'
        return {'device': info.st_dev, 'inode': info.st_ino, 'uid': info.st_uid,
                'mode': stat.S_IMODE(info.st_mode), 'sha256': digest(data)}
    finally:
        os.close(fd)


def validate_plan(plan):
    assert plan['purpose'] == 'desktop_fault' and {'kind'} <= set(plan['fault']) <= {'kind', 'min_motion_px', 'pointer_cleanup'}
    pointer_cleanup(plan['fault'])
    if 'min_motion_px' in plan['fault']:
        validate_min_motion(plan['fault']['min_motion_px'])
    original, _ = fixed_bytes(plan['fault']['kind'])
    # Reuse the strict existing target/grounding/recovery plan checks unchanged.
    bounds = plan['agents'][0]['bounds']
    geometry_plan({**plan, 'purpose': 'geometry_fault',
                   'compositor': {key: plan['compositor'][key] for key in ('pid', 'instance')},
                   'fault': {'kind': 'move', 'to': [bounds['x'] + 1, bounds['y']]}})
    assert set(plan['vm']) == {'machine_id', 'boot_id'}
    assert re.fullmatch(r'[0-9a-f]{32}', plan['vm']['machine_id'])
    assert re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', plan['vm']['boot_id'])
    compositor = plan['compositor']
    assert set(compositor) == {'pid', 'instance', 'uid', 'starttime', 'exe'}
    assert type(compositor['uid']) is int and compositor['uid'] >= 0
    assert re.fullmatch(r'[0-9]+', compositor['starttime'])
    assert Path(compositor['exe']).is_absolute() and Path(compositor['exe']).name == 'Hyprland'
    config = plan['config']
    assert set(config) == {'path', 'device', 'inode', 'uid', 'mode', 'sha256'}
    assert Path(config['path']).is_absolute()
    assert all(type(config[key]) is int and config[key] >= 0 for key in ('device', 'inode', 'uid', 'mode'))
    assert config['sha256'] == digest(original.encode()), 'only the exact dedicated include is permitted'


def validate_min_motion(value):
    assert type(value) in (int, float) and math.isfinite(value) and value > 0, 'positive finite min_motion_px required'


def drag_motion_px(page, lane):
    """Use protocol coordinates, never the primary cursor or another lane."""
    assert set(active_drags(page)) == {lane}, 'motion gate requires the same held lane'
    rows = [row for row in page['events'] if row[5] in (1, 2)]
    assert all(row[5] == lane for row in rows), 'motion gate crossed lanes'
    press = next(row for row in rows if row[2] == 'pointer_button')
    assert not any(row[2] == 'pointer_leave' or
                   (row[2] == 'pointer_enter' and row[0] > press[0]) for row in rows), 'motion gate pointer identity changed'
    positions = [row for row in rows if row[2] in ('pointer_enter', 'pointer_motion')]
    assert all(len(row) == 9 for row in positions), 'motion gate requires surface coordinates'
    before = [row for row in positions if row[0] < press[0]]
    assert before, 'motion gate lacks initial pointer position'
    motion = [row for row in positions if row[2] == 'pointer_motion' and row[0] > press[0]]
    return math.dist(before[-1][7:9], motion[-1][7:9]) if motion else 0


def poll_fault_active(trace, initial, pending, min_motion_px=None, timeout=3):
    if min_motion_px is None:
        return poll_active(trace, initial, None, [pending], timeout=timeout)
    validate_min_motion(min_motion_px)
    deadline = time.monotonic() + timeout
    previous, lanes = initial, None
    while (remaining := deadline - time.monotonic()) > 0:
        page, active = poll_active(trace, previous, lanes, [pending], timeout=remaining)
        lane = next(iter(active))
        displacement = drag_motion_px(page, lane)
        assert time.monotonic() <= deadline, 'motion gate exceeded bounded wait'
        if displacement >= min_motion_px:
            return page, active
        previous, lanes = page, set(active)
    raise AssertionError('insufficient drag motion within bounded wait')


def _guard(config):
    assert platform.system() == 'Linux' and guest_identity() == config['vm'], 'wrong disposable VM'
    assert subprocess.run(['systemd-detect-virt', '--vm', '--quiet'], timeout=2).returncode == 0, 'virtual machine required'
    assert os.environ.get('HYPRLAND_INSTANCE_SIGNATURE') == config['instance'], 'wrong session'
    assert config['compositor']['uid'] == os.getuid() and Path(config['compositor']['exe']).name == 'Hyprland'
    _same_compositor(config['compositor'], config['instance'])
    parent = Path(config['path']).parent
    info = parent.lstat()
    assert parent.resolve(strict=True) == parent and stat.S_ISDIR(info.st_mode)
    assert [info.st_dev, info.st_ino, info.st_uid, stat.S_IMODE(info.st_mode)] == config['directory'], 'config directory changed'


@contextmanager
def _locked(config):
    # Separate opens in parent/watchdog give separate flock descriptions.
    import fcntl
    _guard(config)
    fd = os.open(str(Path(config['path']).parent), os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        _guard(config)
        yield
    finally:
        os.close(fd)


def _replace(config, enabled, before_replace=None):
    """Atomic replacement from fixed, pre-created files with known identities."""
    _guard(config)
    current = file_identity(config['path'])
    known = config['files']
    original, changed = fixed_bytes(config.get('kind', 'config_disable'))
    assert set(known) == {'original', 'disabled', 'restored'}
    for name, data in (('original', original), ('disabled', changed), ('restored', original)):
        assert known[name]['identity']['sha256'] == digest(data.encode()), 'only fixed toggle bytes are permitted'
        assert Path(known[name]['path']).parent == Path(config['path']).parent, 'stage escaped config directory'
    assert known['original']['path'] == config['path']
    assert current in [row['identity'] for row in known.values()], 'unowned config change; restoration refused'
    wanted = 'restored' if enabled else 'disabled'
    if enabled and current in (known['original']['identity'], known['restored']['identity']):
        return
    assert current == known['disabled' if enabled else 'original']['identity'], 'invalid config transition'
    staged = known[wanted]
    assert file_identity(staged['path']) == staged['identity'], 'staged config changed'
    assert file_identity(config['path']) == current, 'config changed before atomic replacement'
    if before_replace:
        before_replace()
    os.replace(staged['path'], config['path'])
    assert file_identity(config['path']) == staged['identity'], 'atomic config replacement not observed'


def verify_status(status, enabled):
    assert 'experiment' not in status, 'signed experiment is not production evidence'
    production = status.get('input', {})
    assert production.get('protocol') == 3 and production.get('test_only') is False, 'production v3 required'
    assert production.get('seat_lifetime') == 'compositor' and production.get('upgrade') == 'desktop_restart'
    assert status.get('configured') is enabled and status.get('transport', {}).get('ready') is enabled
    assert production.get('transport_ready') is enabled, 'input transport state not acknowledged'
    assert len(production.get('lanes', [])) == 2 and {r['lane'] for r in production['lanes']} == {0, 1}
    if not enabled:
        assert all(type(row.get('held_button')) is int and row['held_button'] == 0 and
                   type(row.get('held_keys')) is int and row['held_keys'] == 0 and
                   row.get('drag_active') is False and row.get('lease_active') is False
                   for row in production['lanes']), 'disabled input still held'
    return status


def production_status(instance, enabled):
    return verify_status(json.loads(_hypr(instance, '-j', 'cua:status')), enabled)


def verify_trace_peer(trace, config):
    peer = struct.unpack('3i', trace.socket.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    assert peer[:2] == (config['compositor']['pid'], config['compositor']['uid']), 'wrong trace compositor peer'
    assert trace.hello['protocol'] == 3
    _guard(config)


def connect_trace(path, config):
    _guard(config)
    expected = Path(os.environ['XDG_RUNTIME_DIR']) / 'hypr' / config['instance'] / path.name
    assert path.is_absolute() and path == expected and path.resolve(strict=True) == path, 'wrong trace socket path'
    info = path.lstat()
    assert stat.S_ISSOCK(info.st_mode) and info.st_uid == config['compositor']['uid']
    trace = Trace(path)
    try:
        verify_trace_peer(trace, config)
        return trace
    except BaseException:
        trace.close()
        raise


def _reload(config, enabled):
    _guard(config)
    assert _hypr(config['instance'], 'reload') == 'ok', 'config reload refused'
    return production_status(config['instance'], enabled or config.get('kind') == 'keymap')


def restore_config(config):
    with _locked(config):
        started = time.monotonic_ns()
        untouched = file_identity(config['path']) == config['files']['original']['identity']
        _replace(config, True)
        status = production_status(config['instance'], True) if untouched else _reload(config, True)
        original, _ = fixed_bytes(config.get('kind', 'config_disable'))
        assert Path(config['path']).read_bytes() == original.encode(), 'exact config bytes not restored'
        options = keymap_options(config['instance'], True) if config.get('kind') == 'keymap' else None
        record = {'result': 'restored', 'started_ns': started, 'observed_ns': time.monotonic_ns(),
                  'config': file_identity(config['path']), 'status': status}
        if options is not None:
            record['keymap_options'] = options
        return record


def watchdog(config, cancel_fd):
    record = {'result': 'failed'}
    try:
        _guard(config)
        print('ARMED', flush=True)
        delay = max(0, (config['deadline_ns'] - time.monotonic_ns()) / 1e9)
        ready = select.select([cancel_fd], [], [], delay)[0]
        if ready and os.read(cancel_fd, 1) == b'C':
            record = {'result': 'cancelled_after_restoration'}
        else:
            record = restore_config(config)
    except BaseException as error:
        record['error'] = str(error)
    finally:
        Path(config['record']).write_text(json.dumps(record))


class ConfigFault:
    def __init__(self, plan, evidence):
        validate_plan(plan)
        kind = plan['fault']['kind']
        original, changed = fixed_bytes(kind)
        assert platform.system() == 'Linux', 'disposable Linux VM required'
        assert subprocess.run(['systemd-detect-virt', '--vm', '--quiet'], timeout=2).returncode == 0
        expected = plan['compositor']
        compositor = {key: expected[key] for key in ('pid', 'uid', 'starttime', 'exe')}
        assert compositor['uid'] == os.getuid() and _identity(compositor['pid']) == compositor
        path = Path(plan['config']['path'])
        assert file_identity(path) == {k: v for k, v in plan['config'].items() if k != 'path'}
        assert path.read_bytes() == original.encode(), 'not the dedicated sourced include'
        parent = path.parent.stat()
        assert parent.st_uid == os.getuid() and not parent.st_mode & 0o022, 'private owned config directory required'
        self.config = {'kind': kind, 'vm': plan['vm'], 'compositor': compositor, 'instance': expected['instance'],
                       'path': str(path), 'directory': [parent.st_dev, parent.st_ino, parent.st_uid, stat.S_IMODE(parent.st_mode)],
                       'record': str((evidence / 'config-watchdog.json').resolve()), 'files': {
                           'original': {'path': str(path), 'identity': file_identity(path)}}}
        self.record, self.restoration = {'result': 'unproven', 'kind': kind}, None
        if 'pointer_cleanup' in plan['fault']:
            self.config['pointer_cleanup'] = self.record['pointer_cleanup'] = pointer_cleanup(plan['fault'])
        if pointer_cleanup(plan['fault']) == 'retained_inert':
            self.record['target'] = dict(plan['agents'][0]['target'])
            self.record['bounds'] = dict(plan['agents'][0]['bounds'])
        if 'min_motion_px' in plan['fault']:
            self.config['min_motion_px'] = self.record['min_motion_px'] = plan['fault']['min_motion_px']
        self.child = self.cancel_fd = None
        self.mutated = False
        _guard(self.config)
        self.record['before'] = production_status(expected['instance'], True)
        (evidence / 'pre-fault-status.json').write_text(json.dumps(self.record['before']))
        if kind == 'keymap':
            self.record['keymap_before'] = keymap_options(expected['instance'], True)
        if kind == 'keymap' or pointer_cleanup(self.config) == 'retained_inert':
            idle_lanes(self.record['before'])
        # Files are prepared before the watchdog and before any live drag.
        # Known inodes let either process reject an unrelated replacement.
        try:
            for name, data in (('disabled', changed.encode()), ('restored', original.encode())):
                fd, staged = tempfile.mkstemp(prefix='.cua-config-proof-', suffix='.stage', dir=path.parent)
                with os.fdopen(fd, 'wb') as stream:
                    os.fchmod(stream.fileno(), plan['config']['mode'])
                    stream.write(data)
                    stream.flush()
                    os.fsync(stream.fileno())
                self.config['files'][name] = {'path': staged, 'identity': file_identity(staged)}
        except BaseException:
            self._clean_staged()
            raise

    def _clean_staged(self):
        for name, row in self.config['files'].items():
            if name != 'original' and Path(row['path']).exists():
                assert file_identity(row['path']) == row['identity'], 'staged file changed; preserve it'
                Path(row['path']).unlink()

    def arm(self):
        assert self.child is None, 'one watchdog per episode'
        self.config['deadline_ns'] = time.monotonic_ns() + WATCHDOG_SECONDS[self.config['kind']] * 1_000_000_000
        reader, self.cancel_fd = os.pipe()
        try:
            self.child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()),
                '--watchdog', json.dumps(self.config), str(reader)], pass_fds=(reader,),
                stdout=subprocess.PIPE, text=True, start_new_session=True)
            assert select.select([self.child.stdout], [], [], 2)[0], 'watchdog failed to arm'
            assert self.child.stdout.readline().strip() == 'ARMED', 'watchdog rejected identity'
        finally:
            os.close(reader)

    def inject(self, trace, initial, pending, guard):
        assert self.child and self.child.poll() is None, 'live restoration watchdog required'
        prefix, lanes = poll_fault_active(trace, initial, pending, self.config.get('min_motion_px'))
        gate_ns = time.monotonic_ns()
        with _locked(self.config):
            if self.config.get('kind') == 'keymap' or pointer_cleanup(self.config) == 'retained_inert':
                self.record['gate_status'] = production_status(self.config['instance'], True)
                previous = keymap_lanes(self.record['before'])
                current = keymap_lanes(self.record['gate_status'])
                for lane in previous:
                    assert all(previous[lane][key] == current[lane][key] for key in ('epoch', 'desktop_generation'))
                active = current[next(iter(lanes)) - 1]
                assert active['drag_active'] is True and active['lease_active'] is True
                assert type(active['held_button']) is int and active['held_button'] > 0, 'drag ended before keymap fault'
                if pointer_cleanup(self.config) == 'retained_inert':
                    assert active.get('pointer_focus') is True, 'missing live pointer at fault gate'
            if self.config.get('kind') == 'keymap':
                self.record['keymap_before'] = keymap_options(self.config['instance'], True)
            def authorize():
                # Run AFTER all potentially blocking identity checks, directly
                # before replacement: a slow guard must not authorize stale input.
                assert self.child.poll() is None
                guard()
                assert not pending.done(), 'drag returned before config fault'
                requested_ns = time.monotonic_ns()
                assert 0 <= requested_ns - gate_ns <= 250_000_000, 'stale fault gate'
                assert requested_ns + 3_000_000_000 < self.config['deadline_ns'], 'watchdog deadline too near'
                self.record.update(prefix=prefix, lane=next(iter(lanes)), gate_ns=gate_ns,
                                   requested_ns=requested_ns, watchdog_deadline_ns=self.config['deadline_ns'])
                self.config['lane'] = self.record['lane']
                self.mutated = True  # Lost replies still require restoration.
            _replace(self.config, False, before_replace=authorize)
            self.record['after'] = _reload(self.config, False)
            if self.config.get('kind') == 'keymap':
                self.record['keymap_after'] = keymap_options(self.config['instance'], False)
                verify_keymap_transition(self.record['gate_status'], self.record['after'],
                                         pointer_cleanup(self.config), self.record['lane'])
            elif pointer_cleanup(self.config) == 'retained_inert':
                verify_retained_inert(self.record['gate_status'], self.record['after'], self.record['lane'], enabled=False)
            self.record['acknowledged_ns'] = time.monotonic_ns()
            self.record['config'] = file_identity(self.config['path'])
            self.record['result'] = 'observed'
        guard()
        return prefix, self.record['lane']

    def restore(self):
        if self.restoration is None:
            if self.mutated:
                self.restoration = restore_config(self.config)
            else:
                self.restoration = {'result': 'not_needed'}
            if self.cancel_fd is not None:
                try:
                    os.write(self.cancel_fd, b'C')
                except BrokenPipeError:
                    pass
        return self.restoration

    def close(self):
        try:
            self.restore()
        finally:
            if self.cancel_fd is not None:
                os.close(self.cancel_fd)  # EOF independently restores after failure.
                self.cancel_fd = None
            if self.child:
                self.child.wait(timeout=5)
                self.child.stdout.close()
            if self.restoration is not None:
                self._clean_staged()


def verify_cancelled(boundary, record, before_restore_ns):
    """Check the live fault before any restoration or new action is attempted."""
    prefix, lane = record['prefix'], record['lane']
    policy = pointer_cleanup(record)
    assert set(active_drags(prefix)) == {lane}
    if 'min_motion_px' in record:
        validate_min_motion(record['min_motion_px'])
        assert drag_motion_px(prefix, lane) >= record['min_motion_px'], 'fault preceded required drag motion'
    tail = trace_interval(prefix, boundary)
    assert prefix['events'][-1][1] <= record['gate_ns'] <= record['requested_ns'] <= record['acknowledged_ns']
    assert record['acknowledged_ns'] <= before_restore_ns
    assert 0 <= record['requested_ns'] - record['gate_ns'] <= 250_000_000
    synthetic = [row for row in tail if row[5] in (1, 2)]
    assert all(row[5] == lane for row in synthetic), 'cleanup crossed lanes'
    cancelled = [row for row in synthetic if row[2] == 'agent_cancel']
    assert len(cancelled) == 1 and record['requested_ns'] <= cancelled[0][1] < before_restore_ns, 'no cancellation during fault'
    assert not any(row[2] in ('agent_admitted', 'agent_drag_start', 'agent_drag_end', 'agent_action_end',
                              'keyboard_key', 'pointer_axis') for row in synthetic), 'extra action or false completion'
    releases = [row for row in synthetic if row[2] == 'pointer_button']
    assert len(releases) == 1 and releases[0][6] == 0 and releases[0][0] > cancelled[0][0]
    assert releases[0][1] < before_restore_ns, 'release was delayed until restoration'
    assert not any(row[2] in ('pointer_motion', 'pointer_enter') and row[0] > cancelled[0][0] for row in synthetic)
    if policy == 'retained_inert':
        drag_motion_px(prefix, lane)  # Requires coordinates and unchanged pre-press pointer presence.
        assert not any(row[2] in ('pointer_leave', 'pointer_enter') for row in synthetic), 'retained pointer left or retargeted'
        assert all(row[2] in ('agent_cancel', 'pointer_button', 'keyboard_leave')
                   for row in synthetic if row[0] >= cancelled[0][0]), 'retained pointer was not inert'
    stopped = stopped_prefix(boundary)
    isolation = analyze(stopped)
    assert isolation['result'] == 'passed' and released_synthetic_input(stopped), isolation
    return isolation


def verify_layout_refusal(record):
    policy = pointer_cleanup(record)
    assert record['outcome'] == 'response' and record['replayed'] is False
    assert record['tool'] in ('click', 'scroll')
    assert type(record['runtime_pid']) is int and record['runtime_pid'] != record['previous_runtime_pid']
    assert record['prepared_ns'] <= record['dispatch_ns'] <= record['observed_ns']
    assert record['dispatch_ns'] - record['prepared_ns'] <= MAX_GROUNDING_AGE_NS
    assert record['snapshot']['window_bounds'] == record['after_snapshot']['window_bounds'], 'refused target geometry changed'
    check_response(record['response'], {'kind': 'refused', 'reason': 'unsupported_layout'})
    content = record['response']['structuredContent']
    assert content.get('route') == 'synthetic_events' and content.get('detail') == 'unsupported_layout'
    assert content.get('code') == 'background_unavailable', 'not the compositor layout refusal'
    before, after = keymap_lanes(record['before'], cleared=policy == 'cleared'), keymap_lanes(record['after'], cleared=policy == 'cleared')
    if policy == 'retained_inert':
        verify_retained_inert(record['before'], record['before'], record['lane'])
        verify_refusal_claim(record['before'], record['after'], record['response'], record['lane'])
        verify_target_snapshots(record['target'], record['bounds'], record['snapshot'], record['after_snapshot'])
    for lane in before:
        assert all(before[lane][key] == after[lane][key] for key in ('epoch', 'desktop_generation', 'dispatches')), \
            'layout refusal changed compositor state or dispatched input'
    verify_keymap_options(record['keymap_options'], False)
    tail = trace_interval(record['trace_before'], record['trace_after'])
    assert not any(row[5] in (1, 2) for row in tail), 'refused fresh action dispatched synthetic input'
    assert analyze(stopped_prefix(record['trace_after']))['result'] == 'passed'
    if policy == 'retained_inert':
        verify_refusal_closed(record, record['lane'])
    return {'result': 'verified', 'reason': 'unsupported_layout', 'no_dispatch': 'verified',
            'compiled_map_invalidation': 'verified', 'keymap_hash': 'not_exposed'}


def verify_refusal_claim(before, after, response, interrupted_lane=None):
    """A fresh CLAIM reserves capacity, never TARGET authority or inherited hover.

    This exception is only for the fresh refusal probe between its action and
    EOF. Fault cancellation and post-EOF checks still require no reservation.
    The response identifies the claimed lane; no status owner PID is exposed.
    """
    old, new = idle_lanes(before), keymap_lanes(after)
    claimed = response['structuredContent'].get('lane')
    assert type(claimed) is int and claimed in (0, 1), 'refusal has no exact claimed lane'
    if interrupted_lane is not None:
        assert type(interrupted_lane) is int and interrupted_lane in (1, 2)
        assert old[interrupted_lane - 1]['pointer_focus'] is True
    for lane, row in new.items():
        assert type(old[lane].get('pointer_focus')) is bool
        assert row.get('pointer_focus') is old[lane]['pointer_focus'], 'refusal changed pointer presence'
        assert row.get('reserved') is (lane == claimed), 'unexpected refusal reservation'
        assert all(type(row.get(k)) is int and row[k] == 0 for k in ('held_button', 'held_keys'))
        assert all(row.get(k) is False for k in ('lease_active', 'drag_active', 'keyboard_focus')), 'refused CLAIM gained input authority'
        assert all(row[k] == old[lane][k] for k in ('epoch', 'desktop_generation', 'dispatches')), 'refusal changed desktop or dispatched'
    return {'claimed_lane': claimed, 'capacity_only': True, 'input_authority': False,
            'owner_pid_in_status': 'not_exposed'}


def verify_refusal_closed(record, interrupted_lane):
    closed = record['closure']
    assert closed['runtime_pid'] == record['runtime_pid'] and type(closed['exit_code']) is int
    assert record['observed_ns'] <= closed['started_ns'] <= closed['reaped_ns'] <= closed['observed_ns']
    verify_retained_inert(record['before'], closed['status'], interrupted_lane)
    old, new = keymap_lanes(record['after']), keymap_lanes(closed['status'])
    for lane in old:
        assert all(old[lane][k] == new[lane][k] for k in ('epoch', 'desktop_generation', 'dispatches'))
    assert not any(row[5] in (1, 2) for row in trace_interval(record['trace_after'], closed['trace'])), 'probe EOF changed inert pointer or emitted input'
    return {'result': 'verified', 'reservation_released': True}


def refuse_new_action(client, observer, victim, spec, stage, trace, config, guard, save):
    """Exercise real Driver admission while the compiled physical map is invalid."""
    assert victim.process.poll() is not None, 'old runtime must be reaped before refusal probe'
    assert victim.process.pid not in assert_distinct_runtimes([client, observer])
    fresh = {**spec, 'name': spec['name'] + '-wrong-layout', 'pointer_stage': stage}
    assert not client.tool('start_session', {'session': fresh['name']}).get('isError')
    app_process_identity(spec['app'], spec['target']['pid'])
    prepared_ns = time.monotonic_ns()
    snapshot = grounded_snapshot(client, spec['target'], fresh)
    arguments, _ = pointer_grounding.action(snapshot, pointer_grounding.read_pixels(snapshot['proof_image']), spec['app'], stage)
    tool = pointer_grounding.STAGES[spec['app']][stage]
    assert tool in ('click', 'scroll'), 'never replay the interrupted drag'
    record = {'outcome': 'unknown', 'replayed': False, 'runtime_pid': client.process.pid,
              'previous_runtime_pid': victim.process.pid, 'tool': tool, 'prepared_ns': prepared_ns,
              'snapshot': snapshot, 'arguments': arguments, 'session': fresh['name'],
              'before': production_status(config['instance'], True), 'trace_before': trace.collect()}
    policy = pointer_cleanup(config)
    if policy == 'retained_inert':
        record.update(pointer_cleanup=policy, lane=config['lane'], target=dict(spec['target']), bounds=dict(spec['bounds']))
        verify_retained_inert(record['before'], record['before'], record['lane'])
    else:
        keymap_lanes(record['before'], cleared=True)
    record['keymap_options'] = keymap_options(config['instance'], False)
    guard()
    _guard(config)
    assert file_identity(config['path']) == config['files']['disabled']['identity']
    record['dispatch_ns'] = time.monotonic_ns()
    assert record['dispatch_ns'] - prepared_ns <= MAX_GROUNDING_AGE_NS
    assert record['dispatch_ns'] + 1_000_000_000 < config['deadline_ns'], 'watchdog deadline too near'
    save('wrong-layout-action.json', record)
    try:
        record['response'] = client.tool(tool, {**arguments, **spec['target'], 'session': fresh['name'],
                                               'delivery_mode': 'background'})
        record['outcome'] = 'response'
        record['after_snapshot'] = grounded_snapshot(observer, spec['target'], fresh, session=False)
        record['after'] = production_status(config['instance'], True)
        record['trace_after'] = trace.collect()
        record['keymap_options'] = keymap_options(config['instance'], False)
        record['observed_ns'] = time.monotonic_ns()
        assert record['observed_ns'] < config['deadline_ns'], 'watchdog restored during refusal probe'
        guard()
        if policy == 'retained_inert':
            record['claim'] = verify_refusal_claim(record['before'], record['after'], record['response'], record['lane'])
            closed = record['closure'] = {'runtime_pid': client.process.pid, 'started_ns': time.monotonic_ns()}
            close_owned(client)
            closed.update(reaped_ns=time.monotonic_ns(), exit_code=client.process.poll())
            closed.update(status=production_status(config['instance'], True), trace=trace.collect(), observed_ns=time.monotonic_ns())
            assert closed['observed_ns'] < config['deadline_ns'], 'watchdog restored during probe closure'
            guard()
        record['verification'] = verify_layout_refusal(record)
        return record
    finally:
        save('wrong-layout-action.json', record)


def verify_target_snapshots(target, bounds, before, after):
    for snapshot in (before, after):
        assert all(type(target.get(key)) is int and target[key] > 0 and
                   type(snapshot.get(key)) is int and snapshot[key] == target[key]
                   for key in ('pid', 'window_id')), 'retained hover target identity changed or missing'
        assert snapshot['window_bounds'] == bounds, 'retained hover target geometry changed'
        assert isinstance(snapshot.get('snapshot_id'), str) and snapshot['snapshot_id'], 'missing target snapshot identity'
        runtime = snapshot.get('proof_runtime', {})
        assert type(runtime.get('pid')) is int and runtime['pid'] > 0, 'missing observation runtime'
    # Snapshot counters belong to the observing Driver process, not the app.
    assert (before['proof_runtime']['pid'], before['snapshot_id']) != \
           (after['proof_runtime']['pid'], after['snapshot_id']), 'reused target snapshot'
    times = [snapshot.get(key) for snapshot in (before, after)
             for key in ('proof_observation_started_ns', 'proof_observation_finished_ns')]
    assert all(type(value) is int and value >= 0 for value in times), 'invalid observation timestamp'
    assert times == sorted(times) and times[0] < times[2], 'stale or out-of-order observation'


def verify_fault(boundary, record, restoration, action):
    policy = pointer_cleanup(record)
    original, changed = fixed_bytes(record['kind'])
    assert record['result'] == 'observed' and restoration['result'] == 'restored'
    assert record['config']['sha256'] == digest(changed.encode())
    assert restoration['config']['sha256'] == digest(original.encode())
    verify_status(record['before'], True)
    verify_status(record['after'], record['kind'] == 'keymap')
    verify_status(restoration['status'], True)
    assert record['acknowledged_ns'] <= restoration['started_ns'] <= restoration['observed_ns']
    assert restoration['observed_ns'] < record['watchdog_deadline_ns'], 'watchdog recovery cannot certify the episode'
    if record['kind'] == 'keymap':
        verify_keymap_options(record['keymap_before'], True)
        verify_keymap_options(record['keymap_after'], False)
        verify_keymap_options(restoration['keymap_options'], True)
        assert record['keymap_before'] == restoration['keymap_options'], 'original map options not restored'
        verify_keymap_transition(record['gate_status'], record['after'], policy, record['lane'])
        refusal = record['wrong_layout']
        assert pointer_cleanup(refusal) == policy, 'refusal changed pointer cleanup contract'
        if policy == 'retained_inert':
            assert refusal['lane'] == record['lane'] and refusal['target'] == record['target'] and refusal['bounds'] == record['bounds']
            verify_retained_inert(record['after'], refusal['before'], record['lane'])
            assert refusal['closure']['observed_ns'] < restoration['started_ns'], 'probe not closed before restoration'
        verify_layout_refusal(refusal)
        after, before_refusal = keymap_lanes(record['after']), keymap_lanes(refusal['before'])
        for lane in after:
            assert all(after[lane][key] == before_refusal[lane][key] for key in ('epoch', 'desktop_generation', 'dispatches')), \
                'keymap state changed before fresh refusal'
        trace_interval(record['prefix'], refusal['trace_before'])
        trace_interval(refusal['trace_after'], boundary)
        assert record['acknowledged_ns'] <= refusal['prepared_ns'] <= refusal['observed_ns'] < restoration['started_ns']
        verify_keymap_transition(refusal['after'], restoration['status'], policy, record['lane'])
    if policy == 'retained_inert':
        gate = keymap_lanes(record['gate_status'])
        initial = idle_lanes(record['before'])
        assert all(initial[lane][key] == gate[lane][key]
                   for lane in gate for key in ('epoch', 'desktop_generation')), 'stale native drag gate'
        active = gate[record['lane'] - 1]
        assert active['drag_active'] is True and active['lease_active'] is True
        assert type(active['held_button']) is int and active['held_button'] > 0, 'retained proof has no live native drag gate'
        after = verify_retained_inert(record['gate_status'], record['after'], record['lane'], enabled=record['kind'] == 'keymap')
        restored = verify_retained_inert(record['after'], restoration['status'], record['lane'])
        observed = verify_retained_inert(restoration['status'], record['target_status'], record['lane'])
        assert all(restored[lane][key] == observed[lane][key]
                   for lane in restored for key in ('epoch', 'desktop_generation')), 'desktop changed during target observation'
        if record['kind'] == 'config_disable':
            for lane in gate:
                assert gate[lane]['epoch'] == after[lane]['epoch'], 'disabled compositor lane replaced'
                assert after[lane]['epoch'] != restored[lane]['epoch'], 'restoration reused old admission epoch'
                assert gate[lane]['desktop_generation'] <= after[lane]['desktop_generation'] <= restored[lane]['desktop_generation']
        verify_target_snapshots(record['target'], record['bounds'], record['target_before'], record['target_after'])
    isolation = verify_cancelled(boundary, record, restoration['started_ns'])
    result = {'result': 'verified', 'outcome': fault_outcome(action), 'continuous_isolation': isolation,
              'synthetic_cleanup': 'verified', 'saved_document_effect': 'unproven'}
    if record['kind'] == 'keymap':
        result['wrong_layout'] = verify_layout_refusal(record['wrong_layout'])
    if policy == 'retained_inert':
        result['pointer_cleanup'] = {'policy': policy, 'presence_continuity': 'verified',
                                     'grounded_target_identity': 'verified', 'wayland_surface_identity': 'not_exposed'}
    return result


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2))
    report = {'result': 'failed', 'scope': 'native-config-disable', 'recovery': {'result': 'unproven'},
              'full_desktop_matrix': False, 'physical_hardware': False}
    clients, observer, grab, trace, fault, pool, future = [], None, None, None, None, None, None
    prefix = primary_before = baseline = deadline = None
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
        report['scope'] = 'native-' + plan['fault']['kind'].replace('_', '-')
        assert args.trace_socket.name in ('cua-input-v3.sock', 'cua-input-v3-2.sock')
        origin = provenance(args, plan)
        for name in ('production_desktop_fault_proof.py', 'production_desktop_fault_proof_test.py',
                     'production_geometry_fault_proof.py', 'production_cancel_proof.py',
                     'input_config_toggle.py', 'desktop_faults.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': digest(path.read_bytes())}
        save('provenance.json', origin)
        fault = ConfigFault(plan, args.evidence)
        spec = plan['agents'][0]
        clients.append(launch('agent'))
        assert not clients[0].tool('start_session', {'session': spec['name']}).get('isError')
        observer = launch('observer')
        report['driver_processes'] = assert_distinct_runtimes([clients[0], observer])
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
        assert not active_drags(initial) and not any(row[5] in (1, 2) for row in initial['events'])
        prepared = prepare_drag(clients[0], spec)
        if pointer_cleanup(plan['fault']) == 'retained_inert':
            fault.record['target_before'] = prepared['snapshot']
        save('drag-grounding.json', prepared)
        fault.arm()
        guard()
        pool = ThreadPoolExecutor(max_workers=1)
        future = pool.submit(call_drag, clients[0], spec, prepared, guard)
        _, lane = fault.inject(trace, initial, future, guard)
        save('fault.json', fault.record)
        report['action'] = future.result(timeout=5)
        save('drag-action.json', report['action'])
        if plan['fault']['kind'] == 'keymap':
            cancelled = trace.collect()
            save('keymap-cancelled-prefix.json', cancelled)
            verify_cancelled(cancelled, fault.record, time.monotonic_ns())
            close_owned(clients[0])
            clients.append(launch('wrong-layout'))
            fault.record['wrong_layout'] = refuse_new_action(
                clients[-1], observer, clients[0], spec, plan['recovery']['pointer_stage'],
                trace, fault.config, guard, save)
            close_owned(clients[-1])
            save('fault.json', fault.record)
        restoration = fault.restore()
        save('restoration.json', restoration)
        trace.close()
        trace = None
        trace = connect_trace(args.trace_socket, fault.config)  # NEVER reset trace history.
        boundary = trace.collect()
        save('fault-prefix.json', boundary)
        if pointer_cleanup(plan['fault']) == 'retained_inert':
            interrupted = grounded_snapshot(observer, spec['target'], spec, session=False)
            fault.record['target_after'] = interrupted
            observed = production_status(fault.config['instance'], True)
            verify_retained_inert(restoration['status'], observed, lane)
            fault.record['target_status'] = observed
            save('interrupted-status.json', observed)
            boundary = trace.collect()
            save('fault-prefix.json', boundary)
        report['fault'] = verify_fault(boundary, fault.record, restoration, report['action'])
        if pointer_cleanup(plan['fault']) == 'cleared':
            interrupted = grounded_snapshot(observer, spec['target'], spec, session=False)
        save('interrupted-state.json', {'snapshot': interrupted,
                                       'action': report['action'], 'replayed': False})
        close_owned(clients[0])
        teardown = trace.collect()
        save('pre-recovery-prefix.json', teardown)
        report['runtime_teardown'] = verify_recovery_cleanup(boundary, stopped_prefix(teardown))
        if pointer_cleanup(plan['fault']) == 'retained_inert':
            verify_cancelled(teardown, fault.record, restoration['started_ns'])
        clients.append(launch('recovery'))
        prefix = recover(clients[-1], observer, clients[0], spec, plan['recovery']['pointer_stage'],
                         trace, teardown, lane, guard, save, report['recovery'])
        save('recovery-prefix.json', prefix)
        guard()
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        operations = [(f'close_agent_{i}', lambda c=c: close_owned(c)) for i, c in enumerate(clients)]
        if future:
            def preserve_action():
                try:
                    report['action'] = future.result(timeout=5)
                except Exception as error:
                    report['action'] = {'outcome': 'unknown', 'error': str(error), 'replayed': False}
                save('drag-action.json', report['action'])
            operations.append(('preserve_action', preserve_action))
        if pool:
            operations.append(('shutdown_pool', lambda: pool.shutdown(wait=False, cancel_futures=True)))
        if fault:
            operations += [('preserve_fault', lambda: save('fault.json', fault.record)),
                           ('restore_config', lambda: save('restoration.json', fault.restore())),
                           ('close_fault', fault.close)]
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
                assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held'))
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
