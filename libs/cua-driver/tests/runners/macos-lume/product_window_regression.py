#!/usr/bin/env python3
"""Focused native checks against an already running, signed standard PiP daemon.

Uses the public CLI and an independent read-only WindowServer observer. Never
installs a driver, changes grants, or stops the supplied daemon. See README.md.
"""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

BINARY = Path('/Applications/CuaDriverLocal.app/Contents/MacOS/cua-driver-local')
FRAME_KEYS = [('X', 'x'), ('Y', 'y'), ('Width', 'width'), ('Height', 'height')]


def require(condition, detail):
    if not condition:
        raise RuntimeError(str(detail))


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def wait_until(check, description, seconds=10):
    deadline = time.monotonic() + seconds
    while True:
        result = check()
        if result:
            return result
        require(time.monotonic() < deadline, 'timeout: ' + description)
        time.sleep(0.1)


def document_value(state, pid, wid):
    require(state.get('pid') == pid and state.get('window_id') == wid, 'snapshot identity mismatch')
    fields = [e for e in state.get('elements', []) if e.get('role') == 'AXTextArea']
    require(len(fields) == 1, 'expected one scratch document text area')
    return fields[0].get('value', '')


class Check:
    def __init__(self, args):
        self.args = args
        self.out = args.artifacts.resolve()
        self.out.mkdir(parents=True, exist_ok=False)
        self.owned = {}
        self.restores = []
        self.cases = []
        self.sequence = 0
        self.observer = self.out / 'observer'
        self.report = {'source_sha': args.source_sha, 'pid': args.pid, 'status': 'running'}

    def record(self, name, value):
        self.sequence += 1
        path = self.out / f'{self.sequence:03d}-{name}.json'
        path.write_text(json.dumps(value, indent=2) + '\n')
        return value

    def call(self, tool, args, refusal=None):
        process = subprocess.run(
            [str(BINARY), '--socket', str(self.args.socket), 'call', tool, json.dumps(args)],
            capture_output=True, text=True, timeout=30,
        )
        result = self.record(tool, {'args': args, 'exit_code': process.returncode,
                                   'stdout': process.stdout, 'stderr': process.stderr})
        if refusal is not None:
            require(process.returncode != 0 and refusal in process.stdout + process.stderr, result)
            return result
        require(process.returncode == 0, result)
        return json.loads(process.stdout)

    def observe(self):
        process = subprocess.run(
            [str(self.observer), str(self.args.pid), *map(str, self.owned)],
            capture_output=True, text=True, timeout=5,
        )
        process.check_returncode()
        return self.record('observer', json.loads(process.stdout))

    def window(self, pid, wid):
        rows = [w for w in self.observe()['windows']
                if w['kCGWindowOwnerPID'] == pid and w['kCGWindowNumber'] == wid]
        require(len(rows) == 1, {'pid': pid, 'window_id': wid, 'windows': rows})
        return rows[0]

    def frame(self, pid, wid):
        bounds = self.window(pid, wid)['kCGWindowBounds']
        return dict(pid=pid, window_id=wid, **{a: bounds[k] for k, a in FRAME_KEYS})

    def move(self, name, requested):
        result = self.call('set_window_frame', requested)
        observed = self.frame(requested['pid'], requested['window_id'])
        row = {'case': name, 'requested': requested, 'response': result, 'observed': observed}
        require(result.get('effect') == 'confirmed', row)
        require(all(abs(observed[k] - requested[k]) <= 2 for _, k in FRAME_KEYS), row)
        self.cases.append(row)

    def geometry(self, name, original):
        self.restores.append(original)
        self.move(name + '-move', dict(original, x=original['x'] + 16, y=original['y'] + 16))
        self.move(name + '-resize', dict(original, x=original['x'] + 24, y=original['y'] + 24,
                                        width=original['width'] + 24, height=original['height'] + 24))
        self.move(name + '-restore', original)
        self.restores.pop()

    def launch(self, name, bundle, files=()):
        prior = subprocess.run(['/usr/bin/pgrep', '-x', name], capture_output=True, text=True)
        require(prior.returncode in (0, 1), prior.stderr)
        result = self.call('launch_app', {
            'bundle_id': bundle, 'creates_new_application_instance': True,
            # Argument-domain preference only: never changes persisted defaults.
            'additional_arguments': ['-ApplePersistenceIgnoreState', 'YES'],
            'urls': [str(path) for path in files],
        })
        pid = result['pid']
        require(isinstance(pid, int) and pid > 1 and str(pid) not in prior.stdout.split(), result)
        expected = f'/System/Applications/{name}.app/Contents/MacOS/{name}'
        executable = subprocess.check_output(['/bin/ps', '-p', str(pid), '-o', 'comm='], text=True).strip()
        require(executable == expected, {'pid': pid, 'executable': executable})
        self.owned[pid] = expected
        return pid

    def find_window(self, pid, title):
        def find():
            rows = self.call('list_windows', {'pid': pid})['windows']
            rows = [w for w in rows if w['pid'] == pid and w['title'] == title and w['is_on_screen']]
            require(len(rows) <= 1, rows)
            return rows[0]['window_id'] if rows else None
        return wait_until(find, f'one visible {title!r} window for {pid}')

    def terminate(self, pid):
        if alive(pid):
            executable = subprocess.check_output(['/bin/ps', '-p', str(pid), '-o', 'comm='], text=True).strip()
            require(executable == self.owned[pid], {'cleanup_pid': pid, 'executable': executable})
            os.kill(pid, signal.SIGTERM)
        wait_until(lambda: not alive(pid), f'owned process {pid} exit')

    def refusal_unchanged(self, name, tool, args, error, target):
        before = self.observe()
        bounds = self.frame(*target)
        response = self.call(tool, args, refusal=error)
        after = self.observe()
        require(self.frame(*target) == bounds, name + ' changed target bounds')
        require(before['frontmost_pid'] == after['frontmost_pid'], name + ' changed frontmost process')
        self.cases.append({'case': name, 'response': response, 'bounds': bounds,
                           'frontmost_before': before['frontmost_pid'], 'frontmost_after': after['frontmost_pid']})

    def recipient(self, text, original, distractor, nonce):
        # Global HID route: no pid/window/element, activation, raise, or focus helper.
        self.call('type_text', {'scope': 'desktop', 'text': nonce})
        def read():
            states = [self.call('get_window_state', {'pid': text, 'window_id': wid,
                                                   'include_screenshot': False})
                      for wid in (original, distractor)]
            require(nonce not in document_value(states[1], text, distractor), 'nonce reached the distractor')
            return states if nonce in document_value(states[0], text, original) else None
        return wait_until(read, 'nonce only in original scratch document', seconds=3)

    def front(self, snapshot, pid, wid):
        rows = [w for w in snapshot['visible_windows_front_to_back'] if w['kCGWindowOwnerPID'] == pid
                and w['kCGWindowLayer'] == 0 and w.get('kCGWindowIsOnscreen')]
        return snapshot['frontmost_pid'] == pid and rows and rows[0]['kCGWindowNumber'] == wid

    def run(self):
        subprocess.run(['/usr/bin/swiftc', str(Path(__file__).with_name('product_window_observer.swift')),
                        '-o', str(self.observer)], check=True, capture_output=True, timeout=60)
        configuration = self.call('get_config', {})
        require(configuration['source_sha'] == self.args.source_sha, configuration)
        permissions = self.call('check_permissions', {'prompt': False, 'probe_direct_capture': False})
        source = permissions['source']
        require(permissions['accessibility'] is True and permissions['screen_recording'] is True, permissions)
        require(source['pid'] == self.args.pid and source['attribution'] == 'driver-daemon'
                and source['bundle_id'] == 'com.trycua.driver.local'
                and source['executable'] == str(BINARY), permissions)
        status = subprocess.run([str(BINARY), '--socket', str(self.args.socket), 'status'],
                                capture_output=True, text=True, timeout=10)
        self.record('status', {'exit_code': status.returncode, 'stdout': status.stdout, 'stderr': status.stderr})
        require(status.returncode == 0 and 'permission mode: standard' in status.stdout, status.stdout)
        self.report['permissions'] = permissions
        rows = [w for w in self.observe()['windows'] if w['kCGWindowOwnerPID'] == self.args.pid
                and w['kCGWindowLayer'] == 3 and w.get('kCGWindowIsOnscreen')]
        require(len(rows) == 1, 'expected exactly one live product-owned PiP window')
        own = self.frame(self.args.pid, rows[0]['kCGWindowNumber'])
        self.geometry('own', own)
        token = str(time.time_ns())
        files = [self.out / f'{label}-{token}.txt' for label in ('original', 'distractor')]
        for path in files:
            path.touch(exist_ok=False)
        text = self.launch('TextEdit', 'com.apple.TextEdit', files)
        original, distractor = [self.find_window(text, path.name) for path in files]
        calculator = self.launch('Calculator', 'com.apple.calculator')
        calc_window = self.find_window(calculator, 'Calculator')
        # launch_app's documented detached watchdog suppresses activation for
        # eight seconds after a slow launch, even a legitimate activation.
        time.sleep(9)
        self.geometry('external', self.frame(text, original))
        fixed = self.frame(calculator, calc_window)
        self.restores.append(fixed)
        self.move('fixed-move', dict(fixed, x=fixed['x'] + 16, y=fixed['y'] + 16))
        self.refusal_unchanged('fixed-size', 'set_window_frame',
                               dict(fixed, width=fixed['width'] + 24, height=fixed['height'] + 24),
                               'does not expose a settable AXSize', (calculator, calc_window))
        self.move('fixed-restore', fixed)
        self.restores.pop()
        self.refusal_unchanged('wrong-owner', 'set_window_frame', dict(own, pid=text),
                               f'belongs to pid {self.args.pid}', (self.args.pid, own['window_id']))
        self.refusal_unchanged('public-own-bring', 'bring_to_front',
                               {'pid': self.args.pid, 'window_id': own['window_id']},
                               'own authorization process', (self.args.pid, own['window_id']))
        self.call('bring_to_front', {'pid': text, 'window_id': distractor})
        self.call('bring_to_front', {'pid': text, 'window_id': original})
        wait_until(lambda: self.front(self.observe(), text, original), 'original foreground before menu')
        before = self.recipient(text, original, distractor, 'before_menu_' + token)
        menu = self.call('invoke_menu', {'pid': calculator, 'window_id': calc_window,
                                         'path': ['Window', 'Minimize']})
        def restored():
            observation = self.observe()
            target = [w for w in observation['windows'] if w['kCGWindowNumber'] == calc_window]
            return (not target or not target[0].get('kCGWindowIsOnscreen')) and self.front(observation, text, original)
        wait_until(restored, 'Calculator minimized and original document restored')
        after = self.recipient(text, original, distractor, 'after_menu_' + token)
        self.cases.append({'case': 'external-menu', 'response': menu, 'before': before, 'after': after,
                           'original_window': original, 'distractor_window': distractor,
                           'keyboard_recipient_original_only': True})
        self.terminate(text)
        self.refusal_unchanged('closed-window', 'set_window_frame',
                               dict(own, pid=text, window_id=original),
                               'closed, stale, or unknown to WindowServer', (self.args.pid, own['window_id']))
        require(self.call('get_config', {})['source_sha'] == self.args.source_sha, 'daemon not responsive')

    def execute(self):
        errors = []
        try:
            self.run()
        except BaseException as error:
            # Interrupted runs must never publish a passing partial report.
            errors.append(type(error).__name__ + ': ' + str(error))
        finally:
            for original in self.restores:
                try:
                    self.move('failure-restore', original)
                except Exception as error:
                    errors.append('restore: ' + str(error))
            cleanup = {}
            for pid in self.owned:
                try:
                    self.terminate(pid)
                    cleanup[str(pid)] = 'exited'
                except Exception as error:
                    cleanup[str(pid)] = str(error)
                    errors.append('cleanup: ' + str(error))
            daemon_alive = alive(self.args.pid)
            if not daemon_alive:
                errors.append('supplied daemon exited')
            self.report.update(status='failed' if errors else 'passed', errors=errors,
                               cases=self.cases, owned_process_cleanup=cleanup,
                               daemon_alive=daemon_alive)
            (self.out / 'result.json').write_text(json.dumps(self.report, indent=2) + '\n')
        print(json.dumps({'status': self.report['status'], 'errors': errors, 'artifacts': str(self.out)}))
        return 1 if errors else 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--socket', type=Path, required=True)
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--artifacts', type=Path, required=True, help='new output directory; never overwritten')
    args = parser.parse_args()
    require(sys.platform == 'darwin', 'macOS required')
    require(args.pid > 1 and re.fullmatch(r'[0-9a-f]{40}', args.source_sha), 'exact PID and source SHA required')
    return Check(args).execute()


if __name__ == '__main__':
    sys.exit(main())
