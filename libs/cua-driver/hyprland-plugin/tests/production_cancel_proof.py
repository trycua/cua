"""Bounded native cancellation proof; deterministic tests do not certify native behavior.

Only normal direct Driver calls send app input. The trace connection is read-only
apart from TRACE_START/TRACE_STOP. Kill one exact owned runtime only after fresh
telemetry proves two active drags; preserve unknown delivery without replay.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import math
from pathlib import Path
import subprocess
import time

from driver_input_live import state, wait_for, wm
from primary_trace import Trace, analyze
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
import production_pointer_grounding as pointer_grounding
from production_realapp_proof import (app_process_identity, capacity_lane, check_response,
                                     primary_acknowledgement, provenance, trace_interval,
                                     PRIMARY_LIFETIME_MS, require_primary_active)
from realapp_proof import cleanup_all, released_synthetic_input


PROFILE = {'mode': 'unrestricted', 'acknowledge_unrestricted': True}
DRAG_KEYS = {'from_x', 'from_y', 'to_x', 'to_y', 'duration_ms'}
POINTER_STAGES = {'calc': 'select_range', 'inkscape': 'move_rectangle'}
RECOVERY_STAGES = {'calc': {'click_a1', 'click_b2'}, 'inkscape': {'scroll_down'}}
MAX_GROUNDING_AGE_NS = 5_000_000_000
# Leave margin over the observed 25–30 ms lane-admission interval. This is not
# a worst-case scheduling bound: call_drag still checks the actual age.
GROUNDING_DISPATCH_RESERVE_NS = 250_000_000
MAX_GROUNDING_ATTEMPTS = 2
# max_elements counts all visited AT-SPI nodes, not only emitted controls.
# The native 2,000-node run retained the object row but stopped 14 rendered
# tree lines before the selection status. Allow 500 more visited nodes while
# keeping the trailing menu tree bounded; native coverage must verify this.
# Keep depth uncapped (the object row is deeply nested). Missing oracle
# evidence still fails closed, as does the unchanged five-second age limit.
POINTER_SNAPSHOT_LIMITS = {'inkscape': {'max_elements': 2500}}


def validate_plan(plan):
    assert plan['purpose'] == 'cancellation'
    assert type(plan['kill_agent']) is int and plan['kill_agent'] in (0, 1)
    assert plan.get('termination_signal', 'SIGKILL') in ('SIGKILL', 'SIGTERM'), 'unsupported termination signal'
    assert len(plan['agents']) == 2
    assert {spec['app'] for spec in plan['agents']} == {'calc', 'inkscape'}
    targets = [plan['foreground'], *(spec['target'] for spec in plan['agents'])]
    assert len({target['pid'] for target in targets}) == 3, 'need three separate app processes'
    for target in targets:
        assert set(target) == {'pid', 'window_id'}
        assert all(type(value) is int and value > 0 for value in target.values())
    for spec in plan['agents']:
        assert isinstance(spec['name'], str) and spec['name']
        assert spec.get('profile', PROFILE) == PROFILE, 'this run requires unrestricted plus explicit bypass'
        assert set(spec['bounds']) == {'x', 'y', 'width', 'height'}
        assert all(type(v) in (int, float) and math.isfinite(v) for v in spec['bounds'].values())
        assert spec['bounds']['width'] > 0 and spec['bounds']['height'] > 0
        drag = spec['drag']
        if 'pointer_stage' in spec:
            assert spec['pointer_stage'] == POINTER_STAGES[spec['app']] and drag == {}, \
                'pointer cancellation derives its drag from the fresh image'
            continue
        assert set(drag) == DRAG_KEYS, 'drag cannot override target/session/delivery'
        assert all(type(v) in (int, float) and math.isfinite(v) for v in drag.values())
        assert type(drag['duration_ms']) is int and 1000 <= drag['duration_ms'] <= 2000
        assert (drag['from_x'], drag['from_y']) != (drag['to_x'], drag['to_y'])
    point = plan['primary_point']
    assert len(point) == 2 and all(type(v) is int for v in point)
    if 'recovery' in plan:
        recovery = plan['recovery']
        assert isinstance(recovery, dict) and set(recovery) == {'pointer_stage'}
        assert all('pointer_stage' in spec for spec in plan['agents']), 'recovery needs both app-effect oracles'
        victim = plan['agents'][plan['kill_agent']]
        assert recovery['pointer_stage'] in RECOVERY_STAGES[victim['app']], 'recovery must be a new non-drag action'


def stopped_prefix(page):
    """Analyze a phase boundary without stopping or resetting the real trace."""
    trace_interval(page, page)
    last = page['events'][-1]
    return {**page, 'active': False, 'count': page['count'] + 1,
            'events': page['events'] + [[last[0] + 1, last[1], 'stop', *last[3:5], 0, 0]]}


def verify_recovery_trace(before, after, lane, tool):
    assert tool in ('click', 'scroll'), 'recovery must use a new non-drag action'
    events = trace_interval(before, after)
    assert capacity_lane(before, after, tool) == lane, 'recovery did not reacquire the victim lane'
    assert sum(row[2] == 'agent_admitted' for row in events) == 1, 'recovery admission is not unique'
    assert sum(row[2] == 'agent_action_end' for row in events) == 1, 'recovery completion is not unique'
    assert not any(row[2] in ('agent_cancel', 'agent_drag_start', 'agent_drag_end', 'keyboard_key')
                   for row in events), 'recovery replayed or sent unexpected input'
    buttons = [row[6] for row in events if row[2] == 'pointer_button']
    axes = [row for row in events if row[2] == 'pointer_axis']
    assert (buttons == [1, 0] and not axes) if tool == 'click' else (not buttons and axes), \
        'unexpected recovery input sequence'
    stopped = stopped_prefix(after)
    isolation = analyze(stopped)
    assert isolation['result'] == 'passed' and released_synthetic_input(stopped), isolation
    return {'result': 'verified', 'lane': lane, 'tool': tool, 'continuous_isolation': isolation}


def verify_recovery_cleanup(prefix, stopped):
    """Allow seat teardown after completion, never another synthetic action."""
    trace_interval(prefix, prefix)
    isolation = analyze(stopped)
    assert isolation['result'] == 'passed' and released_synthetic_input(stopped), isolation
    assert stopped['events'][:prefix['count']] == prefix['events'], 'trace history changed during recovery'
    cleanup_events = {'agent_cancel', 'pointer_leave', 'keyboard_leave'}
    assert all(row[2] in cleanup_events for row in stopped['events'][prefix['count']:]
               if row[5] in (1, 2)), 'unexpected synthetic activity after recovery'
    return isolation


def recover_once(client, observer, victim, sibling, spec, stage, trace, boundary, lane, save, result, guard=None):
    """One new action, never a continuation of the killed transport or gesture."""
    assert victim.failed and victim.process.poll() is not None, 'victim must be torn down before recovery'
    pids = assert_distinct_runtimes([client, sibling, observer])
    assert victim.process.pid not in pids, 'recovery reused the victim runtime'
    assert stage in RECOVERY_STAGES[spec['app']]
    fresh = {**spec, 'name': spec['name'] + '-recovery', 'pointer_stage': stage}
    result.update(runtime_pid=client.process.pid, victim_pid=victim.process.pid,
                  policy={'startup_profile': dict(PROFILE), 'managed_user_policy': 'inherited by new runtime'},
                  session=fresh['name'], replayed=False)
    response = client.tool('start_session', {'session': fresh['name']})
    assert not response.get('isError'), response
    identity = app_process_identity(spec['app'], spec['target']['pid'])
    started_ns = time.monotonic_ns()
    before = grounded_snapshot(client, fresh['target'], fresh)
    # Match prepare_drag: discovery precedes the observation; the complete
    # snapshot and pixel grounding remain inside the five-second limit.
    started_ns = before.get('proof_observation_started_ns', started_ns)
    arguments, oracle = pointer_grounding.action(
        before, pointer_grounding.read_pixels(before['proof_image']), spec['app'], stage)
    tool = pointer_grounding.STAGES[spec['app']][stage]
    assert tool in ('click', 'scroll'), 'recovery cannot replay the interrupted drag'
    result['grounding'] = {'snapshot': before, 'app_identity': identity, 'arguments': arguments,
                           'oracle': oracle, 'prepared_ns': started_ns, 'target': fresh['target']}
    save('recovery-grounding.json', result)
    if guard:
        guard()
    dispatch_ns = time.monotonic_ns()
    assert 0 <= dispatch_ns - started_ns <= MAX_GROUNDING_AGE_NS, 'recovery grounding expired; no input sent'
    result['action'] = {'outcome': 'unknown', 'replayed': False, 'dispatch_ns': dispatch_ns}
    try:
        response = client.tool(tool, {**arguments, **fresh['target'], 'session': fresh['name'],
                                     'delivery_mode': 'background'})
    except Exception as error:
        result['action']['error'] = str(error)
    else:
        result['action'].update(outcome='response', response=response)
    save('recovery-action.json', result['action'])
    # Preserve the observed result even if delivery classification or the app oracle fails.
    after = grounded_snapshot(observer, fresh['target'], fresh, session=False)
    save('recovery-after.json', after)
    assert result['action']['outcome'] == 'response', 'recovery delivery remains unknown; no replay'
    check_response(response, {'kind': 'dispatched'})
    result['app_effect'] = pointer_grounding.verify(
        after, pointer_grounding.read_pixels(after['proof_image']), oracle)
    page = trace.collect()
    save('recovery-prefix.json', page)
    result['trace'] = verify_recovery_trace(boundary, page, lane, tool)
    assert client.process.poll() is None and sibling.process.poll() is None, 'runtime exited during recovery'
    if guard:
        guard()
    result['result'] = 'verified'
    return page


def observation_runtime(client):
    """Snapshot counters belong to one direct process and its evidence directory."""
    assert_distinct_runtimes([client])
    return {'pid': client.process.pid, 'directory': str(client.directory.resolve(strict=True))}


def verify_fresh_observation(before, after, observer, *, after_ns):
    """Verify original observation chronology, without comparing unscoped counters."""
    assert after['proof_runtime'] == observation_runtime(observer), 'wrong observation runtime'
    assert before['snapshot_id'] and after['snapshot_id'], 'missing snapshot identity'
    assert (before['proof_runtime'], before['snapshot_id']) != (after['proof_runtime'], after['snapshot_id']), \
        'reused snapshot'
    assert Path(before['proof_image']).resolve() != Path(after['proof_image']).resolve(), 'reused image artifact'
    times = [before['proof_observation_started_ns'], before['proof_observation_finished_ns'],
             after_ns, after['proof_observation_started_ns'], after['proof_observation_finished_ns'],
             time.monotonic_ns()]
    assert all(type(value) is int and value >= 0 for value in times), 'invalid observation timestamp'
    assert times == sorted(times) and times[0] < times[3], 'stale or out-of-order observation'


def grounded_snapshot(client, target, spec=None, *, session=True):
    windows = client.tool('list_windows', {})
    assert not windows.get('isError'), windows
    matches = [w for w in windows['structuredContent']['windows'] if w.get('pid') == target['pid']]
    assert len(matches) == 1 and matches[0].get('window_id') == target['window_id'], 'stale target identity'
    pixels = spec is not None and 'pointer_stage' in spec
    runtime = observation_runtime(client)
    observation_started_ns = time.monotonic_ns()
    result = client.tool('get_window_state', {**target,
                         **(POINTER_SNAPSHOT_LIMITS.get(spec['app'], {}) if pixels
                            else {'max_elements': 100, 'max_depth': 6}),
                         **({'session': spec['name']} if spec and session else {})})
    assert not result.get('isError'), result
    observation_finished_ns = time.monotonic_ns()
    assert observation_runtime(client) == runtime, 'observation runtime changed'
    content = {**result['structuredContent'], 'proof_runtime': runtime,
               'proof_observation_started_ns': observation_started_ns,
               'proof_observation_finished_ns': observation_finished_ns}
    width, height = content.get('screenshot_width', 0), content.get('screenshot_height', 0)
    assert width > 0 and height > 0, 'missing grounding image'
    if spec:
        assert content['window_bounds'] == spec['bounds'], 'reviewed geometry is stale'
        if not pixels:
            for end in ('from', 'to'):
                assert 0 < spec['drag'][end + '_x'] < width and 0 < spec['drag'][end + '_y'] < height, \
                    'drag leaves the fresh snapshot'
    # A cached response must not gain a new age from this wrapper's clock.
    snapshot_id = content.get('snapshot_id')
    assert isinstance(snapshot_id, str) and snapshot_id, 'missing snapshot identity'
    seen = vars(client).setdefault('_proof_snapshot_ids', set())
    assert snapshot_id not in seen, 'reused snapshot in Driver runtime'
    seen.add(snapshot_id)
    if pixels:
        images = [row for row in result.get('content', []) if row.get('type') == 'image']
        assert len(images) == 1 and images[0].get('image_file'), 'missing exact snapshot image'
        path = client.directory / images[0]['image_file']
        assert path.resolve(strict=True).parent == client.directory.resolve(strict=True), 'image escapes evidence'
        images_seen = vars(client).setdefault('_proof_image_paths', set())
        assert path.resolve() not in images_seen, 'reused image artifact in Driver runtime'
        images_seen.add(path.resolve())
        content = {**content, 'proof_image': str(path)}
    return content


def prepare_drag(client, spec):
    """Ground once before either gesture starts; never reuse an earlier action's image."""
    started_ns = time.monotonic_ns()
    before = grounded_snapshot(client, spec['target'], spec)
    observation_started_ns = before.get('proof_observation_started_ns', started_ns)
    observed_ns = time.monotonic_ns()
    arguments, oracle = dict(spec['drag']), None
    if 'pointer_stage' in spec:
        arguments, oracle = pointer_grounding.action(
            before, pointer_grounding.read_pixels(before['proof_image']), spec['app'], spec['pointer_stage'])
    return {'snapshot': before, 'arguments': arguments, 'oracle': oracle,
            'target': dict(spec['target']), 'session': spec['name'],
            'prepared_ns': observation_started_ns,
            'timing': {'preparation_started_ns': started_ns,
                       'observation_started_ns': observation_started_ns, 'observation_finished_ns': observed_ns,
                       'grounding_finished_ns': time.monotonic_ns()}}


def pair_has_dispatch_budget(prepared):
    """Check the whole pair without extending either action's freshness bound."""
    assert len(prepared) == 2
    assert 0 < GROUNDING_DISPATCH_RESERVE_NS < MAX_GROUNDING_AGE_NS
    checked_ns = time.monotonic_ns()
    ages = [checked_ns - item['prepared_ns'] for item in prepared]
    for item, age in zip(prepared, ages):
        item.setdefault('timing', {}).update(pair_gate_ns=checked_ns, pair_grounding_age_ns=age)
    assert all(age >= 0 for age in ages), 'grounding timestamp is in the future'
    return all(age <= MAX_GROUNDING_AGE_NS - GROUNDING_DISPATCH_RESERVE_NS for age in ages)


def prepare_drags(clients, specs, save):
    """Observe independent apps concurrently before either timed action starts.

    Reserve time for the first drag's admission before dispatching its sibling.
    At most one paired observation refresh is allowed here, before ANY input
    attempt. It never retries a failed observation or an input action. The
    unchanged five-second gate in call_drag still checks each actual dispatch.
    """
    assert len(clients) == len(specs) == 2
    for attempt in range(1, MAX_GROUNDING_ATTEMPTS + 1):
        assert_distinct_runtimes(clients)
        with ThreadPoolExecutor(max_workers=2) as observations:
            pending = [observations.submit(prepare_drag, client, spec)
                       for client, spec in zip(clients, specs)]
            prepared = [future.result() for future in pending]
        for index, item in enumerate(prepared):
            # Preserve superseded observations as well as the final canonical
            # names consumed by the independent evidence verifier.
            save(f'agent-{index}-drag-grounding-attempt-{attempt}.json', item)
            save(f'agent-{index}-drag-grounding.json', item)
        ready = pair_has_dispatch_budget(prepared)
        if not ready:
            # If preparation raises, the runner never receives this pair to
            # re-save it during cleanup. Retain the failed gate's timing here.
            for index, item in enumerate(prepared):
                save(f'agent-{index}-drag-grounding-attempt-{attempt}.json', item)
                save(f'agent-{index}-drag-grounding.json', item)
        save(f'drag-grounding-attempt-{attempt}.json', {
            'attempt': attempt, 'checked_ns': prepared[0]['timing']['pair_gate_ns'],
            'grounding_ages_ns': [item['timing']['pair_grounding_age_ns'] for item in prepared],
            'dispatch_reserve_ns': GROUNDING_DISPATCH_RESERVE_NS, 'ready': ready,
            'input_attempted': False})
        if ready:
            return prepared
    raise AssertionError('paired drag grounding has insufficient dispatch time; no input sent')


def active_drags(page):
    """Reject incomplete telemetry before using its last observed lane state."""
    trace_interval(page, page)
    active, admitted, pressed = {}, set(), set()
    for seq, timestamp, kind, _x, _y, lane, value, *_surface in page['events']:
        if lane not in (1, 2):
            continue
        if kind == 'agent_admitted':
            assert lane not in admitted, 'multiple admissions before termination'
            admitted.add(lane)
        elif kind == 'agent_drag_start':
            assert lane in admitted and lane not in active, 'ambiguous drag admission'
            active[lane] = timestamp
        elif kind in ('agent_drag_end', 'agent_cancel'):
            raise AssertionError('drag ended before the termination gate')
        elif kind in ('agent_action_end', 'keyboard_key'):
            raise AssertionError('unexpected action before termination')
        elif kind == 'pointer_button':
            assert value == 1 and lane in active and lane not in pressed, 'drag is not held'
            pressed.add(lane)
    return {lane: start for lane, start in active.items() if lane in pressed}


def poll_active(trace, previous, lanes, pending, timeout=3):
    """A stale prefix or completed action must never authorize a process kill."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        started = time.monotonic()
        page = trace.collect()
        assert time.monotonic() - started <= 0.25, 'trace read too stale for termination'
        trace_interval(previous, page)
        active = active_drags(page)
        assert all(not future.done() for future in pending), 'action already returned'
        ready = set(active) == set(lanes) if lanes else len(active) == 1
        if ready:
            if len(active) == 1 or page['events'][-1][1] - max(active.values()) >= 100_000_000:
                return page, active
        previous = page
        time.sleep(0.01)
    raise AssertionError('no fresh active overlap within bounded wait')


def terminate_owned(victim, sibling, pending, signal='SIGKILL'):
    assert signal in ('SIGKILL', 'SIGTERM'), 'unsupported termination signal'
    assert_distinct_runtimes([victim, sibling])
    assert all(not future.done() for future in pending), 'action returned before termination'
    # Poison before killing: no further RPC, including a snapshot, may use it.
    victim.failed = True
    requested_ns = time.monotonic_ns()
    if signal == 'SIGKILL':
        victim.process.kill()
    else:
        victim.process.terminate()
    # A timeout fails this case. Cleanup may reap the exact child later, but
    # must not turn an ignored SIGTERM into a passing SIGKILL result.
    victim.process.wait(timeout=3)
    assert victim.process.poll() is not None, 'owned runtime was not reaped'
    assert sibling.process.poll() is None, 'sibling exited during termination'
    return {'pid': victim.process.pid, 'signal': signal, 'requested_ns': requested_ns,
            'reaped_ns': time.monotonic_ns()}


def close_owned(client):
    """Retain close failures while still reaping the exact child if it survives."""
    try:
        client.close()
    finally:
        if client.process.poll() is None:
            client.process.kill()
            client.process.wait(timeout=3)


def verify_cancellation(stopped, kill_prefix, victim_lane, sibling_lane):
    isolation = analyze(stopped)
    assert isolation.get('telemetry_complete') is True and isolation['result'] == 'passed', isolation
    assert {victim_lane, sibling_lane} == {1, 2}
    assert set(active_drags(kill_prefix)) == {1, 2}
    assert stopped['events'][:kill_prefix['count']] == kill_prefix['events'], 'trace history changed'
    tail = stopped['events'][kill_prefix['count']:]
    cancelled = [r for r in tail if r[2] == 'agent_cancel']
    completed = [r for r in tail if r[2] == 'agent_drag_end']
    assert len(cancelled) == 1 and cancelled[0][5] == victim_lane, 'cancellation crossed lanes or is missing'
    assert len(completed) == 1 and completed[0][5] == sibling_lane, 'sibling did not complete alone'
    assert cancelled[0][0] < completed[0][0], 'sibling completed before cancellation'
    assert not any(r[2] in ('agent_admitted', 'agent_drag_start', 'agent_action_end', 'keyboard_key')
                   for r in tail), 'unexpected extra action after termination'
    for lane in (victim_lane, sibling_lane):
        releases = [r for r in tail if r[5] == lane and r[2] == 'pointer_button']
        assert len(releases) == 1 and releases[0][6] == 0, 'missing own-lane button release'
        assert releases[0][0] > cancelled[0][0], 'release preceded observed cancellation'
        if lane == sibling_lane:
            assert releases[0][0] < completed[0][0], 'sibling release followed completion'
    assert released_synthetic_input(stopped)
    assert isolation['agent_drag_overlap_ms'] >= 100, 'insufficient traced overlap'
    return {'result': 'verified', 'victim_lane': victim_lane, 'sibling_lane': sibling_lane,
            'continuous_isolation': isolation, 'synthetic_cleanup': 'verified'}


def call_drag(client, spec, prepared=None, guard=None):
    prepared = prepare_drag(client, spec) if prepared is None else prepared
    assert prepared['target'] == spec['target'] and prepared['session'] == spec['name']
    if guard:
        guard()
    dispatch_ns = time.monotonic_ns()
    age = dispatch_ns - prepared['prepared_ns']
    prepared.setdefault('timing', {}).update(dispatch_attempt_ns=dispatch_ns, grounding_age_ns=age)
    assert 0 <= age <= MAX_GROUNDING_AGE_NS, 'prepared drag snapshot expired; no input sent'
    try:
        response = client.tool('drag', {**prepared['arguments'], **spec['target'],
                               'session': spec['name'], 'delivery_mode': 'background'})
    except Exception as error:
        return {'outcome': 'unknown', 'error': str(error), 'replayed': False}
    return {'outcome': 'response', 'response': response, 'replayed': False}


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2))
    report = {'result': 'failed', 'scope': 'native-runtime-cancellation', 'actions': {},
              'cancellation': 'unproven', 'reacquisition': 'unproven',
              'sibling_pointer_delivery': 'unproven',
              'saved_app_effects': 'unproven', 'full_desktop_matrix': False}
    clients, observer, grab, trace, pool = [], None, None, None, None
    futures, kill_prefix, lanes = {}, None, None
    cancellation_boundary = recovery_prefix = None
    prepared = []
    primary_before = baseline = None
    deadline = None
    def guard():
        require_primary_active(grab, deadline)
    try:
        plan = json.loads(args.plan.read_text())
        save('plan.json', plan)
        validate_plan(plan)
        assert args.trace_socket and args.trace_socket.name in ('cua-input-v3.sock', 'cua-input-v3-2.sock'), \
            'cancellation requires continuous v3 trace'
        origin = provenance(args, plan)
        for name in ('production_cancel_proof.py', 'production_cancel_proof_test.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        save('provenance.json', origin)
        def launch(name):
            directory = args.evidence / name
            directory.mkdir()
            return DirectMCP(args.driver, directory, PROFILE)
        for index, spec in enumerate(plan['agents']):
            clients.append(launch(f'agent-{index}'))
            result = clients[-1].tool('start_session', {'session': spec['name']})
            assert not result.get('isError'), result
            grounded_snapshot(clients[-1], spec['target'], spec)
        observer = launch('observer')
        report['driver_processes'] = assert_distinct_runtimes([*clients, observer])
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
                                 str(desktop['screen_height']), str(PRIMARY_LIFETIME_MS)], stdout=subprocess.PIPE, text=True)
        assert primary_acknowledgement(grab.stdout) == 'HELD\n'
        wait_for(lambda: state(args.foreground_journal)['held'])
        grounded_snapshot(observer, plan['foreground'])
        primary_before, baseline = wm(), state(args.foreground_journal)
        assert primary_before['pid'] == plan['foreground']['pid'] and baseline['held']
        trace = Trace(args.trace_socket)
        assert trace.hello['protocol'] == 3
        trace.exchange('TRACE_START')
        initial = trace.collect()
        assert not active_drags(initial) and not any(r[5] in (1, 2) for r in initial['events']), 'trace is not quiet'
        victim, sibling = plan['kill_agent'], 1 - plan['kill_agent']
        pool = ThreadPoolExecutor(max_workers=2)
        # Complete both app observations before starting either timed gesture.
        # The exact images, derived coordinates, and oracles are retained before
        # any dispatch. Expired grounding fails without replaying an action.
        prepared = prepare_drags(clients, plan['agents'], save)
        guard()
        # Include evidence-write and primary-guard latency in the final paired
        # gate. Never re-observe after this point or after any input attempt.
        assert pair_has_dispatch_budget(prepared), 'paired drag grounding has insufficient dispatch time; no input sent'
        futures[victim] = pool.submit(call_drag, clients[victim], plan['agents'][victim], prepared[victim], guard)
        first, active = poll_active(trace, initial, None, list(futures.values()))
        victim_lane = next(iter(active))
        guard()
        futures[sibling] = pool.submit(call_drag, clients[sibling], plan['agents'][sibling], prepared[sibling], guard)
        kill_prefix, _ = poll_active(trace, first, {1, 2}, list(futures.values()))
        # No disk I/O or observation between the fresh trace gate and exact-child kill.
        guard()
        report['termination'] = terminate_owned(clients[victim], clients[sibling], list(futures.values()),
                                                plan.get('termination_signal', 'SIGKILL'))
        lanes = victim_lane, 3 - victim_lane
        save('termination-prefix.json', kill_prefix)
        for index, future in futures.items():
            report['actions'][str(index)] = future.result(timeout=5)
        killed = report['actions'][str(victim)]
        if killed['outcome'] == 'response':
            content = killed['response'].get('structuredContent', {})
            kind = 'unknown' if (content.get('delivery') or {}).get('mode') == 'unknown' else 'partial'
            check_response(killed['response'], {'kind': kind})
        surviving = report['actions'][str(sibling)]
        assert surviving['outcome'] == 'response', 'sibling delivery remains unknown'
        check_response(surviving['response'], {'kind': 'dispatched'})
        assert clients[sibling].process.poll() is None, 'sibling runtime exited'
        for index in (victim, sibling):
            spec = plan['agents'][index]
            # The observer uses its own fresh read after the killed connection is lost.
            after = grounded_snapshot(observer, spec['target'], spec, session=False)
            save(f'agent-{index}-after.json', after)
            guard()
            if index == victim and 'recovery' in plan:
                image = Path(after['proof_image'])
                report['interrupted_state'] = {
                    'snapshot': f'agent-{index}-after.json', 'image': str(image),
                    'snapshot_sha256': hashlib.sha256(
                        (args.evidence / f'agent-{index}-after.json').read_bytes()).hexdigest(),
                    'image_sha256': hashlib.sha256(image.read_bytes()).hexdigest(),
                    'action': killed, 'saved_document_effect': 'unproven', 'replayed': False}
                save('interrupted-state.json', report['interrupted_state'])
            if index == sibling and prepared[index]['oracle'] is not None:
                report['sibling_app_effect'] = pointer_grounding.verify(
                    after, pointer_grounding.read_pixels(after['proof_image']), prepared[index]['oracle'])
        if 'recovery' in plan:
            cancellation_boundary = trace.collect()
            save('cancellation-prefix.json', cancellation_boundary)
            report['cancellation'] = verify_cancellation(stopped_prefix(cancellation_boundary), kill_prefix, *lanes)
            assert clients[victim].failed and clients[victim].process.poll() is not None
            # Register the fresh runtime for unconditional cleanup before any RPC.
            clients.append(launch('recovery'))
            report['reacquisition'] = {'result': 'unproven', 'replayed': False}
            recovery_prefix = recover_once(
                clients[-1], observer, clients[victim], clients[sibling], plan['agents'][victim],
                plan['recovery']['pointer_stage'], trace, cancellation_boundary, victim_lane,
                save, report['reacquisition'], guard)
            interrupted = report['interrupted_state']
            for path, digest in ((args.evidence / interrupted['snapshot'], interrupted['snapshot_sha256']),
                                 (Path(interrupted['image']), interrupted['image_sha256'])):
                assert hashlib.sha256(path.read_bytes()).hexdigest() == digest, 'interrupted evidence changed'
        grounded_snapshot(observer, plan['foreground'])
        assert wm() == primary_before, 'primary cursor/focus/workspace changed'
        current = state(args.foreground_journal)
        assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held')), current
        guard()
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        operations = [(f'close_agent_{i}', lambda c=client: close_owned(c)) for i, client in enumerate(clients)]
        if kill_prefix is not None:
            operations.append(('preserve_termination_prefix', lambda: save('termination-prefix.json', kill_prefix)))
        if pool:
            # Closing owned transports first unblocks pending RPCs, including failed setup.
            def join_actions():
                for index, future in futures.items():
                    try:
                        report['actions'][str(index)] = future.result(timeout=5)
                    except Exception as error:
                        report['actions'][str(index)] = {'outcome': 'unknown', 'error': str(error), 'replayed': False}
                assert all(future.done() for future in futures.values()), 'action worker did not stop'
            operations.append(('join_actions', join_actions))
            operations.append(('shutdown_pool', lambda: pool.shutdown(wait=False, cancel_futures=True)))
        # Keep dispatch-attempt timing even when the freshness gate raises before
        # MCP input. Initial grounding was already saved before either action.
        for index, item in enumerate(prepared):
            operations.append((f'preserve_grounding_{index}',
                               lambda i=index, value=item: save(f'agent-{i}-drag-grounding.json', value)))
        if trace:
            def finish_trace():
                trace.exchange('TRACE_STOP')
                stopped = trace.collect()
                save('trace.json', stopped)
                guard()
                assert kill_prefix is not None and lanes, 'no termination evidence'
                if cancellation_boundary is None:
                    report['cancellation'] = verify_cancellation(stopped, kill_prefix, *lanes)
                else:
                    report['cancellation'] = verify_cancellation(stopped_prefix(cancellation_boundary), kill_prefix, *lanes)
                    prefix = recovery_prefix or cancellation_boundary
                    assert stopped['events'][:prefix['count']] == prefix['events'], 'trace history changed during recovery'
                    isolation = analyze(stopped)
                    assert isolation['result'] == 'passed' and released_synthetic_input(stopped), isolation
                    if recovery_prefix is not None:
                        isolation = verify_recovery_cleanup(recovery_prefix, stopped)
                    report['continuous_isolation'] = isolation
                if 'sibling_app_effect' in report:
                    # Bound the completed gesture to the admitted survivor lane.
                    # The victim may share its start coordinates and was canceled;
                    # neither it nor a later recovery action proves this delivery.
                    drag_trace = stopped_prefix(cancellation_boundary) if cancellation_boundary else stopped
                    report['sibling_pointer_delivery'] = pointer_grounding.verify_drag_trace(
                        drag_trace, prepared[sibling]['arguments'], report['sibling_app_effect'],
                        expected_lane=lanes[1])
                assert wm() == primary_before, 'primary changed during cleanup'
                current = state(args.foreground_journal)
                assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held')), current
                guard()
            operations += [('finish_trace', finish_trace), ('close_trace', trace.close)]
        def release_primary():
            if grab:
                if grab.poll() is None:
                    grab.terminate()
                stop_process(grab)
                wait_for(lambda: not state(args.foreground_journal)['held'])
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
