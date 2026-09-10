"""Opt-in production idle-peer regression in a prepared disposable Hyprland desktop.

Two new, grounded Calc clicks share one DirectMCP runtime and named session.
Between them only read-only compositor status is polled, for at most 85 seconds.
Neither a primary grab nor a trace spans the real 60-second input-peer expiry.
No transport reset, test input packet, timeout override, or action retry is used.
"""
import argparse
from production_app_smoke import add_provenance_arguments
import hashlib
import json
import math
from pathlib import Path
import subprocess
import time

from driver_input_live import state, wait_for, wm
from primary_trace import Trace
from production_cancel_proof import (MAX_GROUNDING_AGE_NS, PROFILE, close_owned,
                                     grounded_snapshot, verify_recovery_cleanup,
                                     verify_recovery_trace)
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
import production_pointer_grounding as grounding
from production_realapp_proof import (PRIMARY_LIFETIME_MS, app_process_identity,
                                     capacity_lane, check_response, primary_acknowledgement,
                                     provenance, read_input_status, require_primary_active)
from realapp_proof import cleanup_all


IDLE_SECONDS = 60
EXPIRY_DEADLINE_SECONDS = 85
# read_input_status uses a five-second subprocess timeout. Reserve its entire
# budget before starting another read, so a stalled hyprctl cannot extend 85s.
STATUS_READ_BUDGET_NS = 5_000_000_000
STAGES = ('click_b2', 'click_a1')


def validate_plan(plan):
    assert plan['purpose'] == 'idle_reconnect'
    assert len(plan['agents']) == 1, 'one persistent input runtime is required'
    spec = plan['agents'][0]
    assert spec['app'] == 'calc' and spec.get('profile', PROFILE) == PROFILE
    assert isinstance(spec['name'], str) and spec['name']
    assert Path(spec['document']).name == 'cua-smoke-calc.ods', 'only the synthetic document is allowed'
    assert set(spec['bounds']) == {'x', 'y', 'width', 'height'}
    assert all(type(v) in (int, float) and math.isfinite(v) for v in spec['bounds'].values())
    assert spec['bounds']['width'] > 0 and spec['bounds']['height'] > 0
    for target in (spec['target'], plan['foreground']):
        assert set(target) == {'pid', 'window_id'}
        assert all(type(v) is int and v > 0 for v in target.values())
    assert spec['target']['pid'] != plan['foreground']['pid']
    assert len(plan['primary_point']) == 2 and all(type(v) is int for v in plan['primary_point'])
    assert not any(key in plan for key in ('phases', 'recovery', 'idle_timeout', 'moving_primary'))


def process_birth(pid, proc_root=Path('/proc')):
    # comm may contain spaces or parentheses; field 22 follows the final ')'.
    return int((proc_root / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()[19])


def calc_identity(spec):
    pid = spec['target']['pid']
    identity = app_process_identity('calc', pid)
    document = Path(spec['document']).resolve(strict=True)
    assert str(document).encode() in Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0'), \
        'Calc process is not bound to the synthetic document'
    return {key: identity[key] for key in ('pid', 'executable', 'sha256')} | {
        'start_ticks': process_birth(pid), 'document': str(document)}


def require_runtime(client, runtime):
    assert client.process is runtime['process'] and client.process.pid == runtime['pid'], 'Driver process changed'
    assert client.process.poll() is None and not client.failed and not client.closed, 'Driver runtime did not survive'


def lane_states(status):
    assert status['state'] == 'input_v3_candidate'
    data = status['input']
    assert data['protocol'] == 3 and data['test_only'] is False and data['transport_ready'] is True
    assert len(data['lanes']) == 2 and {row['lane'] for row in data['lanes']} == {0, 1}
    lanes = {row['lane'] + 1: row for row in data['lanes']}
    for row in lanes.values():
        for key in ('reserved', 'lease_active', 'drag_active', 'pointer_focus', 'keyboard_focus'):
            assert type(row[key]) is bool, 'malformed lane state'
        assert row['lease_active'] is False and row['drag_active'] is False
        assert row['held_button'] == 0 and row['held_keys'] == 0 and row['keyboard_focus'] is False
        assert type(row['dispatches']) is int and row['dispatches'] >= 0
        assert isinstance(row['epoch'], str) and row['epoch']
        assert type(row['desktop_generation']) is int
    return lanes


def unchanged_lanes(before, after):
    for lane in before:
        for key in ('epoch', 'desktop_generation', 'dispatches'):
            assert before[lane][key] == after[lane][key], f'lane {key} changed during idle'


def wait_for_idle_expiry(client, runtime, occupied_status, lane, dispatch_ns, save,
                         *, read_status=read_input_status, now=time.monotonic_ns, sleep=time.sleep):
    """Status is read through hyprctl, never through the idle input connection."""
    original = lane_states(occupied_status)
    assert original[lane]['reserved'] is True and original[lane]['pointer_focus'] is True
    assert not original[3 - lane]['reserved'] and not original[3 - lane]['pointer_focus']
    counter = client.counter
    deadline = dispatch_ns + EXPIRY_DEADLINE_SECONDS * 1_000_000_000
    samples = []
    while now() + STATUS_READ_BUDGET_NS <= deadline:
        require_runtime(client, runtime)
        assert client.counter == counter, 'idle runtime received another MCP request'
        status = read_status()
        observed_ns = now()
        assert observed_ns <= deadline, 'idle expiry observation exceeded 85 seconds'
        current = lane_states(status)
        unchanged_lanes(original, current)
        assert not current[3 - lane]['reserved'] and not current[3 - lane]['pointer_focus'], 'another input owner appeared'
        samples.append({'observed_ns': observed_ns, 'status': status})
        save('idle-observations.json', samples)
        require_runtime(client, runtime)
        assert client.counter == counter, 'idle runtime received another MCP request'
        if not current[lane]['reserved']:
            assert observed_ns - dispatch_ns >= IDLE_SECONDS * 1_000_000_000, 'lane disappeared before real idle timeout'
            assert current[lane]['pointer_focus'] is True, 'idle expiry unnecessarily cleared inert hover'
            return {'verified': True, 'elapsed_ns': observed_ns - dispatch_ns, 'status': status,
                    'runtime_pid': runtime['pid'], 'mcp_counter': counter, 'lane': lane}
        assert current[lane]['pointer_focus'] is True, 'passive focus disappeared before peer expiry'
        sleep(min(1, max(0, (deadline - now()) / 1_000_000_000)))
    raise AssertionError('input peer did not expire within 85 seconds; no second action')


def grid_digest(snapshot, pixels):
    x, y, width, height = grounding.calc_table(snapshot, pixels)
    digest = hashlib.sha256()
    for row in range(y, y + height):
        offset = row * pixels.stride + x * pixels.channels
        digest.update(bytes(pixels.data[offset:offset + width * pixels.channels]))
    return digest.hexdigest()


def click_once(client, observer, spec, stage, runtime, identity, trace, boundary, save, result, guard):
    """Exactly one click invocation; unknown outcomes remain failures without replay."""
    assert stage in STAGES
    require_runtime(client, runtime)
    assert calc_identity(spec) == identity, 'Calc process identity changed'
    fresh = {**spec, 'pointer_stage': stage}
    before = grounded_snapshot(client, spec['target'], fresh)
    pixels = grounding.read_pixels(before['proof_image'])
    arguments, oracle = grounding.action(before, pixels, 'calc', stage)
    before_digest = grid_digest(before, pixels)
    result.update(stage=stage, session=spec['name'], target=dict(spec['target']), runtime_pid=runtime['pid'],
                  grounding=before, arguments=arguments, oracle=oracle, replayed=False)
    save(stage + '-grounding.json', result)
    guard()
    require_runtime(client, runtime)
    assert process_birth(spec['target']['pid']) == identity['start_ticks'], 'Calc process identity changed'
    dispatch_ns = time.monotonic_ns()
    assert 0 <= dispatch_ns - before['proof_observation_started_ns'] <= MAX_GROUNDING_AGE_NS, \
        'snapshot grounding expired; no input sent'
    result['action'] = {'outcome': 'unknown', 'dispatch_ns': dispatch_ns, 'attempts': 1, 'replayed': False}
    try:
        response = client.tool('click', {**arguments, **spec['target'], 'session': spec['name'],
                                        'delivery_mode': 'background'})
    except Exception as error:
        result['action']['error'] = str(error)
    else:
        result['action'].update(outcome='response', response=response)
    save(stage + '-action.json', result['action'])
    after = grounded_snapshot(observer, spec['target'], fresh, session=False)
    save(stage + '-after.json', after)
    assert result['action']['outcome'] == 'response', 'click outcome unknown; no replay'
    check_response(response, {'kind': 'dispatched'})
    after_pixels = grounding.read_pixels(after['proof_image'])
    result['app_effect'] = grounding.verify(after, after_pixels, oracle)
    after_digest = grid_digest(after, after_pixels)
    assert before_digest != after_digest, 'Calc grid pixels did not change'
    result['pixels'] = {'before': before_digest, 'after': after_digest, 'changed': True}
    assert calc_identity(spec) == identity, 'Calc process identity changed'
    require_runtime(client, runtime)
    page = trace.collect()
    lane = capacity_lane(boundary, page, 'click')
    result['trace'] = verify_recovery_trace(boundary, page, lane, 'click')
    guard()
    result['result'] = 'verified'
    return page, lane


def episode(args, plan, client, observer, runtime, identity, stage, save, result, *, final=False):
    """A new held foreground grab and trace, both bounded below their 60s limit."""
    grab = trace = prefix = baseline = primary_before = None
    deadline = None
    def guard():
        require_primary_active(grab, deadline)
    try:
        fg = grounded_snapshot(observer, plan['foreground'])['window_bounds']
        desktop = observer.tool('get_desktop_state', {})
        assert not desktop.get('isError'), desktop
        desktop = desktop['structuredContent']
        x, y = plan['primary_point']
        assert 0 < x < fg['width'] and 0 < y < fg['height']
        x, y = fg['x'] + x, fg['y'] + y
        assert 0 <= x < desktop['screen_width'] and 0 <= y < desktop['screen_height']
        deadline = time.monotonic_ns() + PRIMARY_LIFETIME_MS * 1_000_000
        grab = subprocess.Popen([str(args.primary_grab), str(x), str(y), str(desktop['screen_width']),
                                 str(desktop['screen_height']), str(PRIMARY_LIFETIME_MS)],
                                stdout=subprocess.PIPE, text=True)
        assert primary_acknowledgement(grab.stdout) == 'HELD\n'
        wait_for(lambda: state(args.foreground_journal)['held'])
        primary_before, baseline = wm(), state(args.foreground_journal)
        assert primary_before['pid'] == plan['foreground']['pid'] and baseline['held']
        trace = Trace(args.trace_socket)
        assert trace.hello['protocol'] == 3
        trace.exchange('TRACE_START')
        boundary = trace.collect()
        save(stage + '-boundary.json', boundary)
        prefix, lane = click_once(client, observer, plan['agents'][0], stage, runtime, identity,
                                  trace, boundary, save, result, guard)
        save(stage + '-prefix.json', prefix)
        result['occupied_status'] = read_input_status()
        occupied = lane_states(result['occupied_status'])
        assert occupied[lane]['reserved'] and occupied[lane]['pointer_focus']
        assert not occupied[3 - lane]['reserved'] and not occupied[3 - lane]['pointer_focus']
        result['lane'] = lane
        result['client_survived'] = True
    finally:
        operations = []
        if final:
            operations.append(('close_input_runtime', lambda: close_owned(client)))
        if trace:
            def finish_trace():
                guard()
                trace.exchange('TRACE_STOP')
                stopped = trace.collect()
                save(stage + '-trace.json', stopped)
                assert prefix is not None, 'click did not produce a complete trace prefix'
                result['continuous_isolation'] = verify_recovery_cleanup(prefix, stopped)
                assert wm() == primary_before, 'primary cursor/focus changed'
                current = state(args.foreground_journal)
                assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held')), current
                if final:
                    status = read_input_status()
                    save('closed-input-status.json', status)
                    closed = lane_states(status)
                    assert all(not row['reserved'] for row in closed.values())
                    assert closed[lane]['pointer_focus'] and not closed[3 - lane]['pointer_focus']
                    result['synthetic_cleanup'] = 'verified'
                guard()
            operations += [('finish_trace', finish_trace), ('close_trace', trace.close)]
        def release_primary():
            if grab:
                if grab.poll() is None:
                    grab.terminate()
                stop_process(grab)
                wait_for(lambda: not state(args.foreground_journal)['held'])
        operations.append(('release_primary', release_primary))
        errors = cleanup_all(operations)
        result['cleanup_errors'] = errors
        save(stage + '-result.json', result)
        assert not errors, errors


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    plan = json.loads(args.plan.read_text())
    validate_plan(plan)
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2))
    save('plan.json', plan)
    report = {'result': 'failed', 'scope': 'native-production-idle-reconnect',
              'full_desktop_matrix': False, 'actions': []}
    clients = []
    try:
        assert args.trace_socket.name in ('cua-input-v3.sock', 'cua-input-v3-2.sock')
        origin = provenance(args, plan)
        for name in ('production_idle_reconnect_proof.py', 'production_idle_reconnect_proof_test.py',
                     'production_cancel_proof.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        save('provenance.json', origin)
        spec = plan['agents'][0]
        identity = calc_identity(spec)
        report['app_identity'] = identity
        initial = read_input_status()
        save('initial-input-status.json', initial)
        assert all(not row['reserved'] and not row['pointer_focus'] for row in lane_states(initial).values())
        for name in ('input-runtime', 'observer'):
            directory = args.evidence / name
            directory.mkdir()
            clients.append(DirectMCP(args.driver, directory, PROFILE))
        client, observer = clients
        report['driver_processes'] = assert_distinct_runtimes(clients)
        runtime = {'process': client.process, 'pid': client.process.pid}
        response = client.tool('start_session', {'session': spec['name']})
        assert not response.get('isError'), response
        first = {}
        report['actions'].append(first)
        episode(args, plan, client, observer, runtime, identity, STAGES[0], save, first)
        report['expiry'] = wait_for_idle_expiry(client, runtime, first['occupied_status'], first['lane'],
                                               first['action']['dispatch_ns'], save)
        second = {}
        report['actions'].append(second)
        episode(args, plan, client, observer, runtime, identity, STAGES[1], save, second, final=True)
        assert second['lane'] == first['lane'], 'fresh action did not reacquire the same lane'
        before = lane_states(report['expiry']['status'])
        after = lane_states(second['occupied_status'])
        for lane in before:
            for key in ('epoch', 'desktop_generation'):
                assert before[lane][key] == after[lane][key], 'plugin identity changed before reacquisition'
            assert after[lane]['dispatches'] == before[lane]['dispatches'] + (lane == second['lane']), \
                'fresh action dispatch count differs; possible replay'
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        errors = cleanup_all([(f'close_runtime_{i}', lambda c=c: close_owned(c)) for i, c in enumerate(clients)])
        report['cleanup_errors'] = errors
        if errors:
            report['result'] = 'failed'
        save('result.json', report)
    print(json.dumps(report), flush=True)
    return 0 if report['result'] == 'passed' else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('driver', 'plugin', 'source', 'primary-grab', 'plan', 'evidence', 'foreground-journal', 'trace-socket'):
        parser.add_argument('--' + name, required=True, type=Path)
    add_provenance_arguments(parser)
    raise SystemExit(run(parser.parse_args()))
