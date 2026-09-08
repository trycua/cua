"""TEST ONLY: ordinary Calc drag cancelled by a fixture-owned primary hover.

Use settled primary-conflict CLI flags plus --hover-fixture. The exact VM plan
has purpose=active_primary, case=active_drag, fault={kind:primary_hover},
recovery={pointer_stages:[click_a1,click_b2]}, and hover_fixture containing
path/device/inode/uid/sha256/source_sha256 for the reviewed primary_hover_fixture.
Only one native 1:1 monitor at origin, input:follow_mouse=1, and an initially
parked, RELEASED foreground fixture qualify. No config edits or hidden clicks.

The new helper arms without input, binds the compositor's SO_PEERCRED identity,
then accepts one explicit MOVE after a fresh held-Calc-drag gate. Its sync ACK
is NOT focus proof: independently require the exact Calc primary client plus
the product's partial primary_target_busy response and own-seat release.
Keep the raw transition trace and its canonical isolation failures. Continuous
isolation across setup/transition is UNPROVEN. Only the distinct settled
recovery action has a strict isolation claim. No replay of unknown/partial work.
Portable tests establish preparation only, never native certification.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import re
import select
import stat
import subprocess
import time

from desktop_faults import _hypr, _identity
from driver_input_live import state, wait_for
from primary_trace import analyze
from production_active_lock_proof import (drag_once, held_status, recovery_stage,
    transition_evidence as _transition_evidence,
    verify_transition_end as _verify_transition_end)
from production_cancel_proof import (MAX_GROUNDING_AGE_NS, PROFILE, active_drags,
    close_owned, grounded_snapshot, poll_active, prepare_drag, stopped_prefix,
    verify_fresh_observation, verify_recovery_cleanup, verify_recovery_trace)
from production_lock_refusal_proof import click_once, verify_runtimes
from production_mcp import DirectMCP, stop_process
import production_pointer_grounding as pointer_grounding
from production_primary_conflict_proof import (ExactDesktop, clear_status,
    validate_plan as settled_plan)
from production_realapp_proof import (app_process_identity, capacity_lane,
    check_response, provenance, trace_interval)
from production_session_fault_proof import lanes
from realapp_proof import cleanup_all, released_synthetic_input


HOVER_MS = 20000
REASON = 'primary_target_busy'


def verify_parked_foreground(before, after):
    """The fixture emits a new timestamp every 250 ms, even without input."""
    assert before.get('kind') == after.get('kind') == 'state'
    assert all(type(row.get('time')) is int and row['time'] >= 0 for row in (before, after))
    assert after['time'] >= before['time'], 'foreground observation moved backwards'
    assert {key: value for key, value in before.items() if key != 'time'} == \
        {key: value for key, value in after.items() if key != 'time'}, 'foreground state changed before hover'


def transition_evidence(page, *, stopped=False):
    result = _transition_evidence(page, stopped=stopped)
    result['classification'] = 'observed_transition_events_not_attributed_to_hover_fixture_or_driver'
    return result


def verify_transition_end(boundary, stopped):
    result = _verify_transition_end(boundary, stopped)
    result['classification'] = 'observed_transition_events_not_attributed_to_hover_fixture_or_driver'
    return result


def validate_plan(plan):
    assert plan['purpose'] == 'active_primary' and plan['case'] == 'active_drag'
    assert plan['fault'] == {'kind': 'primary_hover'}
    assert plan['recovery'] == {'pointer_stages': ['click_a1', 'click_b2']}
    candidate = {k: v for k, v in plan.items() if k != 'fault'}
    settled_plan({**candidate, 'purpose': 'primary_conflict', 'case': 'initial_refusal',
                  'recovery': {'pointer_stage': 'click_b2'}})
    assert plan['agents'][0]['app'] == 'calc'
    expected = plan['hover_fixture']
    assert set(expected) == {'path', 'device', 'inode', 'uid', 'sha256', 'source_sha256'}
    assert Path(expected['path']).is_absolute() and Path(expected['path']).name == 'primary_hover_fixture'
    assert all(type(expected[k]) is int and expected[k] >= 0 for k in ('device', 'inode', 'uid'))
    assert expected['uid'] == plan['compositor']['uid']
    assert all(re.fullmatch(r'[0-9a-f]{64}', expected[k]) for k in ('sha256', 'source_sha256'))


def desktop_mode(desktop):
    """A precondition, never a request to modify compositor policy."""
    desktop.guard()
    follow = json.loads(_hypr(desktop.instance, '-j', 'getoption', 'input:follow_mouse'))
    assert type(follow.get('int')) is int and follow['int'] == 1, 'need existing input:follow_mouse=1'
    monitors = json.loads(_hypr(desktop.instance, '-j', 'monitors'))
    assert len(monitors) == 1, 'only a single monitor is qualified'
    monitor = monitors[0]
    assert monitor['x'] == monitor['y'] == 0 and monitor['scale'] == 1 and monitor['transform'] == 0
    assert monitor.get('dpmsStatus') is True
    assert all(type(monitor[k]) is int and 0 < monitor[k] <= 65535 for k in ('width', 'height'))
    return {'follow_mouse': follow['int'],
            **{k: monitor[k] for k in ('id', 'name', 'x', 'y', 'width', 'height', 'scale', 'transform')}}


def cancelled_status(before, after, lane):
    held_status(before, lane)
    clear_status(after)
    old, new = lanes(before), lanes(after, cleared=True)
    assert set(old) == set(new) == {0, 1}
    for key in old:
        assert all(old[key][field] == new[key][field]
                   for field in ('epoch', 'desktop_generation', 'dispatches')), 'unrelated desktop transition or dispatch'
        if key != lane - 1:
            assert old[key] == new[key], 'sibling lane changed'
        else:
            # Unlike lock, primary_changed keeps this runtime's reservation.
            assert new[key].get('reserved') is True, 'primary cancellation lost its reservation'


def verify_cancelled(boundary, record, action, target):
    prefix, lane = record['prefix'], record['lane']
    assert record['result'] == 'observed' and record['target'] == target
    assert set(active_drags(prefix)) == {lane}
    assert all(row[5] in (0, lane) for row in prefix['events']), 'unowned synthetic input'
    assert all(row[2] in ('agent_approved', 'agent_admitted', 'agent_drag_start',
                         'pointer_enter', 'pointer_motion', 'pointer_button')
               for row in prefix['events'] if row[5] == lane), 'unexpected pre-transition input'
    assert analyze(stopped_prefix(prefix))['result'] == 'passed', 'isolation failed before fixture MOVE'
    requested, observed = record['requested_ns'], record['observed_ns']
    assert 0 <= requested - prefix['events'][-1][1] <= 250_000_000
    assert 0 <= requested - record['status_started_ns'] <= 250_000_000
    assert 0 <= requested - record['ready']['observed_ns'] < 2_500_000_000
    assert record['ack']['event'] == 'moved' and record['ready']['event'] == 'ready'
    assert [record['ack']['x'], record['ack']['y']] == record['point']
    assert requested <= record['ack']['observed_ns'] <= observed < record['deadline_ns']
    assert record['primary_after']['pid'] == target['pid'] and record['primary_after']['window_id'] == target['window_id']
    assert record['primary_after']['cursor'] == dict(zip(('x', 'y'), record['point']))
    tail = trace_interval(prefix, boundary)
    assert any(row[5] == 0 and row[2] in ('pointer_focus', 'keyboard_focus')
               and requested <= row[1] <= observed for row in tail), 'missing primary focus transition'
    synthetic = [row for row in tail if row[5] in (1, 2)]
    assert all(row[5] == lane for row in synthetic), 'cancellation crossed lanes'
    cancels = [row for row in synthetic if row[2] == 'agent_cancel']
    assert len(cancels) == 1 and requested <= cancels[0][1] <= observed
    cancel = cancels[0]
    assert all(row[2] in ('agent_cancel', 'pointer_button', 'pointer_leave', 'keyboard_leave',
                         'pointer_enter', 'pointer_motion') for row in synthetic), 'extra action or false completion'
    assert not any(row[2] in ('pointer_enter', 'pointer_motion') and row[0] > cancel[0]
                   for row in synthetic), 'input continued after cancellation'
    releases = [row for row in synthetic if row[2] == 'pointer_button']
    assert len(releases) == 1 and releases[0][6] == 0 and releases[0][0] > cancel[0]
    assert releases[0][1] <= observed, 'release not observed at cleared status'
    released_synthetic_input(boundary)
    cancelled_status(record['gate_status'], record['after'], lane)
    assert action['outcome'] == 'response' and action['replayed'] is False, 'unknown outcome; never replay'
    check_response(action['response'], {'kind': 'partial'})
    content = action['response']['structuredContent']
    assert content.get('reason') == REASON and content['delivery']['mode'] == 'background'
    assert content['delivery']['delivered_count'] > 0, 'no acknowledged gesture progress'
    assert action['dispatch_ns'] <= prefix['events'][-1][1] <= action['observed_ns'] < record['deadline_ns']
    return {'result': 'verified', 'reason': REASON, 'lane': lane,
            'cancel_sequence': cancel[0], 'release_sequence': releases[0][0],
            'continuous_primary_isolation': 'unproven', 'saved_app_effect': 'unproven'}


class HoverFixture:
    """One reviewed executable, exact compositor, one command, no held buttons."""
    def __init__(self, desktop, args):
        self.desktop, self.args = desktop, args
        self.expected = desktop.plan['hover_fixture']
        self.child = None
        self.buffer = b''
        self.sent = False
        self.record = {'result': 'unproven', 'events': []}
        self.mode = desktop_mode(desktop)
        self.record['mode'] = self.mode
        self.check_binary()

    def check_binary(self):
        path, expected = self.args.hover_fixture, self.expected
        assert str(path) == expected['path'] and path.resolve(strict=True) == path
        info = path.lstat()
        assert stat.S_ISREG(info.st_mode) and os.access(path, os.X_OK)
        assert [info.st_dev, info.st_ino, info.st_uid] == [expected[k] for k in ('device', 'inode', 'uid')]
        assert hashlib.sha256(path.read_bytes()).hexdigest() == expected['sha256']
        source = self.args.source / 'libs/cua-driver/hyprland-plugin/tests/primary_hover_fixture.c'
        assert source.resolve(strict=True) == source
        assert hashlib.sha256(source.read_bytes()).hexdigest() == expected['source_sha256']

    def guard(self):
        assert desktop_mode(self.desktop) == self.mode, 'display geometry changed'
        self.check_binary()
        assert self.child.poll() is None and _identity(self.child.pid) == self.identity
        with Path(f'/proc/{self.child.pid}/exe').open('rb') as stream:
            info = os.fstat(stream.fileno())
            assert [info.st_dev, info.st_ino, info.st_uid] == [self.expected[k] for k in ('device', 'inode', 'uid')]
            assert hashlib.sha256(stream.read()).hexdigest() == self.expected['sha256']
        assert self.child.poll() is None and _identity(self.child.pid) == self.identity

    def event(self, expected):
        def read():
            if b'\n' not in self.buffer:
                if not select.select([self.child.stdout], [], [], 0)[0]:
                    return None
                chunk = os.read(self.child.stdout.fileno(), 4096)
                assert chunk, 'fixture EOF before acknowledgement'
                self.buffer += chunk
                assert len(self.buffer) <= 4096
                if b'\n' not in self.buffer:
                    return None
            line, self.buffer = self.buffer.split(b'\n', 1)
            value = json.loads(line)
            assert set(value) == {'event', 'x', 'y', 'observed_ns'} and value['event'] == expected
            assert all(type(value[k]) is int and value[k] >= 0 for k in ('x', 'y', 'observed_ns'))
            assert 0 < value['observed_ns'] <= time.monotonic_ns()
            assert not self.record['events'] or self.record['events'][-1]['observed_ns'] <= value['observed_ns']
            self.record['events'].append(value)
            return value
        return wait_for(read, timeout=2)

    def arm(self):
        assert self.child is None
        assert desktop_mode(self.desktop) == self.mode
        self.check_binary()
        with (self.args.evidence / ('hover-' + str(time.monotonic_ns()) + '.stderr')).open('wb') as log:
            self.child = subprocess.Popen([str(self.args.hover_fixture), str(self.mode['width']),
                str(self.mode['height']), str(HOVER_MS), str(self.desktop.compositor['pid'])],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, start_new_session=True)
        self.identity = _identity(self.child.pid)
        assert self.identity['uid'] == self.desktop.compositor['uid'] and self.identity['exe'] == str(self.args.hover_fixture)
        self.record['identity'] = self.identity
        self.record['ready'] = self.event('ready')
        self.guard()

    def move(self, point, target, prepared_ns, *, pending=None):
        assert not self.sent, 'a fixture command is never replayed'
        self.guard()
        assert target in (self.desktop.plan['foreground'], self.desktop.plan['agents'][0]['target'])
        assert len(point) == 2 and all(type(v) is int for v in point)
        assert 0 <= point[0] < self.mode['width'] and 0 <= point[1] < self.mode['height']
        # Target containment must remain true at the last identity check.
        window = next(w for w in self.desktop.guard() if w['pid'] == target['pid'])
        assert all(start < value < start + size for value, start, size in zip(point, window['at'], window['size']))
        requested = time.monotonic_ns()
        assert 0 <= requested - prepared_ns <= MAX_GROUNDING_AGE_NS, 'stale hover grounding'
        assert 0 <= requested - self.record['ready']['observed_ns'] < 2_500_000_000, 'ready window expired'
        if pending is not None:
            assert not pending.done(), 'drag returned before MOVE'
            assert 0 <= requested - self.record['prefix']['events'][-1][1] <= 250_000_000, 'stale held trace'
            assert 0 <= requested - self.record['status_started_ns'] <= 250_000_000, 'stale held status'
        self.record.update(requested_ns=requested, point=point, target=target,
                           prepared_ns=prepared_ns, deadline_ns=requested + HOVER_MS * 1_000_000)
        self.sent = True  # Lost ack still means the single command may have landed.
        self.child.stdin.write(f'MOVE {point[0]} {point[1]}\n'.encode('ascii'))
        self.child.stdin.flush()
        self.record['ack'] = self.event('moved')
        assert [self.record['ack']['x'], self.record['ack']['y']] == point
        self.record['primary_after'] = self.desktop.primary(target)
        assert self.record['primary_after']['cursor'] == dict(zip(('x', 'y'), point)), 'hover missed grounded target'
        self.guard()
        self.record['observed_ns'] = time.monotonic_ns()
        self.record['result'] = 'observed'

    def inject(self, trace, initial, pending, prepared):
        self.guard()
        first, _ = poll_active(trace, initial, None, [pending], timeout=1)
        started = time.monotonic_ns()
        gate = json.loads(_hypr(self.desktop.instance, '-j', 'cua:status'))
        page, active = poll_active(trace, first, None, [pending], timeout=.25)
        lane = next(iter(active))
        held_status(gate, lane)
        assert analyze(stopped_prefix(page))['result'] == 'passed'
        self.record.update(prefix=page, lane=lane, gate_status=gate, status_started_ns=started)
        bounds = prepared['snapshot']['window_bounds']
        point = [bounds['x'] + prepared['arguments']['from_x'], bounds['y'] + prepared['arguments']['from_y']]
        self.move(point, prepared['target'], prepared['prepared_ns'], pending=pending)
        self.record['after'] = json.loads(_hypr(self.desktop.instance, '-j', 'cua:status'))
        self.record['observed_ns'] = time.monotonic_ns()
        cancelled_status(gate, self.record['after'], lane)

    def close(self):
        if not self.child:
            return
        if self.child.poll() is None and not self.child.stdin.closed:
            self.child.stdin.close()  # Never manufactures a button release.
        stop_process(self.child)
        self.record['exit_code'] = self.child.returncode
        if self.sent:
            assert self.child.returncode == 0, 'hover helper did not finish by acknowledged controller EOF'
            if 'finished' not in self.record:
                self.record['finished'] = self.event('finished')


def prepare_recovery(client, spec, previous, allowed_stages):
    started_ns = time.monotonic_ns()
    before = grounded_snapshot(client, spec['target'], spec)
    verify_fresh_observation(previous, before, client, after_ns=started_ns)
    stage = recovery_stage(before)
    assert stage in allowed_stages
    arguments, oracle = pointer_grounding.action(before, pointer_grounding.read_pixels(before['proof_image']), 'calc', stage)
    return {'snapshot': before, 'arguments': arguments, 'oracle': oracle,
            'stage': stage, 'prepared_ns': before['proof_observation_started_ns']}


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2) + '\n')
    report = {'result': 'failed', 'scope': 'active-primary-client-conflict-cancellation',
        'cancellation': {'result': 'unproven'}, 'recovery': {'result': 'unproven'},
        'continuous_isolation_across_transitions': 'unproven', 'full_desktop_matrix': False,
        'physical_hardware': False, 'runtime_pids': []}
    clients, fixtures, trace, pool, future = [], [], None, None, None
    tracing, phase = False, 'transition'
    def launch(name):
        directory = args.evidence / name
        directory.mkdir()
        client = DirectMCP(args.driver, directory, PROFILE)
        clients.append(client)
        verify_runtimes(clients)
        report['runtime_pids'].append(client.process.pid)
        return client
    def start_trace():
        nonlocal tracing
        started = time.monotonic_ns()
        tracing = True
        trace.exchange('TRACE_START')
        page = trace.collect()
        trace_interval(page, page)
        assert page['count'] == 1 and started <= page['events'][0][1] <= time.monotonic_ns()
        save(phase + '-initial.json', page)
        return page
    try:
        plan = json.loads(args.plan.read_text())
        save('plan.json', plan)
        validate_plan(plan)
        desktop = ExactDesktop(plan)
        spec = plan['agents'][0]
        app_process_identity('calc', spec['target']['pid'])
        origin = provenance(args, plan)
        for name in (Path(__file__).name, 'production_active_primary_proof_test.py',
                     'primary_hover_fixture.c', 'primary_hover_fixture_test.py',
                     'production_active_lock_proof.py', 'production_primary_conflict_proof.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        origin['hover_fixture'] = plan['hover_fixture']
        origin['ownership'] = {key: plan[key] for key in ('vm', 'compositor', 'processes')}
        save('provenance.json', origin)
        save('initial-status.json', desktop.status(unreserved=True, allow_passive=True))
        before_primary = desktop.primary(plan['foreground'])
        baseline = state(args.foreground_journal)
        assert baseline['held'] is False, 'a primary grab prevents the required cross-client hover'
        save('initial-primary.json', {'primary': before_primary, 'foreground': baseline})
        observer, actor = launch('observer'), launch('drag')
        assert not actor.tool('start_session', {'session': spec['name']}).get('isError')
        prepared = prepare_drag(actor, spec)
        save('drag-grounding.json', prepared)
        fixture = HoverFixture(desktop, args)
        fixtures.append(fixture)
        trace = desktop.trace(args.trace_socket)
        initial = start_trace()
        fixture.arm()
        assert desktop.primary(plan['foreground']) == before_primary
        verify_parked_foreground(baseline, state(args.foreground_journal))
        action = report['action'] = {'outcome': 'unknown', 'replayed': False,
            'prepared_ns': prepared['prepared_ns'], 'runtime_pid': actor.process.pid}
        pool = ThreadPoolExecutor(max_workers=1)
        future = pool.submit(drag_once, actor, spec, prepared, action, save)
        fixture.inject(trace, initial, future, prepared)
        future.result(timeout=3)
        boundary = trace.collect()
        save('cancellation-boundary.json', boundary)
        save('cancellation-transition-analysis.json', transition_evidence(boundary))
        save('primary-transition.json', fixture.record)
        report['cancellation'] = verify_cancelled(boundary, fixture.record, action, spec['target'])
        save('cancelled-app-state.json', grounded_snapshot(observer, spec['target'], spec, session=False))
        close_owned(actor)  # Release was proved before runtime teardown.
        desktop.status(unreserved=True)
        fixture.close()
        # Freshly observe the foreground before one explicitly owned restoration.
        grounded_ns = time.monotonic_ns()
        foreground_snapshot = grounded_snapshot(observer, plan['foreground'])
        save('foreground-grounding.json', {'snapshot': foreground_snapshot, 'prepared_ns': grounded_ns})
        foreground = foreground_snapshot['window_bounds']
        point = [foreground['x'] + plan['primary_point'][0], foreground['y'] + plan['primary_point'][1]]
        restored = HoverFixture(desktop, args)
        fixtures.append(restored)
        restored.arm()
        restored.move(point, plan['foreground'], grounded_ns)
        save('foreground-restoration.json', restored.record)
        trace.exchange('TRACE_STOP')
        stopped = trace.collect()
        tracing = False
        save('transition-trace.json', stopped)
        save('transition-analysis.json', transition_evidence(stopped, stopped=True))
        report['transition'] = verify_transition_end(boundary, stopped)
        settled = desktop.status(unreserved=True)
        save('settled-status.json', settled)
        primary, baseline = desktop.primary(plan['foreground']), state(args.foreground_journal)
        assert baseline['held'] is False
        phase = 'recovery'
        initial = start_trace()
        fresh = launch('recovery')
        assert actor.process.poll() is not None and fresh.process.pid != actor.process.pid
        recovery = report['recovery']
        spec = {**spec, 'name': spec['name'] + '-recovery'}
        assert not fresh.tool('start_session', {'session': spec['name']}).get('isError')
        grounding = prepare_recovery(fresh, spec, prepared['snapshot'], plan['recovery']['pointer_stages'])
        before, arguments, oracle = grounding['snapshot'], grounding['arguments'], grounding['oracle']
        prepared_ns, spec['pointer_stage'] = grounding['prepared_ns'], grounding['stage']
        save('recovery-grounding.json', grounding)
        restored.guard()
        assert desktop.primary(plan['foreground']) == primary
        recovery['action'] = {'outcome': 'unknown', 'replayed': False, 'prepared_ns': prepared_ns,
                              'runtime_pid': fresh.process.pid}
        response = click_once(fresh, {**arguments, **spec['target'], 'session': spec['name'],
            'delivery_mode': 'background'}, recovery['action'], save, 'recovery-action.json')
        check_response(response, {'kind': 'dispatched'})
        after = grounded_snapshot(observer, spec['target'], spec, session=False)
        save('recovery-after.json', after)
        verify_runtimes(clients)
        assert recovery['action']['dispatch_ns'] <= recovery['action']['observed_ns']
        verify_fresh_observation(before, after, observer, after_ns=recovery['action']['observed_ns'])
        recovery['app_effect'] = pointer_grounding.verify(after, pointer_grounding.read_pixels(after['proof_image']), oracle)
        prefix = trace.collect()
        save('recovery-prefix.json', prefix)
        recovery['trace'] = verify_recovery_trace(initial, prefix, capacity_lane(initial, prefix, 'click'), 'click')
        close_owned(fresh)
        trace.exchange('TRACE_STOP')
        stopped = trace.collect()
        tracing = False
        save('recovery-trace.json', stopped)
        recovery['isolation'] = verify_recovery_cleanup(prefix, stopped)
        restored.guard()
        assert time.monotonic_ns() < restored.record['deadline_ns']
        current = state(args.foreground_journal)
        assert desktop.primary(plan['foreground']) == primary
        assert all(current[k] == baseline[k] for k in ('held', 'clicks', 'keys', 'scroll'))
        final = desktop.status(unreserved=True, allow_passive=True)
        save('recovery-primary-readback.json', {'before': primary, 'after': desktop.primary(plan['foreground']),
             'foreground_before': baseline, 'foreground_after': current})
        save('final-status.json', final)
        old, new = lanes(settled, cleared=True), lanes(final, cleared=True, allow_passive=True)
        assert sum(new[k]['dispatches'] - old[k]['dispatches'] for k in old) == 1
        for key in old:
            assert new[key]['dispatches'] >= old[key]['dispatches']
            assert all(new[key][field] == old[key][field] for field in ('epoch', 'desktop_generation'))
        recovery['result'] = 'verified'
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = {'type': type(error).__name__, 'message': str(error)}
    finally:
        operations = [(f'close_runtime_{i}', lambda c=c: close_owned(c)) for i, c in enumerate(clients)]
        if future:
            operations.append(('retain_action', lambda: future.result(timeout=3)))
        if pool:
            operations.append(('close_pool', lambda: pool.shutdown(wait=False)))
        if trace:
            if tracing:
                def preserve_trace():
                    trace.exchange('TRACE_STOP')
                    page = trace.collect()
                    save('failed-' + phase + '-trace.json', page)
                    save('failed-' + phase + '-analysis.json', transition_evidence(page, stopped=True))
                operations.append(('preserve_trace', preserve_trace))
            operations.append(('close_trace', trace.close))
        operations += [(f'close_hover_{i}', f.close) for i, f in enumerate(fixtures)]
        errors = cleanup_all(operations)
        for i, fixture in enumerate(fixtures):
            save(f'hover-{i}.json', fixture.record)
        save('cleanup.json', {'errors': errors})
        if errors:
            report['result'] = 'failed'
        save('result.json', report)
    print(json.dumps(report), flush=True)
    return 0 if report['result'] == 'passed' else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('driver', 'plugin', 'source', 'primary-grab', 'hover-fixture',
                 'plan', 'evidence', 'foreground-journal', 'trace-socket'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--source-sha', required=True)
    raise SystemExit(run(parser.parse_args()))
