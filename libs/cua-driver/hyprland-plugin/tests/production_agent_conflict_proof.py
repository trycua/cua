"""Run one native v3 same-client passive-hover agent-conflict refusal cell.

CLI: --plan --evidence --driver --plugin --source --source-sha --trace-socket
--primary-grab --foreground-journal, as for production_primary_conflict_proof.py.
Run with python3.11 or newer on the exact prepared disposable VM. Input is sent
through normal Driver calls; the independent fixture holds the primary grab.
No app launch, config edit, signing, policy installation, or action replay.

Integration boundary: the primary-conflict plan shape with purpose=agent_conflict,
case=passive_hover_refusal, app_profile=inkscape-only, one Inkscape agent whose
pointer_stage is scroll_down|scroll_up (drag={}), refused={pointer_stage} set to
the opposite scroll, and recovery={pointer_stage} in scroll_visible|scroll_down|
scroll_up. vm, compositor, processes, foreground, primary_point and
package_versions are unchanged. The controller must leave the separate
foreground fixture ready for the independent 60-second primary grab, which this
runner holds for the whole cell with one continuous strict trace.

Sequence, one normal Driver call per stage, distinct runtimes A, B and C:
A performs one freshly grounded scroll and keeps its passive hover
(reserved, pointer_focus, no lease). B claims the other lane and its TARGET on
the exact same window must be refused agent_target_busy with zero synthetic
events, unchanged geometry/rectangle and A's state retained. B closes first,
then A; A's orphan hover remains while its reservation clears. C must reacquire
the original owner lane for a new freshly grounded scroll with an application
effect. Recovery on the other lane fails closed. Active-lease conflicts,
same-process sibling-window claims and other-lane recovery are UNPROVEN here.
Portable tests are preparation only; native execution is a separate gate.
"""
import argparse
from production_app_smoke import add_provenance_arguments
import hashlib
import json
from pathlib import Path
import subprocess
import time

from driver_input_live import state, wait_for
from primary_trace import analyze
from production_cancel_proof import (MAX_GROUNDING_AGE_NS, PROFILE, close_owned, grounded_snapshot,
    validate_app_profile, verify_fresh_observation, verify_recovery_cleanup, verify_recovery_trace)
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
import production_pointer_grounding as pointer_grounding
from production_primary_conflict_proof import ExactDesktop, clear_status, validate_plan as primary_plan
from production_realapp_proof import (PRIMARY_LIFETIME_MS, app_process_identity, assert_no_dispatch,
    capacity_lane, check_response, primary_acknowledgement, provenance, require_primary_active, trace_interval)
from realapp_proof import cleanup_all, released_synthetic_input

SCROLL_STAGES = ('scroll_down', 'scroll_up')
OPPOSITE = {'scroll_down': 'scroll_up', 'scroll_up': 'scroll_down'}
RECOVERY_STAGES = ('scroll_visible', *SCROLL_STAGES)
STAGES = ('owner', 'refused', 'recovery')
REFUSAL = {'kind': 'refused', 'reason': 'agent_target_busy'}


def validate_plan(plan):
    assert plan['purpose'] == 'agent_conflict' and plan['case'] == 'passive_hover_refusal'
    assert plan.get('app_profile') == 'inkscape-only', 'this cell qualifies the Inkscape-only profile'
    assert 'fault' not in plan, 'active faults are not covered by this runner'
    assert len(plan['agents']) == 1, 'one target window, one agent'
    spec = plan['agents'][0]
    assert spec['app'] == 'inkscape' and spec['drag'] == {}
    assert spec['pointer_stage'] in SCROLL_STAGES, 'owner needs a fixed visible scroll'
    assert set(plan['refused']) == {'pointer_stage'}
    assert plan['refused']['pointer_stage'] == OPPOSITE[spec['pointer_stage']], 'refused scroll must be opposite'
    assert set(plan['recovery']) == {'pointer_stage'} and plan['recovery']['pointer_stage'] in RECOVERY_STAGES
    # Reuse the profile and exact vm/compositor/process identity contracts;
    # substitute the stages they require, keeping the scroll stages validated above.
    validate_app_profile({**plan, 'recovery': {'pointer_stage': 'scroll_down'}}, require_drag=False)
    primary_plan({**plan, 'purpose': 'primary_conflict', 'case': 'initial_refusal',
                  'agents': [{**spec, 'pointer_stage': 'move_rectangle'}], 'recovery': {'pointer_stage': 'scroll_down'}})


def lane_rows(status):
    rows = {row['lane']: row for row in status['input']['lanes']}
    assert set(rows) == {0, 1}
    return rows


def verify_owner_status(status, owner_lane, *, peer_reserved=False):
    """A live owner keeps passive hover on its lane without any input authority."""
    clear_status(status)
    assert owner_lane in (0, 1)
    rows = lane_rows(status)
    assert rows[owner_lane]['reserved'] is True and rows[owner_lane]['pointer_focus'] is True, 'owner hover lost'
    other = rows[1 - owner_lane]
    assert other['reserved'] is peer_reserved, 'unexpected peer reservation'
    assert other['pointer_focus'] is False, 'peer lane gained pointer focus'
    return status


def verify_refusal_status(status, response, owner_lane):
    """B's CLAIM survives the TARGET refusal until EOF, but grants nothing."""
    check_response(response, REFUSAL)
    lane = response['structuredContent'].get('lane')
    assert type(lane) is int and lane == 1 - owner_lane, 'refusal did not come from the other lane'
    return verify_owner_status(status, owner_lane, peer_reserved=True)


def verify_orphan_status(status, owner_lane):
    """After the owner's EOF only its inert hover remains; nothing is reserved."""
    clear_status(status, unreserved=True, allow_passive=True)
    rows = lane_rows(status)
    assert rows[owner_lane]['pointer_focus'] is True, 'orphan hover was retired without a fresh TARGET'
    assert rows[1 - owner_lane]['pointer_focus'] is False, 'other lane gained pointer focus'
    return status


def verify_refusal(before, after, response, owner_lane):
    check_response(response, REFUSAL)
    lane = response['structuredContent'].get('lane')
    assert type(lane) is int and lane == 1 - owner_lane, 'refusal did not come from the other lane'
    # Rejects every synthetic row in the call interval, including the owner's
    # pointer_leave: a TARGET refusal must not evict protected peer hover.
    assert_no_dispatch(before, after)


def verify_no_effect(after, image, oracle):
    """The refused scroll must leave the reviewed rectangle and document unchanged."""
    assert oracle['app'] == 'inkscape' and oracle['stage'] in SCROLL_STAGES
    rectangle = pointer_grounding.blue_rectangle(after, image)
    previous = oracle['rectangle']
    assert all(abs(rectangle[key] - previous[key]) <= 1 for key in ('x', 'y', 'w', 'h')), 'refused scroll moved canvas'
    geometry = pointer_grounding.inkscape_geometry(after, allow_transform_center=True)
    assert geometry == oracle['geometry'], 'refused scroll changed document geometry'
    return {'verified': True, 'scope': 'fresh-snapshot-no-pointer-effect', 'rectangle': rectangle, 'geometry': geometry}


def verify_close_events(before, after, allowed):
    """Runtime EOF may only tear down; it never sends or replays input."""
    events = trace_interval(before, after)
    assert all(row[2] in allowed for row in events if row[5] in (1, 2)), 'runtime close changed synthetic state'
    assert not any(row[2] == 'agent_admitted' for row in events), 'runtime close admitted an agent'
    return {'synthetic_events': [row for row in events if row[5] in (1, 2)], 'allowed': sorted(allowed)}


def ground(actor, spec):
    """Ground one scroll on the acting runtime's fresh image; resolve scroll_visible before input."""
    started_ns = time.monotonic_ns()
    before = grounded_snapshot(actor, spec['target'], spec)
    image = pointer_grounding.read_pixels(before['proof_image'])
    stage = spec['pointer_stage']
    if stage == 'scroll_visible':
        stage = pointer_grounding.visible_inkscape_scroll_stage(before, image)
    assert stage in SCROLL_STAGES and pointer_grounding.STAGES[spec['app']][stage] == 'scroll'
    arguments, oracle = pointer_grounding.action(before, image, spec['app'], stage)
    return {'snapshot': before, 'arguments': arguments, 'oracle': oracle, 'stage': stage,
            'requested_stage': spec['pointer_stage'],
            'prepared_ns': before.get('proof_observation_started_ns', started_ns)}


def action(actor, observer, spec, stage, trace, guard, save, record, *, owner_lane=None):
    """One normal Driver scroll call; always retain its raw after-observation."""
    assert stage in STAGES and (owner_lane is None) is (stage == 'owner')
    prepared = ground(actor, spec)
    arguments = {**prepared['arguments'], **spec['target'], 'session': spec['name'], 'delivery_mode': 'background'}
    record.update(runtime_pid=actor.process.pid, grounding=prepared, tool='scroll', arguments=arguments,
                  expected='refused' if stage == 'refused' else 'dispatched', outcome='not_attempted', replayed=False)
    save(stage + '-action.json', record)
    record['trace_before'] = trace.collect()
    # Launch, session start and grounding must not touch either synthetic lane.
    assert_no_dispatch(record['boundary'], record['trace_before'])
    guard()
    assert_distinct_runtimes([actor, observer])
    record['dispatch_ns'] = time.monotonic_ns()
    assert 0 <= record['dispatch_ns'] - prepared['prepared_ns'] <= MAX_GROUNDING_AGE_NS, 'stale grounding'
    record['outcome'] = 'unknown'
    try:
        record['response'] = actor.tool('scroll', arguments)
        record['outcome'] = 'response'
    except Exception as error:
        record['error'] = str(error)
    finally:
        record['observed_ns'] = time.monotonic_ns()
        save(stage + '-action.json', record)
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
                save(stage + '-action.json', record)
    guard()
    assert_distinct_runtimes([actor, observer])
    assert record['dispatch_ns'] <= record['observed_ns'], 'action returned before dispatch'
    verify_fresh_observation(prepared['snapshot'], record['after'], observer, after_ns=record['observed_ns'])
    assert record['outcome'] == 'response', 'delivery unknown; never replay'
    image = pointer_grounding.read_pixels(record['after']['proof_image'])
    if stage == 'refused':
        verify_refusal(record['trace_before'], record['trace_after'], record['response'], owner_lane)
        record['no_effect'] = verify_no_effect(record['after'], image, prepared['oracle'])
    else:
        check_response(record['response'], {'kind': 'dispatched'})
        record['app_effect'] = pointer_grounding.verify(record['after'], image, prepared['oracle'])
        lane = capacity_lane(record['trace_before'], record['trace_after'], 'scroll')
        if stage == 'recovery':
            assert lane == owner_lane + 1, 'recovery did not reacquire the original owner lane'
        reported = record['response']['structuredContent'].get('lane')
        assert reported is None or reported == lane - 1, 'Driver diagnostic lane disagrees with trace lane'
        record['trace_verification'] = verify_recovery_trace(record['trace_before'], record['trace_after'], lane, 'scroll')
        record['lane'], record['status_lane'] = lane, lane - 1
    save(stage + '-action.json', record)


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2) + '\n')
    report = {'result': 'failed', 'scope': 'same-client-passive-hover-refusal-and-owner-lane-recovery',
              'active_lease_conflict': 'unproven', 'other_lane_recovery': 'unproven',
              'same_process_sibling_window': 'unproven', 'full_desktop_matrix': False,
              'physical_hardware': False, 'stages': {}}
    clients, observer, desktop, trace, grab = [], None, None, None, None
    tracing, primary, baseline, deadline, prefix = False, None, None, None, None
    def guard():
        assert desktop.primary(plan['foreground']) == primary, 'primary cursor/focus/workspace changed'
        current = state(args.foreground_journal)
        assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held')), 'foreground input changed'
        require_primary_active(grab, deadline)
    def boundary(name):
        nonlocal prefix
        page = trace.collect()
        previous, prefix = prefix, page
        save(name, page)
        return previous, page
    def finish_trace():
        nonlocal tracing
        trace.exchange('TRACE_STOP')
        tracing = False
        stopped = trace.collect()
        save('trace.json', stopped)
        checked = analyze(stopped)
        assert checked['result'] == 'passed' and released_synthetic_input(stopped), checked
        assert [row[2] for row in stopped['events'] if row[2] in ('start', 'stop')] == ['start', 'stop'], \
            'trace restarted within the cell'
        report['isolation'] = verify_recovery_cleanup(prefix, stopped) if prefix else checked
        guard()
        report['final_status'] = desktop.status(unreserved=True, allow_passive=True)
    def launch(name):
        directory = args.evidence / name
        directory.mkdir()
        value = DirectMCP(args.driver, directory, PROFILE)
        clients.append(value)
        return value
    def start(actor, spec):
        assert not actor.tool('start_session', {'session': spec['name']}).get('isError')
    try:
        plan = json.loads(args.plan.read_text())
        save('plan.json', plan)
        validate_plan(plan)
        spec = plan['agents'][0]
        desktop = ExactDesktop(plan)
        app_process_identity(spec['app'], spec['target']['pid'])
        origin = provenance(args, plan)
        for name in (Path(__file__).name, 'production_agent_conflict_proof_test.py',
                     'production_primary_conflict_proof.py', 'desktop_faults.py', 'production_cancel_proof.py',
                     'production_desktop_fault_proof.py', 'production_geometry_fault_proof.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        origin['ownership'] = {'vm': plan['vm'], 'compositor': plan['compositor'], 'processes': plan['processes']}
        save('provenance.json', origin)
        observer = launch('observer')
        trace = desktop.trace(args.trace_socket)
        report['preflight_status'] = desktop.status(unreserved=True)
        # The independent primary fixture holds the separate foreground client
        # for the whole cell; no agent action may change it.
        foreground = grounded_snapshot(observer, plan['foreground'])['window_bounds']
        result = observer.tool('get_desktop_state', {})
        assert not result.get('isError')
        screen = result['structuredContent']
        x, y = plan['primary_point']
        assert 0 < x < foreground['width'] and 0 < y < foreground['height']
        x, y = foreground['x'] + x, foreground['y'] + y
        assert 0 <= x < screen['screen_width'] and 0 <= y < screen['screen_height']
        desktop.guard()
        deadline = time.monotonic_ns() + PRIMARY_LIFETIME_MS * 1_000_000
        grab = subprocess.Popen([str(args.primary_grab), str(x), str(y), str(screen['screen_width']),
            str(screen['screen_height']), str(PRIMARY_LIFETIME_MS)], stdout=subprocess.PIPE, text=True)
        assert primary_acknowledgement(grab.stdout) == 'HELD\n'
        wait_for(lambda: state(args.foreground_journal)['held'], timeout=3)
        primary, baseline = desktop.primary(plan['foreground']), state(args.foreground_journal)
        assert primary['cursor'] == {'x': x, 'y': y}, 'primary fixture missed planned point'
        assert baseline['held']
        report.update(primary_before=primary, foreground_before=baseline)
        report['initial_status'] = desktop.status(unreserved=True)
        started_ns = time.monotonic_ns()
        # A lost acknowledgement can still mean tracing started; retain the
        # cleanup obligation before issuing the test-only command.
        tracing = True
        trace.exchange('TRACE_START')
        prefix = trace.collect()
        trace_interval(prefix, prefix)
        assert prefix['count'] == 1 and started_ns <= prefix['events'][0][1] <= time.monotonic_ns()
        report['initial_trace'] = prefix
        guard()

        owner = launch('owner')
        assert_distinct_runtimes([owner, observer])
        start(owner, spec)
        row = report['stages']['owner'] = {'boundary': prefix}
        action(owner, observer, spec, 'owner', trace, guard, save, row)
        prefix = row['trace_after']
        owner_lane = row['status_lane']
        row['post_action_status'] = desktop.raw_status()
        save('owner-action.json', row)
        verify_owner_status(row['post_action_status'], owner_lane)
        row['result'] = 'verified'
        save('owner-action.json', row)

        refused = launch('refused')
        assert_distinct_runtimes([owner, refused, observer])
        refused_spec = {**spec, 'name': spec['name'] + '-refused', 'pointer_stage': plan['refused']['pointer_stage']}
        start(refused, refused_spec)
        row = report['stages']['refused'] = {'boundary': prefix}
        action(refused, observer, refused_spec, 'refused', trace, guard, save, row, owner_lane=owner_lane)
        prefix = row['trace_after']
        # Retain the observed status before validation can raise. A live
        # refused connection still owns its CLAIM; EOF must clear it below.
        row['post_action_status'] = desktop.raw_status()
        save('refused-action.json', row)
        verify_refusal_status(row['post_action_status'], row['response'], owner_lane)
        assert_distinct_runtimes([owner, refused, observer])
        row['result'] = 'verified'
        save('refused-action.json', row)

        close_owned(refused)
        row = report['stages']['close_refused'] = {'runtime_pid': refused.process.pid}
        row['status'] = wait_for(lambda: verify_owner_status(desktop.raw_status(), owner_lane), timeout=2)
        row['teardown'] = verify_close_events(*boundary('close-refused-trace.json'), set())
        assert_distinct_runtimes([owner, observer])
        guard()
        save('close-refused.json', row)

        close_owned(owner)
        row = report['stages']['close_owner'] = {'runtime_pid': owner.process.pid}
        row['status'] = wait_for(lambda: verify_orphan_status(desktop.raw_status(), owner_lane), timeout=2)
        row['teardown'] = verify_close_events(*boundary('close-owner-trace.json'), {'agent_cancel'})
        guard()
        save('close-owner.json', row)

        recovery = launch('recovery')
        assert_distinct_runtimes([recovery, observer])
        assert len({owner.process.pid, refused.process.pid, recovery.process.pid, observer.process.pid}) == 4, \
            'reused runtime process'
        recovery_spec = {**spec, 'name': spec['name'] + '-recovery', 'pointer_stage': plan['recovery']['pointer_stage']}
        start(recovery, recovery_spec)
        row = report['stages']['recovery'] = {'boundary': prefix}
        action(recovery, observer, recovery_spec, 'recovery', trace, guard, save, row, owner_lane=owner_lane)
        prefix = row['trace_after']
        row['post_action_status'] = desktop.raw_status()
        save('recovery-action.json', row)
        verify_owner_status(row['post_action_status'], owner_lane)
        row['result'] = 'verified'
        save('recovery-action.json', row)

        close_owned(recovery)
        row = report['stages']['close_recovery'] = {'runtime_pid': recovery.process.pid}
        row['status'] = wait_for(lambda: desktop.status(unreserved=True, allow_passive=True), timeout=2)
        save('close-recovery.json', row)
        finish_trace()
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
    add_provenance_arguments(parser)
    raise SystemExit(run(parser.parse_args()))
