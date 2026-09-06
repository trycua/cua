"""TEST ONLY: active Calc drag cancellation by an acknowledged session lock.

Use the settled lock proof's CLI flags and exact identity plan, but with
purpose=active_lock and recovery={pointer_stages:[click_a1,click_b2]}. Ground
before arming the reviewed real lock helper; its
ready-to-LOCK window is only three seconds. Normal Driver supplies all app
input. Require a fresh held-input gate before LOCK, an explicit partial
desktop_changed response, cancellation and own-seat release before graceful
protocol unlock, then a fresh-runtime, fresh-pixel, visibly effective click.
Unknown attempts are retained and never replayed or promoted to success.

The raw lock/unlock trace and its unchanged primary analysis are retained.
Primary events are classified, not removed: continuous primary isolation
across either transition is UNPROVEN, even if cancellation passes. Only the
pre-request prefix and separate settled recovery trace use strict isolation.
No native setup, authentication, product changes, or full-matrix certification.
Portable tests exercise orchestration and failure oracles, not native behavior.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import subprocess
import time

from desktop_faults import _identity
from driver_input_live import state, wait_for, wm
from primary_trace import analyze
from production_cancel_proof import (MAX_GROUNDING_AGE_NS, PROFILE, active_drags,
    close_owned, grounded_snapshot, poll_active, prepare_drag, stopped_prefix,
    verify_recovery_cleanup, verify_recovery_trace)
from production_lock_refusal_proof import (LOCK_MS, LockFixture, click_once,
    settle_locked, stable_status, validate_plan as lock_plan, verify_runtimes)
from production_mcp import DirectMCP, stop_process
import production_pointer_grounding as pointer_grounding
from production_realapp_proof import (PRIMARY_LIFETIME_MS, capacity_lane,
    check_response, primary_acknowledgement, provenance, require_primary_active,
    trace_interval)
from production_session_fault_proof import connect_trace, lanes, power, production_status
from realapp_proof import cleanup_all, released_synthetic_input


def validate_plan(plan):
    assert plan['purpose'] == 'active_lock' and plan['fault'] == {'kind': 'lock'}
    assert plan['recovery'] == {'pointer_stages': ['click_a1', 'click_b2']}
    lock_plan({**plan, 'purpose': 'lock_refusal', 'recovery': {'pointer_stage': 'click_b2'}})


def recovery_stage(snapshot):
    """Choose one newly grounded click, never retry a sent action."""
    return ('click_a1' if pointer_grounding.calc_formula_selection(
        snapshot, pointer_grounding.rows(snapshot), 'B2') else 'click_b2')


def prepare_recovery(client, spec, allowed_stages):
    before = grounded_snapshot(client, spec['target'], spec)
    prepared_ns = before['proof_observation_started_ns']
    assert type(prepared_ns) is int and 0 < prepared_ns <= time.monotonic_ns(), 'invalid observation timestamp'
    stage = recovery_stage(before)
    assert stage in allowed_stages
    arguments, oracle = pointer_grounding.action(
        before, pointer_grounding.read_pixels(before['proof_image']), 'calc', stage)
    return {'snapshot': before, 'arguments': arguments, 'oracle': oracle,
            'stage': stage, 'prepared_ns': prepared_ns}


def held_status(status, lane):
    """Trace lanes are one-based; production status lanes are zero-based."""
    rows = lanes(status)
    assert lane in (1, 2) and set(rows) == {0, 1}
    for key, row in rows.items():
        if key == lane - 1:
            assert type(row['held_button']) is int and row['held_button'] == 272
            assert type(row['held_keys']) is int and row['held_keys'] == 0
            assert all(row.get(k) is True for k in ('drag_active', 'lease_active', 'pointer_focus', 'reserved'))
        else:
            assert all(type(row.get(k)) is int and row[k] == 0 for k in ('held_button', 'held_keys'))
            assert all(row.get(k) is False for k in ('drag_active', 'lease_active', 'pointer_focus', 'keyboard_focus'))
            assert row.get('reserved') is False, 'unowned lane is reserved'


def cancelled_status(before, after):
    old, new = lanes(before), lanes(after, cleared=True)
    assert set(old) == set(new) == {0, 1}
    for lane in old:
        assert old[lane]['epoch'] == new[lane]['epoch'], 'compositor lane replaced'
        assert old[lane]['dispatches'] == new[lane]['dispatches'], 'cancelled drag completed a dispatch'
        assert new[lane]['desktop_generation'] > old[lane]['desktop_generation'], 'lock did not revoke authority'
        assert new[lane].get('reserved') is False, 'reservation survived lock'


class ActiveLockFixture(LockFixture):
    """Keep the settled helper's identity and graceful-unlock guards intact."""
    def arm(self):
        self.check_targets()
        self.check_binary()
        assert self.child is None, 'fixture already armed'
        self.record['before'] = production_status(self.config)
        stable_status(self.record['before'], self.record['before'])
        with (self.args.evidence / 'lock-fixture.stderr').open('wb') as log:
            self.child = subprocess.Popen([str(self.args.lock_fixture), str(LOCK_MS),
                str(self.config['compositor']['pid'])], stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=log, start_new_session=True)
        self.identity = _identity(self.child.pid)
        assert self.identity['uid'] == self.config['compositor']['uid']
        assert self.identity['exe'] == str(self.args.lock_fixture)
        self.record['identity'] = self.identity
        self.record['ready'] = self.event('ready')
        self.check_running_binary()

    def inject(self, trace, initial, pending):
        assert self.child is not None and not self.requested
        self.check_targets()
        self.check_binary()
        self.check_running_binary()
        power(self.config, True)
        first, _ = poll_active(trace, initial, None, [pending], timeout=1)
        status_started = time.monotonic_ns()
        gate = production_status(self.config)
        page, active = poll_active(trace, first, None, [pending], timeout=0.25)
        lane = next(iter(active))
        held_status(gate, lane)
        for key, row in lanes(self.record['before']).items():
            assert all(row[field] == lanes(gate)[key][field]
                       for field in ('epoch', 'desktop_generation', 'dispatches')), 'desktop changed before LOCK'
        self.record.update(gate_status=gate, status_started_ns=status_started,
                           prefix=page, lane=lane)
        isolation = analyze(stopped_prefix(page))
        self.record['pre_request_isolation'] = isolation
        assert isolation['result'] == 'passed', isolation
        assert not pending.done(), 'drag finished before LOCK'
        requested = time.monotonic_ns()
        assert 0 <= requested - page['events'][-1][1] <= 250_000_000, 'stale held-input trace'
        assert 0 <= requested - status_started <= 250_000_000, 'stale held-input status'
        assert 0 <= requested - self.record['ready']['observed_ns'] < 2_500_000_000, 'fixture ready window expired'
        self.record['requested_ns'] = requested
        self.deadline_ns = requested + LOCK_MS * 1_000_000
        self.record['deadline_ns'] = self.deadline_ns
        self.requested = True  # A lost acknowledgement may still mean lock ownership.
        self.child.stdin.write(b'LOCK\n')
        self.child.stdin.flush()
        self.record['ack'] = self.event('locked')
        assert requested <= self.record['ack']['observed_ns'] < self.deadline_ns
        self.record.update(after=production_status(self.config), observed_ns=time.monotonic_ns())
        cancelled_status(gate, self.record['after'])
        self.locked()
        self.record['result'] = 'acknowledged'
        return lane


def drag_once(client, spec, prepared, record, save):
    """Journal before transport; all exceptions preserve a single unknown attempt."""
    assert prepared['target'] == spec['target'] and prepared['session'] == spec['name']
    assert record['outcome'] == 'unknown' and record['replayed'] is False
    record['request'] = {**prepared['arguments'], **spec['target'],
                         'session': spec['name'], 'delivery_mode': 'background'}
    save('drag-action.json', record)
    try:
        record['dispatch_ns'] = time.monotonic_ns()
        assert 0 <= record['dispatch_ns'] - prepared['prepared_ns'] <= MAX_GROUNDING_AGE_NS, 'drag grounding expired; no input sent'
        record.update(outcome='response', response=client.tool('drag', record['request']))
    except Exception as error:
        record['error'] = str(error)
    finally:
        record['observed_ns'] = time.monotonic_ns()
        save('drag-action.json', record)
    return record


def transition_evidence(page, *, stopped=False):
    """Never filter or relabel the canonical analysis, including its failures."""
    analysis = analyze(page if stopped else stopped_prefix(page))
    primary = [row for row in page['events'] if row[5] == 0 and row[2] not in ('start', 'stop')]
    return {'continuous_primary_isolation': 'unproven', 'raw_primary_analysis': analysis,
        'analysis_uses_end_sentinel': not stopped, 'primary_transition_events': primary,
        'classification': 'observed_transition_events_not_attributed_to_lock_or_driver'}


def verify_cancelled(boundary, record, action):
    assert record['result'] == 'acknowledged'
    prefix, lane = record['prefix'], record['lane']
    assert set(active_drags(prefix)) == {lane}
    assert all(row[5] in (0, lane) for row in prefix['events']), 'unowned synthetic activity'
    assert all(row[2] in ('agent_approved', 'agent_admitted', 'agent_drag_start',
                         'pointer_enter', 'pointer_motion', 'pointer_button')
               for row in prefix['events'] if row[5] == lane), 'unexpected input before LOCK'
    held_status(record['gate_status'], lane)
    requested, deadline = record['requested_ns'], record['deadline_ns']
    assert 0 <= requested - prefix['events'][-1][1] <= 250_000_000
    assert 0 <= requested - record['status_started_ns'] <= 250_000_000
    assert 0 <= requested - record['ready']['observed_ns'] < 2_500_000_000
    assert record['ack']['event'] == 'locked'
    assert requested <= record['ack']['observed_ns'] <= record['observed_ns'] < deadline
    assert analyze(stopped_prefix(prefix))['result'] == 'passed', 'pre-LOCK isolation failed'
    tail = trace_interval(prefix, boundary)
    synthetic = [row for row in tail if row[5] in (1, 2)]
    assert all(row[5] == lane for row in synthetic), 'cleanup crossed lanes'
    cancels = [row for row in synthetic if row[2] == 'agent_cancel']
    assert len(cancels) == 1 and requested <= cancels[0][1] < deadline, 'missing post-LOCK cancellation'
    cancel = cancels[0]
    assert all(row[2] in ('agent_cancel', 'pointer_button', 'pointer_leave', 'keyboard_leave',
                          'pointer_motion', 'pointer_enter') for row in synthetic), 'extra action or false completion'
    assert not any(row[2] in ('pointer_motion', 'pointer_enter') and row[0] > cancel[0]
                   for row in synthetic), 'input continued after cancellation'
    releases = [row for row in synthetic if row[2] == 'pointer_button']
    assert len(releases) == 1 and releases[0][6] == 0 and releases[0][0] > cancel[0], 'missing own-seat release'
    assert releases[0][1] <= record['observed_ns'] < deadline, 'release was not observed before cleared lock status'
    released_synthetic_input(boundary)
    cancelled_status(record['gate_status'], record['after'])
    # A transport-unknown response cannot establish the production reason.
    assert action['outcome'] == 'response' and action['replayed'] is False, 'unknown drag outcome; no replay'
    check_response(action['response'], {'kind': 'partial'})
    content = action['response']['structuredContent']
    assert content['delivery']['mode'] == 'background' and content['delivery']['delivered_count'] > 0
    assert content.get('reason') == 'desktop_changed', 'lock cancellation reason is not established'
    assert action['dispatch_ns'] <= prefix['events'][-1][1] <= action['observed_ns'] < deadline
    return {'result': 'verified', 'reason': 'desktop_changed', 'lane': lane,
        'cancel_sequence': cancel[0], 'release_sequence': releases[0][0],
        'synthetic_release': 'verified', 'saved_app_effect': 'unproven',
        'continuous_primary_isolation': 'unproven'}


def verify_transition_end(boundary, stopped):
    analysis = analyze(stopped)
    assert analysis.get('telemetry_complete') is True, analysis
    assert stopped['events'][:boundary['count']] == boundary['events'], 'transition trace history changed'
    assert not any(row[5] in (1, 2) for row in stopped['events'][boundary['count']:]), 'synthetic activity after cancellation boundary'
    released_synthetic_input(stopped)
    return transition_evidence(stopped, stopped=True)


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2))
    report = {'result': 'failed', 'scope': 'active-session-lock-cancellation',
        'cancellation': {'result': 'unproven'}, 'recovery': {'result': 'unproven'},
        'continuous_isolation_across_transitions': 'unproven',
        'physical_hardware': False, 'full_desktop_matrix': False, 'runtime_pids': []}
    clients, fixture, trace, grab, pool, future = [], None, None, None, None, None
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
        save(phase + '-trace-initial.json', page)
        return page
    try:
        plan = json.loads(args.plan.read_text())
        save('plan.json', plan)
        validate_plan(plan)
        fixture = ActiveLockFixture(plan, args)
        origin = provenance(args, plan)
        for name in (Path(__file__).name, 'production_active_lock_proof_test.py',
                     'production_lock_refusal_proof.py', 'production_lock_refusal_proof_test.py',
                     'session_lock_fixture.c', 'production_session_fault_proof.py',
                     'production_cancel_proof.py', 'desktop_faults.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        origin['lock_fixture'] = plan['lock_fixture']
        save('provenance.json', origin)
        spec = plan['agents'][0]
        observer, actor = launch('observer'), launch('agent')
        assert not actor.tool('start_session', {'session': spec['name']}).get('isError')
        assert state(args.foreground_journal)['held'] is False, 'lock setup requires released primary fixture'
        prepared = prepare_drag(actor, spec)
        save('drag-grounding.json', prepared)
        trace = connect_trace(args.trace_socket, fixture.config)
        initial = start_trace()
        fixture.arm()
        action = report['action'] = {'outcome': 'unknown', 'replayed': False,
                                    'prepared_ns': prepared['prepared_ns'], 'runtime_pid': actor.process.pid}
        save('drag-action.json', action)
        pool = ThreadPoolExecutor(max_workers=1)
        future = pool.submit(drag_once, actor, spec, prepared, action, save)
        fixture.inject(trace, initial, future)
        future.result(timeout=3)
        fixture.locked()
        boundary = trace.collect()
        save('cancellation-boundary.json', boundary)
        save('cancellation-transition-analysis.json', transition_evidence(boundary))
        report['cancellation'] = verify_cancelled(boundary, fixture.record, action)
        # Do not close the actor to manufacture a release before this oracle.
        close_owned(actor)
        settle_locked(fixture)
        stable_status(fixture.record['after'], production_status(fixture.config))
        quiet = trace.collect()
        save('locked-quiescent-prefix.json', quiet)
        assert not any(row[5] in (1, 2) for row in trace_interval(boundary, quiet)), 'input after cancellation'
        restoration = fixture.restore()
        save('restoration.json', restoration)
        trace.exchange('TRACE_STOP')
        stopped = trace.collect()
        save('transition-trace.json', stopped)
        tracing = False
        save('transition-analysis.json', transition_evidence(stopped, stopped=True))
        report['transition'] = verify_transition_end(boundary, stopped)
        fixture.check_targets()
        power(fixture.config, True)
        # Intentional foreground focus/grab setup is outside both action traces.
        setup = {'before': wm(), 'started_ns': time.monotonic_ns(), 'continuous_isolation': 'unproven'}
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
        primary, baseline = wm(), state(args.foreground_journal)
        assert primary['pid'] == plan['foreground']['pid'] and int(primary['address'], 16) == plan['foreground']['window_id']
        assert primary['cursor'] == {'x': x, 'y': y} and baseline['held']
        setup.update(after=primary, observed_ns=time.monotonic_ns())
        save('foreground-setup.json', setup)
        fixture.check_targets()
        stable_status(restoration['after'], production_status(fixture.config))
        phase = 'recovery'
        initial = start_trace()
        fresh = launch('recovery')
        assert actor.process.poll() is not None and fresh.process.pid != actor.process.pid
        recovery = report['recovery']
        recovery.update(runtime_pid=fresh.process.pid, previous_runtime_pid=actor.process.pid, replayed=False)
        spec = {**spec, 'name': spec['name'] + '-recovery'}
        assert not fresh.tool('start_session', {'session': spec['name']}).get('isError')
        grounding = prepare_recovery(fresh, spec, plan['recovery']['pointer_stages'])
        prepared_ns, arguments, oracle = (grounding[key] for key in ('prepared_ns', 'arguments', 'oracle'))
        recovery['stage'] = spec['pointer_stage'] = grounding['stage']
        save('recovery-grounding.json', grounding)
        require_primary_active(grab, deadline)
        recovery['action'] = {'outcome': 'unknown', 'replayed': False, 'prepared_ns': prepared_ns}
        response = click_once(fresh, {**arguments, **spec['target'], 'session': spec['name'],
            'delivery_mode': 'background'}, recovery['action'], save, 'recovery-action.json')
        check_response(response, {'kind': 'dispatched'})
        after = grounded_snapshot(observer, spec['target'], spec, session=False)
        save('recovery-after.json', after)
        recovery['app_effect'] = pointer_grounding.verify(after, pointer_grounding.read_pixels(after['proof_image']), oracle)
        prefix = trace.collect()
        save('recovery-trace-prefix.json', prefix)
        recovery['trace'] = verify_recovery_trace(initial, prefix, capacity_lane(initial, prefix, 'click'), 'click')
        close_owned(fresh)
        trace.exchange('TRACE_STOP')
        stopped = trace.collect()
        save('recovery-trace.json', stopped)
        tracing = False
        recovery['isolation'] = verify_recovery_cleanup(prefix, stopped)
        released_synthetic_input(stopped)
        require_primary_active(grab, deadline)
        current = state(args.foreground_journal)
        save('recovery-primary-readback.json', {'before': primary, 'after': wm(), 'foreground_before': baseline, 'foreground_after': current})
        assert wm() == primary and all(current[k] == baseline[k] for k in ('clicks', 'keys', 'scroll', 'held'))
        fixture.check_targets()
        final = production_status(fixture.config)
        save('final-status.json', final)
        old, new = lanes(restoration['after'], cleared=True), lanes(final, cleared=True)
        assert set(old) == set(new) == {0, 1}
        assert sum(new[k]['dispatches'] - old[k]['dispatches'] for k in old) == 1
        for key in old:
            assert new[key]['dispatches'] >= old[key]['dispatches'] and new[key].get('reserved') is False
            assert all(new[key][field] == old[key][field] for field in ('epoch', 'desktop_generation'))
        recovery['result'] = 'verified'
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        operations = []
        if fixture:
            operations.append(('graceful_unlock', lambda: fixture.restore(cleanup=True)))
        operations += [(f'close_runtime_{i}', lambda c=c: close_owned(c)) for i, c in enumerate(clients)]
        if future:
            def preserve_action():
                # Retain the original attempt, even if closing its runtime was
                # needed to unblock transport. Cleanup cannot certify it.
                future.result(timeout=3)
                save('drag-action.json', report['action'])
            operations.append(('preserve_action', preserve_action))
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
        if grab:
            def release_primary():
                if grab.poll() is None:
                    grab.terminate()
                stop_process(grab)
                wait_for(lambda: not state(args.foreground_journal)['held'], timeout=3)
            operations.append(('release_primary', release_primary))
        errors = cleanup_all(operations)
        if fixture:
            save('lock-fixture.json', fixture.record)
        save('cleanup.json', {'errors': errors})
        if errors:
            report['result'] = 'failed'
        save('result.json', report)
    print(json.dumps(report), flush=True)
    return 0 if report['result'] == 'passed' else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('driver', 'plugin', 'source', 'primary-grab', 'plan', 'evidence',
                 'foreground-journal', 'trace-socket', 'lock-fixture'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--source-sha', required=True)
    raise SystemExit(run(parser.parse_args()))
