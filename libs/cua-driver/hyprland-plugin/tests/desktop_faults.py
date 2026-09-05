"""External adversaries for a selected disposable Hyprland VM, never agent input.

Native agent actions belong to Cua Driver. Run a separate fault-only episode to
measure legitimate lock/DPMS primary-seat changes. No control claims that those
changes are uninterrupted-user behavior. Lock-client failure can leave Wayland
locked by design: discard the disposable guest if graceful recovery fails.
"""
import json
import os
from pathlib import Path
import platform
import re
import select
import signal
import subprocess
import sys
import time


def _identity(pid):
    if type(pid) is not int or pid <= 1:
        raise ValueError('an exact non-init PID is required')
    root = Path(f'/proc/{pid}')
    fields = (root / 'stat').read_text().rsplit(')', 1)[1].split()
    return {'pid': pid, 'starttime': fields[19], 'uid': root.stat().st_uid,
            'exe': str((root / 'exe').resolve(strict=True))}


def _hypr(instance, *args):
    return subprocess.check_output(['hyprctl', '-i', instance, *args],
                                   text=True, timeout=2).strip()


def _same_compositor(identity, instance):
    if _identity(identity['pid']) != identity:
        raise RuntimeError('selected compositor identity changed')
    instances = json.loads(subprocess.check_output(['hyprctl', '-j', 'instances'],
                                                  text=True, timeout=2))
    if len([row for row in instances if row.get('instance') == instance and
            row.get('pid') == identity['pid']]) != 1:
        raise RuntimeError('selected compositor instance no longer matches PID')


def _wait(predicate, timeout=3):
    deadline = time.monotonic() + timeout
    while True:
        value = predicate()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise TimeoutError('fault state was not acknowledged')
        time.sleep(.02)


def _watchdog(config, cancel_fd, target_fd):
    """Private fixed-operation child; EOF also recovers after controller death."""
    print('ARMED', flush=True)
    readable = select.select([cancel_fd], [], [], config['seconds'])[0]
    if readable and os.read(cancel_fd, 1) == b'C':
        return
    record = {'started_ns': time.monotonic_ns(), 'restored': False}
    try:
        _same_compositor(config['compositor'], config['instance'])
        if config['kind'] == 'lock':
            # Killing a lock client would leave an orphaned lock. Request its
            # protocol unlock; the client also has its own monotonic deadline.
            signal.pidfd_send_signal(target_fd, signal.SIGUSR1)
            record['unlock_requested'] = True
        else:
            if _hypr(config['instance'], 'dispatch', 'dpms', 'on') != 'ok':
                raise RuntimeError('DPMS recovery dispatcher refused')
            def recovered():
                _same_compositor(config['compositor'], config['instance'])
                rows = json.loads(_hypr(config['instance'], '-j', 'monitors', 'all'))
                return rows and all(row.get('dpmsStatus') is True for row in rows)
            _wait(recovered)
            record['restored'] = True
    except Exception as error:
        record['error'] = str(error)
    finally:
        record['observed_ns'] = time.monotonic_ns()
        Path(config['record']).write_text(json.dumps(record))


class FaultController:
    def __init__(self, *, compositor_pid, instance, target, disposable, evidence,
                 lock_fixture=None, move_to=None, resize_to=None, hold_seconds=12,
                 compositor_exe=Path('/usr/bin/Hyprland')):
        if disposable is not True or platform.system() != 'Linux':
            raise ValueError('faults require explicit disposable Linux guest confirmation')
        if subprocess.run(['systemd-detect-virt', '--vm', '--quiet'], timeout=2).returncode:
            raise ValueError('faults require a detected disposable virtual machine')
        if not isinstance(instance, str) or not re.fullmatch(r'[A-Za-z0-9_.-]+', instance):
            raise ValueError('invalid explicit compositor instance')
        if os.environ.get('HYPRLAND_INSTANCE_SIGNATURE') != instance:
            raise ValueError('selected compositor differs from the test session')
        if type(hold_seconds) is not int or not 2 <= hold_seconds <= 20:
            raise ValueError('fault recovery deadline must be 2..20 seconds')
        if not re.fullmatch(r'0x[0-9a-fA-F]+', target.get('address', '')):
            raise ValueError('exact hexadecimal fixture address required')
        self.instance, self.target = instance, dict(target)
        self.compositor = _identity(compositor_pid)
        if (self.compositor['exe'] != str(Path(compositor_exe).resolve(strict=True)) or
                Path(self.compositor['exe']).name != 'Hyprland' or
                self.compositor['uid'] != os.getuid()):
            raise ValueError('selected compositor executable or owner mismatch')
        _same_compositor(self.compositor, instance)
        self.fixture = _identity(target['pid'])
        self.fixture_fd = os.pidfd_open(target['pid'])
        try:
            self._validate_fixture()
        except BaseException:
            os.close(self.fixture_fd)
            raise
        self.evidence = Path(evidence)
        self.evidence.mkdir(parents=True, exist_ok=True)
        if any((self.evidence / name).exists() for name in ('fault.json', 'restoration.json', 'watchdog.json')):
            os.close(self.fixture_fd)
            raise ValueError('fault evidence must not overwrite an existing episode')
        self.lock_fixture = Path(lock_fixture) if lock_fixture else None
        self.move_to, self.resize_to = move_to, resize_to
        self.hold_seconds = hold_seconds
        self.kind = self.before = self.lock_process = self.watchdog = None
        self.lock_state = 'unknown'
        self.cancel_fd = self.lock_fd = None
        self.restoration = None
        self.mutated = False
        self.lock_buffer = b''
        self.lock_events = []
        self.lock_ack_ns = None

    def _validate_fixture(self):
        if select.select([self.fixture_fd], [], [], 0)[0] or _identity(self.target['pid']) != self.fixture:
            raise ValueError('selected fixture PID exited or changed')
        if self.fixture['uid'] != os.getuid():
            raise ValueError('fixture is not owned by the test user')
        cmd = Path(f'/proc/{self.target["pid"]}/cmdline').read_bytes().split(b'\0')
        argv = [item.decode() for item in cmd if item]
        fixture = Path(__file__).resolve().parents[2] / 'tests/fixtures/apps/linux/isolated-input/main.py'
        # Explicit interpreter + canonical repository fixture; arbitrary process
        # names, title matches, and user-supplied executable commands are refused.
        if (len(argv) < 6 or Path(argv[1]).resolve() != fixture or
                argv.count('--actor') != 1 or argv[argv.index('--actor') + 1] != 'Background' or
                '--journal' not in argv):
            raise ValueError('target is not the exact synthetic Background fixture')

    def snapshot(self):
        _same_compositor(self.compositor, self.instance)
        if self.lock_process:
            while True:
                event = self._read_lock_event()
                if event is None:
                    break
                self.lock_events.append(event)
        clients = json.loads(_hypr(self.instance, '-j', 'clients'))
        selected = [row for row in clients if row.get('address') == self.target['address']]
        if len(selected) > 1 or (selected and selected[0].get('pid') != self.target['pid']):
            raise RuntimeError('fixture address changed ownership')
        state = {'target': selected[0] if selected else None,
                'monitors': json.loads(_hypr(self.instance, '-j', 'monitors', 'all')),
                'activewindow': json.loads(_hypr(self.instance, '-j', 'activewindow')),
                'cursor': json.loads(_hypr(self.instance, '-j', 'cursorpos')),
                'lock': self.lock_state, 'lock_ack_ns': self.lock_ack_ns}
        state['observed_ns'] = time.monotonic_ns()
        return state

    def _dispatch(self, command, value):
        if not ((command == 'dpms' and value in ('on', 'off')) or
                (command in ('movewindowpixel', 'resizewindowpixel') and
                 re.fullmatch(r'exact -?\d+ -?\d+,address:' + re.escape(self.target['address']), value))):
            raise ValueError('unsupported fault dispatch')
        _same_compositor(self.compositor, self.instance)
        if command != 'dpms':
            self._validate_fixture()
            if self.snapshot()['target'] is None:
                raise RuntimeError('fixture disappeared before dispatch')
        if _hypr(self.instance, 'dispatch', command, value) != 'ok':
            raise RuntimeError('fault dispatcher did not acknowledge request')

    def _arm(self, kind):
        reader, self.cancel_fd = os.pipe()
        config = {'kind': kind, 'seconds': self.hold_seconds, 'compositor': self.compositor,
                  'instance': self.instance, 'record': str(self.evidence / 'watchdog.json')}
        passed = (reader,) + ((self.lock_fd,) if self.lock_fd is not None else ())
        try:
            self.watchdog = subprocess.Popen([sys.executable, str(Path(__file__).resolve()),
                '--watchdog', json.dumps(config), str(reader), str(self.lock_fd or -1)],
                pass_fds=passed, stdout=subprocess.PIPE, text=True, start_new_session=True)
            if (not select.select([self.watchdog.stdout], [], [], 2)[0] or
                    self.watchdog.stdout.readline().strip() != 'ARMED'):
                raise RuntimeError('fault recovery watchdog did not arm')
        finally:
            os.close(reader)

    def _read_lock_event(self):
        if b'\n' not in self.lock_buffer:
            if not select.select([self.lock_process.stdout], [], [], 0)[0]:
                return None
            chunk = os.read(self.lock_process.stdout.fileno(), 4096)
            if not chunk:
                if self.lock_state == 'unlocked':
                    return None
                raise RuntimeError('lock fixture exited before acknowledged recovery')
            self.lock_buffer += chunk
            if len(self.lock_buffer) > 16384:
                raise RuntimeError('oversized lock fixture response')
            if b'\n' not in self.lock_buffer:
                return None
        line, self.lock_buffer = self.lock_buffer.split(b'\n', 1)
        event = json.loads(line)
        self.lock_state, self.lock_ack_ns = event['event'], event['observed_ns']
        return event

    def _lock_event(self, expected):
        event = self.lock_events.pop(0) if self.lock_events else _wait(self._read_lock_event)
        if event['event'] != expected:
            raise RuntimeError(f'unexpected lock protocol event: {event}')
        return event

    def apply(self, kind):
        if self.kind is not None or kind not in ('destroy', 'move', 'resize', 'lock', 'dpms'):
            raise ValueError('one supported fault per controller')
        self._validate_fixture()
        self.before = self.snapshot()
        if self.before['target'] is None:
            raise ValueError('selected fixture is not mapped')
        self.kind = kind
        record = {'kind': kind, 'before': self.before, 'fault_ns': None, 'acknowledged': False}
        try:
            if kind in ('move', 'resize'):
                target = self.before['target']
                if target.get('floating') is not True or target.get('fullscreen', 0):
                    raise ValueError('geometry faults require an already-floating synthetic fixture')
                key, command = ('at', 'movewindowpixel') if kind == 'move' else ('size', 'resizewindowpixel')
                values = self.move_to if kind == 'move' else self.resize_to
                if (not isinstance(values, (tuple, list)) or len(values) != 2 or
                        any(type(v) is not int or abs(v) > 16384 for v in values) or
                        (kind == 'resize' and min(values) < 1) or list(values) == target[key]):
                    raise ValueError('explicit changed bounded geometry required')
                record['fault_ns'] = time.monotonic_ns()
                self.mutated = True
                self._dispatch(command, f'exact {values[0]} {values[1]},address:{self.target["address"]}')
                _wait(lambda: (self.snapshot()['target'] or {}).get(key) == list(values))
            elif kind == 'destroy':
                record['fault_ns'] = time.monotonic_ns()
                self.mutated = True
                signal.pidfd_send_signal(self.fixture_fd, signal.SIGTERM)
                _wait(lambda: select.select([self.fixture_fd], [], [], 0)[0] and self.snapshot()['target'] is None)
            elif kind == 'dpms':
                if not self.before['monitors'] or not all(row.get('dpmsStatus') is True for row in self.before['monitors']):
                    raise ValueError('DPMS fault requires all existing monitors acknowledged on')
                self._arm(kind)
                record['fault_ns'] = time.monotonic_ns()
                self.mutated = True
                self._dispatch('dpms', 'off')
                _wait(lambda: self._dpms(False))
            else:
                if not self.lock_fixture or self.lock_fixture.name != 'session_lock_fixture':
                    raise ValueError('owned session_lock_fixture binary required')
                # The helper connects to the selected compositor using its
                # inherited Wayland display; SO_PEERCRED validation is in C.
                self.lock_process = subprocess.Popen([str(self.lock_fixture.resolve(strict=True)),
                    str(self.hold_seconds * 1000), str(self.compositor['pid'])],
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, start_new_session=True)
                self.lock_fd = os.pidfd_open(self.lock_process.pid)
                self._lock_event('ready')
                self._arm(kind)
                record['fault_ns'] = time.monotonic_ns()
                self.mutated = True
                self.lock_process.stdin.write('LOCK\n')
                self.lock_process.stdin.flush()
                record['protocol'] = self._lock_event('locked')
            after = self.snapshot()
            if kind == 'lock' and after['lock'] != 'locked':
                raise RuntimeError('session lock ended before fault acknowledgment')
            if kind == 'dpms' and not all(row.get('dpmsStatus') is False for row in after['monitors']):
                raise RuntimeError('DPMS fault ended before acknowledgment')
            record.update(acknowledged=True, after=after, observed_ns=time.monotonic_ns())
            return record
        except BaseException as error:
            record.update(error=str(error), observed_ns=time.monotonic_ns())
            raise
        finally:
            (self.evidence / 'fault.json').write_text(json.dumps(record))

    def _dpms(self, value):
        rows = self.snapshot()['monitors']
        expected = {row['name'] for row in self.before['monitors']}
        return {row['name'] for row in rows} == expected and all(row.get('dpmsStatus') is value for row in rows)

    def rollback(self):
        if self.restoration is not None:
            return self.restoration
        record = {'kind': self.kind, 'restored': False, 'started_ns': time.monotonic_ns()}
        try:
            if self.mutated and self.kind in ('move', 'resize'):
                key, command = ('at', 'movewindowpixel') if self.kind == 'move' else ('size', 'resizewindowpixel')
                values = self.before['target'][key]
                self._dispatch(command, f'exact {values[0]} {values[1]},address:{self.target["address"]}')
                _wait(lambda: (self.snapshot()['target'] or {}).get(key) == values)
            elif self.mutated and self.kind == 'dpms':
                self._dispatch('dpms', 'on')
                _wait(lambda: self._dpms(True))
            elif self.mutated and self.kind == 'lock' and self.lock_process:
                # Safe even if the controller lost its stdin channel. Never
                # SIGKILL/SIGTERM a client that might own a protocol lock.
                if self.lock_process.poll() is None:
                    signal.pidfd_send_signal(self.lock_fd, signal.SIGUSR1)
                record['protocol'] = self._lock_event('unlocked')
                if self.lock_process.wait(timeout=3) != 0:
                    raise RuntimeError('lock fixture reported failed recovery')
            record.update(restored=True, after=self.snapshot(),
                          target_recreated=False if self.kind == 'destroy' else None)
            if self.cancel_fd is not None:
                try:
                    os.write(self.cancel_fd, b'C')
                except BrokenPipeError:
                    pass  # Timed recovery may already have completed.
            self.restoration = record
            return record
        except BaseException as error:
            record['error'] = str(error)
            raise
        finally:
            record['observed_ns'] = time.monotonic_ns()
            (self.evidence / 'restoration.json').write_text(json.dumps(record))

    def close(self):
        try:
            self.rollback()
        finally:
            # EOF asks the independent watchdog to restore on any failure.
            for name in ('cancel_fd', 'fixture_fd', 'lock_fd'):
                fd = getattr(self, name)
                if fd is not None:
                    os.close(fd)
                    setattr(self, name, None)
            for process in (self.lock_process, self.watchdog):
                if process:
                    if process.stdin:
                        process.stdin.close()
                    if process.poll() is not None and process.stdout:
                        process.wait(timeout=1)
                        process.stdout.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


if __name__ == '__main__':
    if len(sys.argv) != 5 or sys.argv[1] != '--watchdog':
        raise SystemExit('Private watchdog entry only; use FaultController from the harness')
    _watchdog(json.loads(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]))
