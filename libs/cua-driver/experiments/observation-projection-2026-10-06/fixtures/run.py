"""Matched live AX-only driver trial. Never creates a virtual display.

ARC_EVAL_SOURCE=/tmp/arc-cua-eval-20261005 /tmp/arc-cua-eval-venv/bin/python run.py
Input is delivered only through each driver's public MCP tools. State/fault
injection belongs to the synthetic fixture, independently of either driver.
"""
import json
from contextlib import ExitStack
from datetime import datetime, timezone
import os
import platform
import select
import statistics
import subprocess
import sys
import tempfile
import tomllib
import threading
import time
from pathlib import Path

import AppKit
import Quartz

HERE = Path(__file__).resolve().parent


def wait_for(fn, timeout=5):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        value = fn()
        if value:
            return value
        time.sleep(.01)
    raise TimeoutError('independent fixture readiness/effect timeout')


def front():
    return int(AppKit.NSWorkspace.sharedWorkspace().frontmostApplication().processIdentifier())


def cursor():
    p = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
    return (round(p.x, 1), round(p.y, 1))


def sanitize_result(content):
    """Exclude global app menus (which can contain the user's recent documents)."""
    if not isinstance(content, dict):
        return content
    out = {k:v for k,v in content.items() if k not in ('tree_markdown', 'screenshot_base64')}
    if 'elements' in out:
        excluded = set()
        clean = []
        for element in out['elements']:
            if element.get('role') == 'AXMenuBar' or element.get('parent_index') in excluded:
                excluded.add(element.get('element_index'))
            else:
                clean.append(element)
        out['elements'] = clean
    if isinstance(out.get('fresh'), dict):
        out['fresh'] = sanitize_result(out['fresh'])
    return out


class MCP:
    def __init__(self, argv, name):
        self.name = name
        self.calls = []
        self.id = 0
        self.proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=open(f'/tmp/arc-eval-{name}.stderr', 'w'), bufsize=0)
        self.buffer = b''
        try:
            initialized = self.request('initialize', {'protocolVersion': '2025-06-18', 'capabilities': {},
                                                      'clientInfo': {'name': 'oh-arc-eval', 'version': '1'}})
            self.server_info = initialized['serverInfo']
            self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
            self.schemas = {t['name']: t['inputSchema'] for t in self.request('tools/list', {})['tools']}
        except BaseException:
            self.close()
            raise

    def send(self, obj):
        self.proc.stdin.write((json.dumps(obj) + '\n').encode())
        self.proc.stdin.flush()

    def request(self, method, params):
        self.id += 1
        self.send({'jsonrpc': '2.0', 'id': self.id, 'method': method, 'params': params})
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if b'\n' not in self.buffer:
                if not select.select([self.proc.stdout], [], [], max(0, deadline-time.monotonic()))[0]:
                    break
                part = os.read(self.proc.stdout.fileno(), 65536)
                if not part:
                    raise RuntimeError('MCP stream ended')
                self.buffer += part
                continue
            line, self.buffer = self.buffer.split(b'\n', 1)
            obj = json.loads(line)
            if obj.get('id') == self.id:
                if 'error' in obj:
                    raise RuntimeError(str(obj['error']))
                return obj['result']
        raise TimeoutError('MCP request timed out; no input retry')

    def call(self, name, **args):
        assert name in self.schemas
        before_front, before_cursor = front(), cursor()
        started = time.perf_counter()
        result = self.request('tools/call', {'name': name, 'arguments': args})
        ms = (time.perf_counter()-started)*1000
        content = result.get('structuredContent')
        if content is None:
            texts = [p.get('text', '') for p in result.get('content', []) if p.get('type') == 'text']
            try:
                content = json.loads('\n'.join(texts))
            except ValueError:
                content = {'text': '\n'.join(texts)}
        self.calls.append({'tool': name, 'arguments': args, 'ms': ms,
                           'error': bool(result.get('isError')), 'result': sanitize_result(content),
                           'front_changed': front() != before_front,
                           'cursor_changed': cursor() != before_cursor,
                           'physical_mouse_recent': min(Quartz.CGEventSourceSecondsSinceLastEventType(
                               Quartz.kCGEventSourceStateHIDSystemState, kind) for kind in
                               (Quartz.kCGEventMouseMoved, Quartz.kCGEventLeftMouseDown,
                                Quartz.kCGEventScrollWheel)) <= ms/1000 + .05})
        return content, bool(result.get('isError'))

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.terminate()
            self.proc.wait(timeout=5)


class Fixture:
    def __init__(self, start='shown'):
        self.temp = tempfile.TemporaryDirectory(prefix='oh-arc-fixture-')
        self.path = Path(self.temp.name) / 'state.json'
        self.log = open(Path(self.temp.name) / 'stderr', 'w')
        self.proc = subprocess.Popen([sys.executable, str(HERE/'fixture.py'), str(self.path), start],
                                     stdout=subprocess.DEVNULL, stderr=self.log)
        try:
            wait_for(lambda: self.path.exists(), 8)
            if start != 'shown':
                wait_for(lambda: self.state().get(start))
        except BaseException:
            state = self.state()
            self.close()
            raise RuntimeError(f'fixture setup failed: {state}')
        self.pid = self.proc.pid
        self.command = 0

    def state(self):
        try:
            return json.loads(self.path.read_text())
        except (OSError, ValueError):
            return {}

    def mutate(self, action):
        self.command += 1
        self.path.with_suffix('.command').write_text(json.dumps({'action': action, 'id': self.command}))
        wait_for(lambda: self.state().get('applied') == self.command)
        # Give structural notifications time to arrive; no driver observation here.
        time.sleep(.1)

    def close(self):
        self.proc.terminate()
        self.proc.wait(timeout=5)
        self.log.close()
        self.temp.cleanup()


def windows(pid):
    options = Quartz.kCGWindowListOptionAll | Quartz.kCGWindowListExcludeDesktopElements
    found = Quartz.CGWindowListCopyWindowInfo(options, Quartz.kCGNullWindowID) or []
    return {str(w.get('kCGWindowName', '')): int(w['kCGWindowNumber']) for w in found
            if int(w.get('kCGWindowOwnerPID', 0)) == pid and int(w.get('kCGWindowLayer', 0)) == 0}


def observe(client, fixture, window):
    args = {'pid': fixture.pid, 'window_id': window}
    if client.name == 'arc':
        result, error = client.call('observe', **args)
    else:
        result, error = client.call('get_window_state', **args, include_screenshot=False)
    if error:
        raise RuntimeError(str(result))
    return result


def find(client, snap, label):
    candidates = [e for e in snap.get('elements', [])
                  if e.get('name', e.get('label')) == label and
                  e.get('role', '').replace('AX', '') not in ('StaticText', 'Group')]
    if len(candidates) != 1:
        raise ValueError(f'exact target {label!r}: found {len(candidates)}')
    return candidates[0]


def act(client, fixture, window, snap, element, action='CLICK', value=None):
    if client.name == 'arc':
        assert action in element.get('actions', []), 'action not offered by observed element'
        args = {'snapshot': snap['snapshot'], 'element': element['id'], 'action': action, 'settle': True}
        if value is not None:
            args['value'] = value
        return client.call('act', **args)
    args = {'pid': fixture.pid, 'window_id': window, 'element_token': element['element_token']}
    if action == 'SET_VALUE':
        return client.call('set_value', **args, value=str(value))
    return client.call('click', **args)


def scenario(client, case, rep):
    start = case if case in ('hidden', 'minimized') else 'shown'
    try:
        f = Fixture(start)
    except Exception as exc:
        return {'driver':client.name, 'case':case, 'rep':rep, 'passed':False,
                'setup_failed':True, 'error':str(exc), 'tool_calls':0, 'tool_ms':0, 'calls':[]}
    offset = len(client.calls)
    watched = {'fixture_active': False, 'samples': 0}
    stop = threading.Event()
    def monitor():
        while not stop.wait(.005):
            watched['samples'] += 1
            watched['fixture_active'] |= bool(f.state().get('active'))
    watcher = threading.Thread(target=monitor, daemon=True)
    watcher.start()
    result = {'driver': client.name, 'case': case, 'rep': rep}
    try:
        win = wait_for(lambda: windows(f.pid).get('Arc Bench Form'))
        t = time.perf_counter()
        snap = observe(client, f, win)
        if case == 'form':
            for label, action, value in [('Full name', 'SET_VALUE', 'Synthetic Person'),
                                         ('Email', 'SET_VALUE', 'synthetic@example.invalid'),
                                         ('Subscribe', 'CLICK', None), ('Submit', 'CLICK', None)]:
                response, error = act(client, f, win, snap, find(client, snap, label), action, value)
                if error or (client.name == 'arc' and response.get('status') != 'done'):
                    raise RuntimeError(str(response))
                snap = response.get('fresh') if client.name == 'arc' else observe(client, f, win)
            state = wait_for(lambda: f.state() if f.state().get('submitted') == 1 else None)
            result['passed'] = all(state.get(k) == v for k,v in {
                'name':'Synthetic Person', 'email':'synthetic@example.invalid',
                'subscribe':True, 'submitted':1, 'submitted_record':'Record A'}.items())
        elif case == 'popup':
            response, error = act(client, f, win, snap, find(client, snap, 'Plan'))
            if error:
                raise RuntimeError(str(response))
            next_snap = response.get('fresh') if client.name == 'arc' else observe(client, f, win)
            response, error = act(client, f, win, next_snap, find(client, next_snap, 'Team'))
            if client.name != 'arc':
                observe(client, f, win)
            state = wait_for(lambda: f.state() if f.state().get('plan') == 'Team' else None, 3)
            result['passed'] = state.get('plan') == 'Team'
            result['action_error'] = error
        elif case == 'menu':
            if client.name == 'arc':
                commands, error = client.call('commands', pid=f.pid)
                paths = [c['path'] for c in commands['commands'] if c['path'] == 'Bench > Increment']
                assert len(paths) == 1
                response, error = client.call('run_command', pid=f.pid, path=paths[0], settle=True)
            else:
                response, error = client.call('invoke_menu', pid=f.pid, window_id=win, path=['Bench','Increment'])
                observe(client, f, win)
            effect_started = time.perf_counter()
            state = wait_for(lambda: f.state() if f.state().get('counter') == 1 else None, 3)
            result['oracle_wait_ms'] = (time.perf_counter()-effect_started)*1000
            result['passed'] = state.get('counter') == 1
            result['action_error'] = error
        elif case in ('hidden', 'minimized'):
            response, error = act(client, f, win, snap, find(client, snap, 'Subscribe'))
            if client.name != 'arc':
                observe(client, f, win)
            result['passed'] = f.state().get('subscribe') is True and f.state().get(case) is True
            result['action_error'] = error
            result['parked'] = response.get('parked', False)
        elif case == 'second_window':
            f.mutate('second')
            snap = observe(client, f, win)
            response, error = act(client, f, win, snap, find(client, snap, 'Subscribe'))
            if client.name != 'arc':
                observe(client, f, win)
            state = f.state()
            result['passed'] = state.get('subscribe') is True and state.get('agree') is False
        elif case == 'delayed_sheet':
            response, error = act(client, f, win, snap, find(client, snap, 'Open 800 ms'))
            next_snap = response.get('fresh') if client.name == 'arc' else observe(client, f, win)
            result['sheet_at_return'] = f.state().get('dialog') is True
            wait_for(lambda: f.state().get('dialog'))
            cached = find(client, next_snap, 'Submit')
            refused, action_error = act(client, f, win, next_snap, cached)
            result['refused'] = action_error or refused.get('status') in ('changed', 'stale')
            result['passed'] = result['refused'] and f.state().get('submitted') == 0
        else:
            cached = find(client, snap, 'Submit')
            f.mutate(case)
            response, error = act(client, f, win, snap, cached)
            time.sleep(.05)
            result['refused'] = error or response.get('status') in ('changed', 'stale')
            result['passed'] = result['refused'] and f.state().get('submitted') == 0
        result['wall_ms'] = (time.perf_counter()-t)*1000
        result['state'] = f.state()
    except Exception as exc:
        result.update(passed=False, error=f'{type(exc).__name__}: {str(exc)[:350]}', state=f.state())
    finally:
        stop.set()
        watcher.join(timeout=1)
        result['background_monitor'] = watched
        result['calls'] = client.calls[offset:]
        result['tool_calls'] = len(result['calls'])
        result['tool_ms'] = sum(c['ms'] for c in result['calls'])
        if client.name == 'arc':
            client.call('release', pid=f.pid)
        f.close()
    return result


def candidate_versions():
    """Refuse stale candidates, then retain exact provenance for each new run."""
    native_check = json.loads(subprocess.check_output(
        ['cua-driver', 'check-update', '--json', '--no-cache'], text=True, timeout=30))
    if (native_check.get('error') or native_check.get('update_available') or not native_check.get('latest_version')
            or native_check.get('current_version') != native_check.get('latest_version')):
        raise RuntimeError('Update Cua Driver before comparing: cua-driver update --apply')
    source = Path(os.environ['ARC_EVAL_SOURCE']).resolve()
    commit = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    latest_arc = subprocess.check_output(
        ['gh', 'api', 'repos/shhivv/arc-cua/commits/master', '--jq', '.sha'], text=True, timeout=30).strip()
    arc_checked_at = datetime.now(timezone.utc).isoformat()
    if commit != latest_arc:
        raise RuntimeError('Update the arc checkout and its installed environment to upstream head before comparing')
    subprocess.run(['git', '-C', str(source), 'diff', '--exit-code', 'HEAD', '--', 'src', 'pyproject.toml'],
                   check=True, capture_output=True)
    import arc_cua
    if not Path(arc_cua.__file__).resolve().is_relative_to(source):
        raise RuntimeError('The active Python environment does not use the checked arc source')
    arc_version = tomllib.loads((source / 'pyproject.toml').read_text())['project']['version']
    return {'arc_commit': commit, 'arc_version': arc_version,
            'native_version': native_check['current_version'],
            'native_latest_version': native_check['latest_version'],
            'native_release': native_check['release_notes_url'] or
                f"https://github.com/trycua/cua/releases/tag/cua-driver-rs-v{native_check['current_version']}",
            'arc_upstream_checked_at': arc_checked_at,
            'candidates_checked_at': native_check['checked_at'],
            'macos': platform.mac_ver()[0], 'python': platform.python_version()}


def verify_servers(versions, arc, native):
    if native.server_info['version'] != versions['native_version']:
        raise RuntimeError('Running Cua daemon differs from the current CLI; restart it before comparing')
    if arc.server_info['version'] != versions['arc_version']:
        raise RuntimeError('Running arc MCP server differs from the checked source')
    versions['servers'] = {'arc': arc.server_info, 'native': native.server_info}


def main():
    reps = int(os.environ.get('ARC_EVAL_REPS', '3'))
    cases = os.environ.get('ARC_EVAL_CASES', 'form,popup,menu,second_window,label,record,disable,sheet,delayed_sheet,hidden,minimized').split(',')
    versions = candidate_versions()
    with ExitStack() as owned:
        arc = MCP([sys.executable, '-m', 'arc_cua', 'mcp'], 'arc')
        owned.callback(arc.close)
        native = MCP(['cua-driver', 'mcp', '--socket', str(Path.home()/'Library/Caches/cua-driver/cua-driver.sock')], 'native')
        owned.callback(native.close)
        verify_servers(versions, arc, native)
        output = {**versions, 'scope':'live AX-only synthetic AppKit, scripted selections, MCP stdio', 'results':[]}
        result_path = HERE / os.environ.get('ARC_EVAL_RESULT_FILE', 'results.json')
        result_path.parent.mkdir(parents=True, exist_ok=True)
        (result_path.parent/'schemas.json').write_text(json.dumps({'arc':{k:v for k,v in arc.schemas.items() if k in
            ('observe','act','commands','run_command','release')}, 'native':{k:v for k,v in native.schemas.items()
            if k in ('get_window_state','click','set_value','invoke_menu')}}, indent=2))
        for rep in range(reps):
            for case in cases:
                for client in ([arc,native] if rep % 2 == 0 else [native,arc]):
                    result = scenario(client, case, rep)
                    output['results'].append(result)
                    result_path.write_text(json.dumps(output, indent=2))
                    print(json.dumps({k:result.get(k) for k in ('driver','case','rep','passed','tool_calls','tool_ms','error','refused','sheet_at_return','parked')}), flush=True)
    for case in cases:
        for name in ('arc','native'):
            rows = [r for r in output['results'] if r['case'] == case and r['driver'] == name]
            print(case, name, sum(r['passed'] for r in rows), '/',len(rows), 'median tool ms', round(statistics.median(r['tool_ms'] for r in rows),1))


if __name__ == '__main__':
    main()
