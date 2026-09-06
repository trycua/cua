"""Prepare one native v3 initial-primary-conflict/refusal and fresh recovery cell.

CLI: --plan --evidence --driver --plugin --source --source-sha --trace-socket
--primary-grab --foreground-journal, as for production_geometry_fault_proof.py.
Run with python3.11 on the exact prepared disposable VM. No app launch, config
edit, signing, policy installation, plugin input, or automatic action replay.

Integration boundary: use the geometry plan's one-agent shape (app, name,
target, bounds, pointer_stage=select_range|move_rectangle, drag={}), foreground,
primary_point, recovery and package_versions. Set purpose=primary_conflict,
case=initial_refusal. Include vm={machine_id,boot_id}, compositor={pid,instance,
uid,starttime,exe}, and processes={target,foreground}, each exact /proc identity
{pid,uid,starttime,exe}. The controller must leave the synthetic target as the
PRIMARY KEYBOARD client before launch, with no held foreground fixture input.
The runner first attempts one freshly image-grounded drag through DirectMCP.
It requires primary_target_busy, zero synthetic events, and idle lane status.

After that runtime is reaped and its strict trace stopped, the independent
primary-grab fixture deliberately moves to/holds the separate foreground client.
Only then does a distinct runtime attempt a NEW freshly grounded click/scroll.
The predeclared oracle is strict unchanged primary state within EACH action
phase, including runtime cleanup. No primary events are filtered. Setup between
phases has no agent runtime/action and is recorded as an intentional transition,
NOT unchanged isolation. Active-drag primary-transition cancellation and
continuous isolation across setup are explicitly UNPROVEN by this narrow cell.
Portable tests are preparation only; native execution is a separate gate.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import socket
import stat
import struct
import subprocess
import time

from desktop_faults import _identity, _same_compositor, _hypr
from driver_input_live import state, wait_for
from primary_trace import Trace, analyze
from production_cancel_proof import (MAX_GROUNDING_AGE_NS, PROFILE, close_owned,
    grounded_snapshot, prepare_drag, verify_fresh_observation, verify_recovery_cleanup, verify_recovery_trace)
from production_desktop_fault_proof import guest_identity, verify_status
from production_geometry_fault_proof import validate_plan as geometry_plan, window_bounds
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
import production_pointer_grounding as pointer_grounding
from production_realapp_proof import (PRIMARY_LIFETIME_MS, app_process_identity,
    assert_no_dispatch, capacity_lane, check_response, primary_acknowledgement,
    provenance, require_primary_active, trace_interval)
from realapp_proof import cleanup_all, released_synthetic_input


def validate_identity(identity):
    assert set(identity) == {'pid', 'uid', 'starttime', 'exe'}
    assert type(identity['pid']) is int and identity['pid'] > 1
    assert type(identity['uid']) is int and identity['uid'] >= 0
    assert re.fullmatch(r'[0-9]+', identity['starttime'])
    assert Path(identity['exe']).is_absolute()


def validate_plan(plan):
    assert plan['purpose'] == 'primary_conflict' and plan['case'] == 'initial_refusal'
    assert 'fault' not in plan, 'active faults are not covered by this runner'
    bounds = plan['agents'][0]['bounds']
    geometry_plan({**plan, 'purpose': 'geometry_fault',
        'compositor': {key: plan['compositor'][key] for key in ('pid', 'instance')},
        'fault': {'kind': 'move', 'to': [bounds['x'] + 1, bounds['y']]}})
    assert set(plan['vm']) == {'machine_id', 'boot_id'}
    assert re.fullmatch(r'[0-9a-f]{32}', plan['vm']['machine_id'])
    assert re.fullmatch(r'[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}', plan['vm']['boot_id'])
    assert set(plan['compositor']) == {'pid', 'instance', 'uid', 'starttime', 'exe'}
    validate_identity({k: v for k, v in plan['compositor'].items() if k != 'instance'})
    assert Path(plan['compositor']['exe']).name == 'Hyprland'
    assert set(plan['processes']) == {'target', 'foreground'}
    targets = {'target': plan['agents'][0]['target'], 'foreground': plan['foreground']}
    assert len({target['window_id'] for target in targets.values()}) == 2
    for name, identity in plan['processes'].items():
        validate_identity(identity)
        assert identity['pid'] == targets[name]['pid']
        assert identity['uid'] == plan['compositor']['uid']
    assert len({plan['compositor']['pid'], *(p['pid'] for p in plan['processes'].values())}) == 3


def clear_status(status, *, unreserved=False):
    """Trace balance alone cannot exclude state held before tracing started."""
    verify_status(status, True)
    for row in status['input']['lanes']:
        assert all(type(row.get(key)) is int and row[key] == 0 for key in ('held_button', 'held_keys'))
        assert all(row.get(key) is False for key in ('drag_active', 'lease_active', 'keyboard_focus'))
        if unreserved:
            assert row.get('reserved') is False and row.get('pointer_focus') is False
    return status


class ExactDesktop:
    """Read-only attestation; primary input belongs solely to the test fixture."""
    def __init__(self, plan):
        self.plan = plan
        self.instance = plan['compositor']['instance']
        self.compositor = {k: v for k, v in plan['compositor'].items() if k != 'instance'}
        self.guard()
        assert subprocess.run(['systemd-detect-virt', '--vm', '--quiet'], timeout=2).returncode == 0

    def guard(self):
        assert platform.system() == 'Linux' and guest_identity() == self.plan['vm'], 'wrong VM boot'
        assert os.environ.get('HYPRLAND_INSTANCE_SIGNATURE') == self.instance, 'wrong compositor session'
        assert self.compositor['uid'] == os.getuid(), 'wrong desktop owner'
        _same_compositor(self.compositor, self.instance)
        for identity in self.plan['processes'].values():
            assert _identity(identity['pid']) == identity, 'target/foreground process changed'
        windows = json.loads(_hypr(self.instance, '-j', 'clients'))
        spec = self.plan['agents'][0]
        for name, target in (('target', spec['target']), ('foreground', self.plan['foreground'])):
            matches = [w for w in windows if w.get('pid') == target['pid']]
            assert len(matches) == 1, 'ambiguous client'
            window = matches[0]
            assert int(window['address'], 16) == target['window_id'] and window.get('xwayland') is False
            if name == 'target':
                assert f'cua-smoke-{spec["app"]}' in window.get('title', '')
                assert window_bounds(window) == spec['bounds'], 'target geometry changed'
        return windows

    def primary(self, target):
        self.guard()
        active = json.loads(_hypr(self.instance, '-j', 'activewindow'))
        assert active.get('pid') == target['pid'] and int(active.get('address', '0'), 16) == target['window_id'], \
            'wrong primary client'
        return {'pid': active['pid'], 'window_id': int(active['address'], 16),
                'cursor': json.loads(_hypr(self.instance, '-j', 'cursorpos')),
                'workspace': json.loads(_hypr(self.instance, '-j', 'activeworkspace'))['id']}

    def status(self, *, unreserved=False):
        self.guard()
        return clear_status(json.loads(_hypr(self.instance, '-j', 'cua:status')), unreserved=unreserved)

    def trace(self, path):
        self.guard()
        assert path.name in ('cua-input-v3.sock', 'cua-input-v3-2.sock')
        expected = Path(os.environ['XDG_RUNTIME_DIR']) / 'hypr' / self.instance / path.name
        assert path.is_absolute() and path == expected and path.resolve(strict=True) == path
        info = path.lstat()
        assert stat.S_ISSOCK(info.st_mode) and info.st_uid == self.compositor['uid']
        trace = Trace(path)
        try:
            peer = struct.unpack('3i', trace.socket.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            assert peer[:2] == (self.compositor['pid'], self.compositor['uid']), 'wrong trace peer'
            assert trace.hello['protocol'] == 3
            self.guard()
            assert trace.collect().get('active') is False, 'another proof owns the trace'
            return trace
        except BaseException:
            trace.close()
            raise


def verify_refusal(before, after, response):
    check_response(response, {'kind': 'refused', 'reason': 'primary_target_busy'})
    assert_no_dispatch(before, after)
    # Include the initial prefix too, not just the call delta.
    assert not any(row[5] in (1, 2) for row in after['events']), 'synthetic input before refusal'


def action(actor, observer, spec, phase, trace, guard, save, record):
    """One normal Driver input call; always retain its raw after-observation."""
    assert phase in ('refusal', 'recovery')
    prepared = prepare_drag(actor, spec)  # Also grounds click/scroll pointer stages.
    tool = pointer_grounding.STAGES[spec['app']][spec['pointer_stage']]
    assert tool == 'drag' if phase == 'refusal' else tool in ('click', 'scroll')
    arguments = {**prepared['arguments'], **spec['target'], 'session': spec['name'], 'delivery_mode': 'background'}
    record.update(runtime_pid=actor.process.pid, grounding=prepared, tool=tool, arguments=arguments,
                  outcome='not_attempted', replayed=False)
    save(phase + '-action.json', record)
    record['trace_before'] = trace.collect()
    trace_interval(record.get('initial_trace', record['trace_before']), record['trace_before'])
    assert not any(row[5] in (1, 2) for row in record['trace_before']['events']), \
        'synthetic activity before the single planned action'
    guard()
    assert_distinct_runtimes([actor, observer])
    record['dispatch_ns'] = time.monotonic_ns()
    assert 0 <= record['dispatch_ns'] - prepared['prepared_ns'] <= MAX_GROUNDING_AGE_NS, 'stale grounding'
    record['outcome'] = 'unknown'
    try:
        record['response'] = actor.tool(tool, arguments)
        record['outcome'] = 'response'
    except Exception as error:
        record['error'] = str(error)
    finally:
        record['observed_ns'] = time.monotonic_ns()
        save(phase + '-action.json', record)
        try:
            record['after'] = grounded_snapshot(observer, spec['target'], spec, session=False)
        except Exception as error:
            record['observation_error'] = str(error)
            raise
        finally:
            # A failed screenshot must not discard independent dispatch evidence.
            try:
                record['trace_after'] = trace.collect()
            finally:
                save(phase + '-action.json', record)
    guard()
    assert_distinct_runtimes([actor, observer])
    assert record['dispatch_ns'] <= record['observed_ns'], 'action returned before dispatch'
    verify_fresh_observation(prepared['snapshot'], record['after'], observer, after_ns=record['observed_ns'])
    assert record['outcome'] == 'response', 'delivery unknown; never replay'
    if phase == 'refusal':
        verify_refusal(record['trace_before'], record['trace_after'], record['response'])
    else:
        check_response(record['response'], {'kind': 'dispatched'})
        record['app_effect'] = pointer_grounding.verify(record['after'],
            pointer_grounding.read_pixels(record['after']['proof_image']), prepared['oracle'])
        lane = capacity_lane(record['trace_before'], record['trace_after'], tool)
        record['trace_verification'] = verify_recovery_trace(record['trace_before'], record['trace_after'], lane, tool)
    save(phase + '-action.json', record)


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2) + '\n')
    report = {'result': 'failed', 'scope': 'initial-primary-client-refusal-and-new-action-recovery',
              'active_drag_cancellation': 'unproven', 'continuous_isolation_across_setup': 'unproven',
              'full_desktop_matrix': False, 'physical_hardware': False, 'phases': {}}
    clients, observer, desktop, trace, grab = [], None, None, None, None
    tracing, phase, primary, baseline, deadline = False, None, None, None, None
    def guard():
        wanted = spec['target'] if phase == 'refusal' else plan['foreground']
        assert desktop.primary(wanted) == primary, 'primary cursor/focus/workspace changed'
        current = state(args.foreground_journal)
        assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held')), 'foreground input changed'
        if phase == 'recovery':
            require_primary_active(grab, deadline)
    def finish_trace():
        nonlocal tracing
        trace.exchange('TRACE_STOP')
        tracing = False
        stopped = trace.collect()
        save(phase + '-trace.json', stopped)
        checked = analyze(stopped)
        assert checked['result'] == 'passed' and released_synthetic_input(stopped), checked
        assert [row[2] for row in stopped['events'] if row[2] in ('start', 'stop')] == ['start', 'stop'], \
            'trace restarted within an action phase'
        boundary = report['phases'][phase]['trace_after']
        assert stopped['events'][:boundary['count']] == boundary['events'], 'final trace history changed'
        if phase == 'refusal':
            assert not any(row[5] in (1, 2) for row in stopped['events']), 'refusal dispatched or teardown sent input'
        else:
            checked = verify_recovery_cleanup(report['phases'][phase]['trace_after'], stopped)
        guard()
        report['phases'][phase]['final_status'] = desktop.status(unreserved=True)
        report['phases'][phase]['isolation'] = checked
    def launch(name):
        directory = args.evidence / name
        directory.mkdir()
        value = DirectMCP(args.driver, directory, PROFILE)
        clients.append(value)
        return value
    try:
        plan = json.loads(args.plan.read_text())
        save('plan.json', plan)
        validate_plan(plan)
        spec = plan['agents'][0]
        desktop = ExactDesktop(plan)
        app_process_identity(spec['app'], spec['target']['pid'])
        origin = provenance(args, plan)
        for name in (Path(__file__).name, 'production_primary_conflict_proof_test.py',
                     'desktop_faults.py', 'production_cancel_proof.py',
                     'production_desktop_fault_proof.py', 'production_geometry_fault_proof.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        origin['ownership'] = {'vm': plan['vm'], 'compositor': plan['compositor'], 'processes': plan['processes']}
        save('provenance.json', origin)
        observer = launch('observer')
        trace = desktop.trace(args.trace_socket)
        for phase in ('refusal', 'recovery'):
            row = report['phases'][phase] = {}
            row['initial_status'] = desktop.status(unreserved=True)
            if phase == 'recovery':
                assert refused.process.poll() is not None, 'refused runtime is still active'
                assert not tracing
                transition = report['setup_transition'] = {'purpose': 'independent-primary-fixture-setup',
                    'agent_runtime_reaped': refused.process.pid, 'active_agent_actions': 0,
                    'continuous_isolation': 'unproven', 'before': desktop.primary(spec['target'])}
                foreground = grounded_snapshot(observer, plan['foreground'])['window_bounds']
                result = observer.tool('get_desktop_state', {})
                assert not result.get('isError')
                screen = result['structuredContent']
                x, y = plan['primary_point']
                assert 0 < x < foreground['width'] and 0 < y < foreground['height']
                x, y = foreground['x'] + x, foreground['y'] + y
                assert 0 <= x < screen['screen_width'] and 0 <= y < screen['screen_height']
                transition.update(expected_target=plan['foreground'], expected_cursor={'x': x, 'y': y},
                                  requested_ns=time.monotonic_ns())
                save('setup-transition.json', transition)
                desktop.guard()
                deadline = time.monotonic_ns() + PRIMARY_LIFETIME_MS * 1_000_000
                grab = subprocess.Popen([str(args.primary_grab), str(x), str(y), str(screen['screen_width']),
                    str(screen['screen_height']), str(PRIMARY_LIFETIME_MS)], stdout=subprocess.PIPE, text=True)
                assert primary_acknowledgement(grab.stdout) == 'HELD\n'
                wait_for(lambda: state(args.foreground_journal)['held'], timeout=3)
                transition['after'] = desktop.primary(plan['foreground'])
                transition['observed_ns'] = time.monotonic_ns()
                save('setup-transition.json', transition)
                assert transition['after']['cursor'] == transition['expected_cursor'], 'primary fixture missed planned point'
                require_primary_active(grab, deadline)
                row['post_setup_status'] = desktop.status(unreserved=True)
            wanted = spec['target'] if phase == 'refusal' else plan['foreground']
            primary, baseline = desktop.primary(wanted), state(args.foreground_journal)
            assert baseline['held'] is (phase == 'recovery')
            row.update(primary_before=primary, foreground_before=baseline)
            started_ns = time.monotonic_ns()
            # A lost acknowledgement can still mean tracing started; retain
            # the cleanup obligation before issuing the test-only command.
            tracing = True
            trace.exchange('TRACE_START')
            initial = trace.collect()
            row['initial_trace'] = initial
            trace_interval(initial, initial)
            assert initial['count'] == 1 and started_ns <= initial['events'][0][1] <= time.monotonic_ns()
            actor = launch(phase)
            assert_distinct_runtimes([actor, observer])
            if phase == 'refusal':
                refused = actor
                current_spec = spec
            else:
                assert actor.process.pid != refused.process.pid, 'reused refused runtime'
                current_spec = {**spec, 'name': spec['name'] + '-recovery',
                                'pointer_stage': plan['recovery']['pointer_stage']}
            assert not actor.tool('start_session', {'session': current_spec['name']}).get('isError')
            action(actor, observer, current_spec, phase, trace, guard, save, row)
            row['post_action_status'] = desktop.status(unreserved=phase == 'refusal')
            close_owned(actor)
            wait_for(lambda: desktop.status(unreserved=True), timeout=2)
            finish_trace()
            row['result'] = 'verified'
            save(phase + '-action.json', row)
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = {'type': type(error).__name__, 'message': str(error)}
    finally:
        operations = [(f'close_runtime_{i}', lambda c=c: close_owned(c)) for i, c in enumerate(clients) if c is not observer]
        if trace:
            if tracing:
                operations.append(('finish_trace', finish_trace))
            operations.append(('close_trace', trace.close))
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
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('driver', 'plugin', 'source', 'primary-grab', 'plan', 'evidence', 'foreground-journal', 'trace-socket'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--source-sha', required=True)
    raise SystemExit(run(parser.parse_args()))
