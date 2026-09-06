"""Signer-free v3 geometry fault episode on a disposable native Hyprland VM.

Use the same CLI flags as production_cancel_proof.py. A plan has purpose
geometry_fault, disposable=true, compositor={pid,instance}, foreground,
primary_point, package_versions, and exactly one agents entry: app, name,
target={pid,window_id}, bounds, pointer_stage (select_range or move_rectangle),
drag={}. fault={kind:move|resize,to:[x,y]} gives absolute position or size;
floating resize preserves the center, and this integer-frame proof requires
even size deltas so the expected position does not depend on pixel rounding.
recovery={pointer_stage:click_a1|click_b2|scroll_down} chooses a NEW action.
The prepared synthetic document must already be floating. Run separate move
and resize episodes with freshly observed plans; never replay a failed drag.

Only normal direct Driver calls send application input. Exact compositor IPC
injects/restores geometry, never replaces app input. A pass requires observed
mid-drag mutation, cancellation, own-seat release, continuous foreground
isolation, and a freshly grounded new-runtime recovery. Portable tests are
synthetic orchestration evidence, not native or physical-hardware proof.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import time

from driver_input_live import state, wait_for, wm
from primary_trace import Trace, analyze
from production_cancel_proof import (GROUNDING_DISPATCH_RESERVE_NS, MAX_GROUNDING_ATTEMPTS,
    MAX_GROUNDING_AGE_NS, POINTER_STAGES, PROFILE,
    RECOVERY_STAGES, active_drags, call_drag, close_owned, grounded_snapshot,
    poll_active, prepare_drag, stopped_prefix, verify_recovery_cleanup, verify_recovery_trace)
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
import production_pointer_grounding as pointer_grounding
from production_realapp_proof import (PRIMARY_LIFETIME_MS, app_process_identity,
    check_response, primary_acknowledgement, provenance, require_primary_active, trace_interval)
from realapp_proof import cleanup_all, released_synthetic_input


def validate_plan(plan):
    assert plan['purpose'] == 'geometry_fault' and plan['disposable'] is True
    assert len(plan['agents']) == 1, 'geometry episode owns one synthetic lane'
    spec = plan['agents'][0]
    assert spec['app'] in POINTER_STAGES
    assert spec['pointer_stage'] == POINTER_STAGES[spec['app']] and spec['drag'] == {}
    assert isinstance(spec['name'], str) and spec['name']
    assert spec.get('profile', PROFILE) == PROFILE
    for target in (spec['target'], plan['foreground']):
        assert set(target) == {'pid', 'window_id'}
        assert all(type(v) is int and v > 1 for v in target.values())
    assert spec['target']['pid'] != plan['foreground']['pid']
    bounds = spec['bounds']
    assert set(bounds) == {'x', 'y', 'width', 'height'}
    assert all(type(v) is int for v in bounds.values())
    assert bounds['width'] > 0 and bounds['height'] > 0
    fault = plan['fault']
    assert set(fault) == {'kind', 'to'} and fault['kind'] in ('move', 'resize')
    assert len(fault['to']) == 2 and all(type(v) is int for v in fault['to'])
    keys = ('x', 'y') if fault['kind'] == 'move' else ('width', 'height')
    old = [bounds[key] for key in keys]
    assert old != fault['to'] and all(abs(a - b) <= 128 for a, b in zip(old, fault['to'])), 'need bounded changed geometry'
    assert fault['kind'] != 'resize' or min(fault['to']) > 0
    expected_geometry(bounds, fault)
    assert set(plan['recovery']) == {'pointer_stage'}
    assert plan['recovery']['pointer_stage'] in RECOVERY_STAGES[spec['app']]
    assert len(plan['primary_point']) == 2 and all(type(v) is int for v in plan['primary_point'])
    compositor = plan['compositor']
    assert set(compositor) == {'pid', 'instance'} and type(compositor['pid']) is int and compositor['pid'] > 1
    assert isinstance(compositor['instance'], str) and re.fullmatch(r'[A-Za-z0-9_.-]+', compositor['instance'])


def process_identity(pid):
    root = Path('/proc') / str(pid)
    return {'pid': pid, 'uid': root.stat().st_uid,
            'starttime': (root / 'stat').read_text().rsplit(')', 1)[1].split()[19],
            'exe': str((root / 'exe').resolve(strict=True))}


def window_bounds(window):
    return dict(zip(('x', 'y', 'width', 'height'), [*window['at'], *window['size']]))


def expected_geometry(bounds, fault):
    """Hyprland's floating resize translates the origin by minus half the delta."""
    expected = dict(bounds)
    if fault['kind'] == 'move':
        expected.update(zip(('x', 'y'), fault['to']))
    else:
        assert fault['kind'] == 'resize'
        for position, size, target in zip(('x', 'y'), ('width', 'height'), fault['to']):
            delta = target - bounds[size]
            assert delta % 2 == 0, 'integer-frame resize proof requires even size deltas'
            expected[position] -= delta // 2
            expected[size] = target
    return expected


class GeometryFault:
    """Only the reviewed native window may be mutated or restored; never kill it."""
    def __init__(self, plan):
        assert plan['disposable'] is True and platform.system() == 'Linux'
        assert subprocess.run(['systemd-detect-virt', '--vm', '--quiet'], timeout=2).returncode == 0
        self.spec, self.fault = plan['agents'][0], plan['fault']
        self.instance = plan['compositor']['instance']
        assert os.environ.get('HYPRLAND_INSTANCE_SIGNATURE') == self.instance, 'wrong compositor session'
        self.compositor = process_identity(plan['compositor']['pid'])
        assert self.compositor['uid'] == os.getuid() and Path(self.compositor['exe']).name == 'Hyprland'
        self.owner = process_identity(self.spec['target']['pid'])
        assert self.owner['uid'] == os.getuid(), 'target is not owned by test user'
        app_process_identity(self.spec['app'], self.owner['pid'])
        self.expected = expected_geometry(self.spec['bounds'], self.fault)
        self.mutated = False
        self.record = {'result': 'unproven', 'kind': self.fault['kind'], 'target': self.spec['target'],
                       'before_bounds': self.spec['bounds'], 'expected_bounds': self.expected}
        before = self.snapshot()
        assert before['bounds'] == self.spec['bounds'], 'reviewed geometry is stale'
        self.record['before'] = before

    def hypr(self, *arguments):
        return subprocess.check_output(['hyprctl', '-i', self.instance, *arguments], text=True, timeout=2).strip()

    def snapshot(self):
        assert process_identity(self.compositor['pid']) == self.compositor, 'compositor identity changed'
        assert process_identity(self.owner['pid']) == self.owner, 'target identity changed'
        instances = json.loads(self.hypr('-j', 'instances'))
        assert sum(row.get('instance') == self.instance and row.get('pid') == self.compositor['pid']
                   for row in instances) == 1, 'wrong compositor instance'
        windows = json.loads(self.hypr('-j', 'clients'))
        matches = [w for w in windows if w.get('pid') == self.owner['pid']]
        assert len(matches) == 1, 'target is not one exact window'
        window = matches[0]
        assert re.fullmatch(r'0x[0-9a-fA-F]+', window.get('address', ''))
        assert int(window['address'], 16) == self.spec['target']['window_id'], 'target address changed'
        assert window.get('xwayland') is False and window.get('floating') is True, 'need prepared floating native target'
        assert f'cua-smoke-{self.spec["app"]}' in window.get('title', ''), 'wrong synthetic document'
        return {'observed_ns': time.monotonic_ns(), 'bounds': window_bounds(window), 'window': window}

    def dispatch(self, to):
        assert len(to) == 2 and all(type(v) is int for v in to)
        address = hex(self.spec['target']['window_id'])
        # Current Lua dispatcher explicitly supports absolute x/y plus window.
        command = (f'hl.dsp.window.{self.fault["kind"]}({{ window = "address:{address}", '
                   f'x = {to[0]}, y = {to[1]}, relative = false }})')
        assert self.hypr('dispatch', command) == 'ok', 'geometry dispatcher refused'

    def inject(self, trace, previous, pending, guard):
        before = self.snapshot()
        assert before['bounds'] == self.spec['bounds'], 'target moved before fault gate'
        page, lanes = poll_active(trace, previous, None, [pending])
        gate_ns = time.monotonic_ns()
        lane = next(iter(lanes))
        before = self.snapshot()
        assert before['bounds'] == self.spec['bounds'], 'target changed at fault gate'
        guard()
        assert not pending.done(), 'drag returned before geometry dispatch'
        self.record.update(prefix=page, lane=lane, gate_ns=gate_ns, gate_observation=before)
        requested = time.monotonic_ns()
        assert 0 <= requested - gate_ns <= 250_000_000, 'stale fault gate'
        self.record['requested_ns'] = requested
        self.mutated = True  # A lost IPC reply can still mean the mutation happened.
        self.dispatch(self.fault['to'])
        self.record['acknowledged_ns'] = time.monotonic_ns()
        def changed():
            observation = self.snapshot()
            self.record['last_observation'] = observation
            return observation if observation['bounds'] == self.expected else None
        self.record['after'] = wait_for(changed, timeout=2)
        guard()
        self.record['result'] = 'observed'
        return page, lane

    def restore(self):
        if not self.mutated:
            return {'result': 'not_needed'}
        observed = self.snapshot()
        assert observed['bounds'] in (self.expected, self.spec['bounds']), 'unowned geometry change; restoration refused'
        if observed['bounds'] != self.spec['bounds']:
            keys = ('x', 'y') if self.fault['kind'] == 'move' else ('width', 'height')
            self.dispatch([self.spec['bounds'][key] for key in keys])
        restored = wait_for(lambda: (row if (row := self.snapshot())['bounds'] == self.spec['bounds'] else None), timeout=2)
        self.mutated = False
        return {'result': 'restored', 'before': observed, 'after': restored}


def fault_outcome(action):
    assert action.get('replayed') is False
    if action['outcome'] == 'unknown':
        return {'kind': 'unknown', 'app_effect_verified': False}
    assert action['outcome'] == 'response'
    response = action['response']
    content = response.get('structuredContent', {})
    if (content.get('delivery') or {}).get('mode') == 'unknown':
        kind = 'unknown'
    elif content.get('effect') == 'partial':
        kind = 'partial'
    else:
        # A refusal after an observed press cannot truthfully claim zero delivery.
        reason = (content.get('refusal') or {}).get('code', content.get('reason'))
        check_response(response, {'kind': 'refused', 'reason': reason})
        raise AssertionError('refusal conflicts with trace-observed drag delivery')
    check_response(response, {'kind': kind})
    if kind == 'partial':
        assert content['delivery']['delivered_count'] > 0, 'partial result hid observed delivery'
    return {'kind': kind, 'app_effect_verified': False}


def verify_fault(boundary, record, action):
    assert record['result'] == 'observed' and record['after']['bounds'] == record['expected_bounds']
    assert record['before_bounds'] != record['expected_bounds']
    prefix, lane = record['prefix'], record['lane']
    assert set(active_drags(prefix)) == {lane}
    tail = trace_interval(prefix, boundary)
    assert record['gate_ns'] <= record['requested_ns'] <= record['acknowledged_ns'] <= record['after']['observed_ns']
    assert 0 <= record['requested_ns'] - record['gate_ns'] <= 250_000_000
    assert prefix['events'][-1][1] <= record['requested_ns'], 'trace clock is incompatible'
    synthetic = [row for row in tail if row[5] in (1, 2)]
    assert all(row[5] == lane for row in synthetic), 'cleanup crossed lanes'
    cancelled = [row for row in synthetic if row[2] == 'agent_cancel']
    assert len(cancelled) == 1 and cancelled[0][1] >= record['requested_ns'], 'no post-fault cancellation'
    assert not any(row[2] in ('agent_admitted', 'agent_drag_start', 'agent_drag_end', 'agent_action_end',
                              'keyboard_key', 'pointer_axis') for row in synthetic), 'extra action or false completion'
    releases = [row for row in synthetic if row[2] == 'pointer_button']
    assert len(releases) == 1 and releases[0][6] == 0 and releases[0][0] > cancelled[0][0], 'missing own-seat release'
    assert not any(row[2] in ('pointer_motion', 'pointer_enter') and row[0] > cancelled[0][0]
                   for row in synthetic), 'input continued after cancellation'
    stopped = stopped_prefix(boundary)
    isolation = analyze(stopped)
    assert isolation['result'] == 'passed' and released_synthetic_input(stopped), isolation
    return {'result': 'verified', 'outcome': fault_outcome(action), 'continuous_isolation': isolation,
            'synthetic_cleanup': 'verified', 'saved_document_effect': 'unproven'}


def prepare_recovery(client, spec, stage, guard, save):
    """Refresh an expired successful observation once, before any input.

    Preserve both snapshots and the real observation start. Failed observations
    and unknown actions are never retried; actual dispatch still checks 5 s.
    """
    for attempt in range(1, MAX_GROUNDING_ATTEMPTS + 1):
        guard()
        app_process_identity(spec['app'], spec['target']['pid'])
        before = grounded_snapshot(client, spec['target'], spec)
        prepared_ns = before['proof_observation_started_ns']
        assert type(prepared_ns) is int and prepared_ns > 0, 'invalid recovery observation timestamp'
        arguments, oracle = pointer_grounding.action(before, pointer_grounding.read_pixels(before['proof_image']), spec['app'], stage)
        checked_ns = time.monotonic_ns()
        age = checked_ns - prepared_ns
        assert age >= 0, 'recovery observation is in the future'
        ready = age <= MAX_GROUNDING_AGE_NS - GROUNDING_DISPATCH_RESERVE_NS
        prepared = {'snapshot': before, 'arguments': arguments, 'oracle': oracle, 'prepared_ns': prepared_ns,
                    'attempt': attempt, 'checked_ns': checked_ns, 'grounding_age_ns': age,
                    'dispatch_reserve_ns': GROUNDING_DISPATCH_RESERVE_NS, 'ready': ready, 'input_attempted': False}
        save(f'recovery-grounding-attempt-{attempt}.json', prepared)
        save('recovery-grounding.json', prepared)
        if ready:
            return prepared
    raise AssertionError('recovery grounding expired; no input sent')


def recover(client, observer, victim, spec, stage, trace, boundary, lane, guard, save, result):
    assert victim.process.poll() is not None, 'old runtime must be reaped before recovery'
    assert victim.process.pid not in assert_distinct_runtimes([client, observer]), 'reused old runtime'
    assert stage in RECOVERY_STAGES[spec['app']]
    fresh = {**spec, 'name': spec['name'] + '-recovery', 'pointer_stage': stage}
    result.update(runtime_pid=client.process.pid, previous_runtime_pid=victim.process.pid, replayed=False)
    assert not client.tool('start_session', {'session': fresh['name']}).get('isError')
    prepared = prepare_recovery(client, fresh, stage, guard, save)
    prepared_ns, arguments, oracle = prepared['prepared_ns'], prepared['arguments'], prepared['oracle']
    tool = pointer_grounding.STAGES[spec['app']][stage]
    assert tool in ('click', 'scroll')
    guard()
    dispatch_ns = time.monotonic_ns()
    assert 0 <= dispatch_ns - prepared_ns <= MAX_GROUNDING_AGE_NS, 'recovery grounding expired'
    result['action'] = {'outcome': 'unknown', 'replayed': False, 'dispatch_ns': dispatch_ns}
    try:
        response = client.tool(tool, {**arguments, **spec['target'], 'session': fresh['name'], 'delivery_mode': 'background'})
    except Exception as error:
        result['action']['error'] = str(error)
    else:
        result['action'].update(outcome='response', response=response)
    save('recovery-action.json', result['action'])
    after = grounded_snapshot(observer, spec['target'], fresh, session=False)
    save('recovery-after.json', after)
    assert result['action']['outcome'] == 'response', 'recovery delivery unknown; no replay'
    check_response(response, {'kind': 'dispatched'})
    result['app_effect'] = pointer_grounding.verify(after, pointer_grounding.read_pixels(after['proof_image']), oracle)
    page = trace.collect()
    result['trace'] = verify_recovery_trace(boundary, page, lane, tool)
    guard()
    result['result'] = 'verified'
    return page


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2))
    report = {'result': 'failed', 'scope': 'native-geometry-fault', 'fault': 'unproven',
              'recovery': {'result': 'unproven'}, 'full_desktop_matrix': False, 'physical_hardware': False}
    clients, observer, grab, trace, fault, pool, future = [], None, None, None, None, None, None
    prefix = primary_before = baseline = deadline = None
    def guard():
        require_primary_active(grab, deadline)
    try:
        plan = json.loads(args.plan.read_text())
        save('plan.json', plan)
        validate_plan(plan)
        assert args.trace_socket.name in ('cua-input-v3.sock', 'cua-input-v3-2.sock')
        fault = GeometryFault(plan)
        origin = provenance(args, plan)
        for name in ('production_geometry_fault_proof.py', 'production_geometry_fault_proof_test.py', 'production_cancel_proof.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        save('provenance.json', origin)
        def launch(name):
            directory = args.evidence / name
            directory.mkdir()
            return DirectMCP(args.driver, directory, PROFILE)
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
        trace = Trace(args.trace_socket)
        assert trace.hello['protocol'] == 3
        start_ns = time.monotonic_ns()
        trace.exchange('TRACE_START')
        initial = trace.collect()
        assert start_ns <= initial['events'][0][1] <= time.monotonic_ns(), 'incompatible trace clock'
        assert not active_drags(initial) and not any(r[5] in (1, 2) for r in initial['events'])
        prepared = prepare_drag(clients[0], spec)
        save('drag-grounding.json', prepared)
        guard()
        pool = ThreadPoolExecutor(max_workers=1)
        future = pool.submit(call_drag, clients[0], spec, prepared, guard)
        _, lane = fault.inject(trace, initial, future, guard)
        save('fault.json', fault.record)
        report['action'] = future.result(timeout=5)
        save('drag-action.json', report['action'])
        boundary = trace.collect()
        save('fault-prefix.json', boundary)
        report['fault'] = verify_fault(boundary, fault.record, report['action'])
        changed = {**spec, 'bounds': fault.expected}
        after = grounded_snapshot(observer, spec['target'], changed, session=False)
        save('interrupted-state.json', {'snapshot': after, 'action': report['action'], 'replayed': False,
                                       'saved_document_effect': 'unproven'})
        close_owned(clients[0])
        teardown = trace.collect()
        report['runtime_teardown'] = verify_recovery_cleanup(boundary, stopped_prefix(teardown))
        boundary = teardown  # Seat teardown is included before new admission.
        save('pre-recovery-prefix.json', boundary)
        clients.append(launch('recovery'))
        prefix = recover(clients[-1], observer, clients[0], changed, plan['recovery']['pointer_stage'],
                         trace, boundary, lane, guard, save, report['recovery'])
        save('recovery-prefix.json', prefix)
        guard()
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        operations = [(f'close_agent_{index}', lambda c=client: close_owned(c)) for index, client in enumerate(clients)]
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
            operations.append(('preserve_fault', lambda: save('fault.json', fault.record)))
            operations.append(('restore_geometry', lambda: save('restoration.json', fault.restore())))
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
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('driver', 'plugin', 'source', 'primary-grab', 'plan', 'evidence', 'foreground-journal', 'trace-socket'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--source-sha', required=True)
    raise SystemExit(run(parser.parse_args()))
