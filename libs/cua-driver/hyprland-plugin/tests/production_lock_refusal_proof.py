"""TEST ONLY: settled ext-session-lock refusal and fresh-runtime Calc recovery.

Use geometry-proof CLI flags plus --lock-fixture. The exact disposable guest
plan uses purpose=lock_refusal, fault={kind:lock}, the session-fault identity,
monitor and foreground_fixture fields, and lock_fixture={path,device,inode,uid,
sha256,source_sha256}. Hash the reviewed binary and repository C source. The
fixture is not a real authentication screen. Only its acknowledged protocol
unlock restores the test desktop. The controller requests it by closing stdin;
an unsolicited EOF/deadline unlock is cleanup, never proof.

A normal Driver screenshot BEFORE lock grounds one denied click in a NEW
runtime AFTER acknowledged lock. No active-lock cancellation is exercised.
Lock/unlock and deliberate foreground-grab setup are outside action traces:
continuous isolation across these transitions is explicitly UNPROVEN. Each
settled action has an unfiltered strict trace. Recovery is one newly grounded
click, with visible Calc effect under the restored foreground fixture grab.
No product wake/unlock policy, replay, source fixture edit, or native setup.
Portable tests are orchestration checks, never native certification.
"""
import argparse
from production_app_smoke import add_provenance_arguments
import hashlib
import json
import os
from pathlib import Path
import re
import select
import stat
import subprocess
import time

from desktop_faults import _identity
from driver_input_live import state, wait_for, wm
from primary_trace import analyze
from production_cancel_proof import (MAX_GROUNDING_AGE_NS, PROFILE, close_owned,
    grounded_snapshot, stopped_prefix, verify_recovery_cleanup, verify_recovery_trace)
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
from production_desktop_fault_proof import idle_lanes, pointer_cleanup
import production_pointer_grounding as pointer_grounding
from production_realapp_proof import (PRIMARY_LIFETIME_MS, capacity_lane, check_response,
    primary_acknowledgement, provenance, require_primary_active, trace_interval)
from production_session_fault_proof import (PROCESS_KEYS, SessionFault, connect_trace,
    guard_guest, lanes, power, production_status, validate_plan as session_plan,
    validate_fault_options)
from realapp_proof import cleanup_all, released_synthetic_input


LOCK_MS = 20000
# Measured lock/settle/fresh-runtime setup takes about one second.
REFUSAL_SETUP_RESERVE_NS = 1_250_000_000


def validate_plan(plan):
    assert plan['purpose'] == 'lock_refusal' and plan['fault']['kind'] == 'lock'
    validate_fault_options(plan['fault'], motion=False)
    session_plan({**plan, 'purpose': 'session_fault', 'fault': {**plan['fault'], 'kind': 'dpms'}})
    fixture = plan['lock_fixture']
    assert set(fixture) == {'path', 'device', 'inode', 'uid', 'sha256', 'source_sha256'}
    assert Path(fixture['path']).is_absolute() and Path(fixture['path']).name == 'session_lock_fixture'
    assert all(type(fixture[k]) is int and fixture[k] >= 0 for k in ('device', 'inode', 'uid'))
    assert fixture['uid'] == plan['compositor']['uid']
    assert all(re.fullmatch(r'[0-9a-f]{64}', fixture[k]) for k in ('sha256', 'source_sha256'))
    assert plan['identities']['foreground']['uid'] == plan['identities']['target']['uid'] == fixture['uid']
    assert len({plan['compositor']['pid'], *(p['pid'] for p in plan['identities'].values())}) == 3


def stable_status(before, after, *, advanced=False, policy='cleared'):
    # By default only a pre-transition baseline may contain an inert pointer.
    pointer_cleanup({'pointer_cleanup': policy})
    retained = policy == 'retained_inert'
    old = idle_lanes(before) if advanced or retained else lanes(before, cleared=True)
    new = idle_lanes(after) if retained else lanes(after, cleared=True)
    assert set(old) == set(new)
    for lane in old:
        if retained:
            assert old[lane]['pointer_focus'] is new[lane]['pointer_focus'], 'pointer presence changed'
        assert old[lane].get('reserved') is False and new[lane].get('reserved') is False
        assert old[lane]['epoch'] == new[lane]['epoch'], 'compositor lane replaced'
        assert old[lane]['dispatches'] == new[lane]['dispatches'], 'unexpected dispatch'
        a, b = old[lane]['desktop_generation'], new[lane]['desktop_generation']
        assert b > a if advanced else b == a, 'unexpected desktop generation'


def verify_runtimes(clients):
    assert len({client.process.pid for client in clients}) == len(clients), 'reused prior runtime PID'
    for client in clients:
        if client.closed:
            assert client.process.poll() is not None, 'closed runtime was not reaped'
    assert_distinct_runtimes([client for client in clients if not client.closed])


def settle_locked(fixture):
    """Observe a quiet locked primary for 100ms before the strict action trace."""
    samples = fixture.record['settling'] = []
    stable_since = None
    previous = None
    def sample():
        nonlocal stable_since, previous
        fixture.locked()
        current = {'primary': wm(), 'status': production_status(fixture.config)}
        stable_status(fixture.record['after'], current['status'], policy=pointer_cleanup(fixture.config))
        now = time.monotonic_ns()
        samples.append({**current, 'observed_ns': now})
        if current != previous:
            stable_since = now
        previous = current
        return current if now - stable_since >= 100_000_000 else None
    return wait_for(sample, timeout=2)


class LockFixture(SessionFault):
    """Reuse exact guest/boot/compositor/window/file guards, without DPMS IPC."""
    def __init__(self, plan, args):
        self.plan, self.args = plan, args
        self.config = {'disposable': plan['disposable'], 'vm': plan['vm'],
            'compositor': {key: plan['compositor'][key] for key in PROCESS_KEYS},
            'instance': plan['compositor']['instance'], 'monitors': plan['monitors']}
        self.child = None
        self.buffer = b''
        self.events = []
        self.record = {'result': 'unproven', 'events': self.events}
        for key in ('pointer_cleanup', 'min_motion_px'):
            if key in plan['fault']:
                self.config[key] = self.record[key] = plan['fault'][key]
        self.requested = False
        self.restored = False
        self.check_targets()
        power(self.config, True)
        self.check_binary()

    def check_binary(self):
        expected, path = self.plan['lock_fixture'], self.args.lock_fixture
        assert str(path) == expected['path'] and path.resolve(strict=True) == path
        info = path.lstat()
        assert stat.S_ISREG(info.st_mode) and os.access(path, os.X_OK)
        assert [info.st_dev, info.st_ino, info.st_uid] == [expected[k] for k in ('device', 'inode', 'uid')]
        assert hashlib.sha256(path.read_bytes()).hexdigest() == expected['sha256']
        source = self.args.source / 'libs/cua-driver/hyprland-plugin/tests/session_lock_fixture.c'
        assert source.resolve(strict=True) == source
        assert hashlib.sha256(source.read_bytes()).hexdigest() == expected['source_sha256']

    def check_running_binary(self):
        # A pathname alone cannot distinguish a replaced executable between
        # preflight and exec. Bind the actual mapped executable before LOCK.
        expected = self.plan['lock_fixture']
        assert self.child.poll() is None and _identity(self.child.pid) == self.identity
        with Path(f'/proc/{self.child.pid}/exe').open('rb') as stream:
            info = os.fstat(stream.fileno())
            assert stat.S_ISREG(info.st_mode)
            assert [info.st_dev, info.st_ino, info.st_uid] == [expected[k] for k in ('device', 'inode', 'uid')]
            assert hashlib.sha256(stream.read()).hexdigest() == expected['sha256']
        assert self.child.poll() is None and _identity(self.child.pid) == self.identity

    def read_event(self):
        if b'\n' not in self.buffer:
            if not select.select([self.child.stdout], [], [], 0)[0]:
                return None
            chunk = os.read(self.child.stdout.fileno(), 4096)
            assert chunk, 'fixture EOF without expected protocol acknowledgement'
            self.buffer += chunk
            assert len(self.buffer) <= 16384, 'oversized fixture event'
            if b'\n' not in self.buffer:
                return None
        line, self.buffer = self.buffer.split(b'\n', 1)
        event = json.loads(line)
        assert set(event) == {'event', 'observed_ns'}
        assert event['event'] in ('ready', 'locked', 'finished', 'unlocked')
        assert type(event['observed_ns']) is int and 0 < event['observed_ns'] <= time.monotonic_ns()
        assert not self.events or self.events[-1]['observed_ns'] <= event['observed_ns']
        self.events.append(event)
        return event

    def event(self, expected, timeout=2):
        event = wait_for(self.read_event, timeout=timeout)
        assert event['event'] == expected, f'unexpected fixture protocol event: {event}'
        return event

    def lock(self):
        self.check_targets()
        self.check_binary()
        assert self.child is None
        self.record['before'] = production_status(self.config)
        (self.args.evidence / 'pre-fault-status.json').write_text(json.dumps(self.record['before']))
        idle_lanes(self.record['before'])
        with (self.args.evidence / 'lock-fixture.stderr').open('wb') as log:
            self.child = subprocess.Popen([str(self.args.lock_fixture), str(LOCK_MS),
                str(self.config['compositor']['pid'])], stdin=subprocess.PIPE,
                stdout=subprocess.PIPE, stderr=log, start_new_session=True)
        self.identity = _identity(self.child.pid)
        assert self.identity['uid'] == self.config['compositor']['uid']
        assert self.identity['exe'] == str(self.args.lock_fixture)
        self.record['identity'] = self.identity
        self.event('ready')
        self.check_running_binary()
        self.check_targets()
        self.record['requested_ns'] = time.monotonic_ns()
        self.deadline_ns = self.record['requested_ns'] + LOCK_MS * 1_000_000
        self.requested = True  # Lost response may still mean lock ownership.
        self.child.stdin.write(b'LOCK\n')
        self.child.stdin.flush()
        self.record['ack'] = self.event('locked')
        assert self.record['requested_ns'] <= self.record['ack']['observed_ns'] < self.deadline_ns
        self.record['after'] = production_status(self.config)
        stable_status(self.record['before'], self.record['after'], advanced=True, policy=pointer_cleanup(self.config))
        self.locked()
        self.record['result'] = 'acknowledged'

    def locked(self):
        self.check_targets()
        power(self.config, True)
        assert self.child.poll() is None and _identity(self.child.pid) == self.identity
        assert time.monotonic_ns() + 1_000_000_000 < self.deadline_ns, 'lock deadline too near'
        assert self.read_event() is None, 'lock ended before explicit restoration'
        assert self.events[-1]['event'] == 'locked'

    def restore(self, *, cleanup=False):
        if not self.child or self.restored:
            return {'result': 'not_needed'}
        record = {'result': 'unproven', 'cleanup': cleanup, 'started_ns': time.monotonic_ns()}
        self.record['restoration'] = record
        # EOF is the fixture's bounded protocol-unlock request. Never signal or
        # kill the lock client, including when proof collection failed.
        try:
            if self.child.stdin and not self.child.stdin.closed:
                try:
                    if not cleanup:
                        self.locked()
                finally:
                    self.child.stdin.close()
            if self.requested:
                while not self.events or self.events[-1]['event'] != 'unlocked':
                    event = wait_for(self.read_event, timeout=3)
                    assert event['event'] == 'unlocked' or (cleanup and event['event'] == 'locked')
                    record['ack'] = event
                record.setdefault('ack', self.events[-1])
        finally:
            # Even malformed/missing acknowledgements must reap an exited
            # child. If lock acquisition is still pending, allow its own
            # deadline plus sync grace to finish; never signal the lock client.
            remaining = (getattr(self, 'deadline_ns', time.monotonic_ns()) - time.monotonic_ns()) / 1_000_000_000
            timeout = max(3, min(LOCK_MS / 1000 + 3, remaining + 3))
            record['exit_code'] = self.child.wait(timeout=timeout)
            self.child.stdout.close()
        assert record['exit_code'] == 0, 'fixture failed graceful restoration'
        self.restored = True
        guard_guest(self.config)
        record.update(after=production_status(self.config), observed_ns=time.monotonic_ns())
        if not cleanup:
            assert self.requested and record['started_ns'] <= record['ack']['observed_ns'] < self.deadline_ns
            stable_status(self.record['after'], record['after'], advanced=True, policy=pointer_cleanup(self.config))
        record['result'] = 'restored'
        return record


def verify_refusal(record):
    assert record['outcome'] == 'response' and record['replayed'] is False
    check_response(record['response'], {'kind': 'refused', 'reason': 'session_unavailable'})
    assert record['prepared_ns'] <= record['lock_ack_ns'] <= record['runtime_started_ns'] <= record['dispatch_ns'] <= record['observed_ns'] < record['deadline_ns']
    assert record['dispatch_ns'] - record['prepared_ns'] <= MAX_GROUNDING_AGE_NS
    stable_status(record['before'], record['after'], policy=pointer_cleanup(record))
    assert not any(row[5] in (1, 2) for row in trace_interval(record['trace_before'], record['trace_after'])), 'refused action emitted synthetic events'
    isolation = analyze(stopped_prefix(record['trace_after']))
    assert isolation['result'] == 'passed', isolation
    return {'result': 'verified', 'reason': 'session_unavailable', 'zero_dispatch': True, 'isolation': isolation}


def verify_refusal_cleanup(prefix, stopped):
    trace_interval(prefix, prefix)
    isolation = analyze(stopped)
    assert isolation['result'] == 'passed', isolation
    assert stopped['events'][:prefix['count']] == prefix['events'], 'refusal trace history changed'
    assert not any(row[5] in (1, 2) for row in stopped['events']), 'refused action emitted synthetic events'
    return isolation


def verify_inert_transition(initial, stopped):
    """Retain raw lock analysis without claiming primary transition isolation."""
    analysis = analyze(stopped)
    assert analysis.get('telemetry_complete') is True, analysis
    trace_interval(initial, initial)
    assert stopped['events'][:initial['count']] == initial['events'], 'transition trace history changed'
    assert not any(row[5] in (1, 2) for row in stopped['events'][initial['count']:]), 'synthetic activity while pointer must remain inert'
    return {'continuous_primary_isolation': 'unproven', 'raw_primary_analysis': analysis,
            'synthetic_pointer_continuity': 'verified'}


def click_once(client, arguments, record, save, name):
    """Persist the attempt before transport; an exception never permits replay."""
    assert record['outcome'] == 'unknown' and record['replayed'] is False
    record['request'] = arguments
    save(name, record)
    try:
        record['dispatch_ns'] = time.monotonic_ns()
        assert 0 <= record['dispatch_ns'] - record['prepared_ns'] <= MAX_GROUNDING_AGE_NS, 'grounding expired; no input sent'
        tool = record.get('tool', 'click')
        assert tool in ('click', 'scroll'), 'only a new non-drag action is allowed'
        response = client.tool(tool, arguments)
        record.update(outcome='response', response=response)
        return response
    except Exception as error:
        record['error'] = str(error)
        raise
    finally:
        record['observed_ns'] = time.monotonic_ns()
        save(name, record)


def prepare_click(client, spec, *, session=True):
    """Keep the full observation's original age, excluding earlier discovery."""
    snapshot = grounded_snapshot(client, spec['target'], spec, session=session)
    prepared_ns = snapshot['proof_observation_started_ns']
    assert type(prepared_ns) is int and 0 < prepared_ns <= time.monotonic_ns()
    arguments, oracle = pointer_grounding.action(snapshot,
        pointer_grounding.read_pixels(snapshot['proof_image']), spec['app'], spec['pointer_stage'])
    return {'snapshot': snapshot, 'arguments': arguments, 'oracle': oracle,
            'prepared_ns': prepared_ns, 'tool': pointer_grounding.STAGES[spec['app']][spec['pointer_stage']]}


def prepare_refusal_click(observer, spec, save):
    """Refresh successful grounding at most once, before starting the lock."""
    first = prepare_click(observer, spec, session=False)
    save('refusal-grounding-attempt-1.json', first)
    checked_ns = time.monotonic_ns()
    age_ns = checked_ns - first['prepared_ns']
    assert age_ns >= 0, 'future refusal grounding'
    refresh = age_ns + REFUSAL_SETUP_RESERVE_NS > MAX_GROUNDING_AGE_NS
    decision = {'checked_ns': checked_ns, 'initial_age_ns': age_ns,
        'reserve_ns': REFUSAL_SETUP_RESERVE_NS, 'max_age_ns': MAX_GROUNDING_AGE_NS,
        'refresh_requested': refresh, 'selected_attempt': None if refresh else 1}
    first.update(attempt=1, prelock_decision=decision)
    save('refusal-grounding-attempt-1.json', first)
    selected = first
    if refresh:
        # This is a new read-only observation, never an input or failure retry.
        selected = prepare_click(observer, spec, session=False)
        decision.update(selected_attempt=2, refresh_finished_ns=time.monotonic_ns())
        selected.update(attempt=2, prelock_decision=decision)
        save('refusal-grounding-attempt-2.json', selected)
        save('refusal-grounding-attempt-1.json', first)
    save('refusal-grounding.json', selected)
    return selected


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2))
    report = {'result': 'failed', 'scope': 'settled-session-lock-refusal',
        'active_lock_cancellation': 'unproven', 'continuous_isolation_across_transitions': 'unproven',
        'physical_hardware': False, 'full_desktop_matrix': False, 'runtime_pids': [],
        'recovery': {'result': 'unproven'}}
    clients, fixture, trace, grab, deadline = [], None, None, None, None
    tracing = False
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
        tracing = True  # A lost acknowledgement may still have started tracing.
        trace.exchange('TRACE_START')
        page = trace.collect()
        trace_interval(page, page)
        assert page['count'] == 1 and started <= page['events'][0][1] <= time.monotonic_ns()
        return page
    try:
        plan = json.loads(args.plan.read_text())
        save('plan.json', plan)
        validate_plan(plan)
        fixture = LockFixture(plan, args)
        origin = provenance(args, plan)
        for name in (Path(__file__).name, 'production_lock_refusal_proof_test.py', 'session_lock_fixture.c',
                     'production_session_fault_proof.py', 'production_desktop_fault_proof.py',
                     'desktop_faults.py', 'production_cancel_proof.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        origin['lock_fixture'] = plan['lock_fixture']
        save('provenance.json', origin)
        observer = launch('observer')
        trace = connect_trace(args.trace_socket, fixture.config)
        spec = {**plan['agents'][0], 'pointer_stage': plan['recovery']['pointer_stage']}
        assert state(args.foreground_journal)['held'] is False, 'lock setup requires released primary fixture'
        probe = prepare_refusal_click(observer, spec, save)
        arguments, prepared_ns = probe['arguments'], probe['prepared_ns']
        policy = pointer_cleanup(fixture.config)
        if policy == 'retained_inert':
            lock_initial = start_trace()
        fixture.lock()
        settle_locked(fixture)
        if policy == 'retained_inert':
            trace.exchange('TRACE_STOP')
            tracing = False
            lock_stopped = trace.collect()
            save('lock-transition-trace.json', lock_stopped)
            save('lock-transition-analysis.json', verify_inert_transition(lock_initial, lock_stopped))
        locked_primary, locked_foreground = wm(), state(args.foreground_journal)
        refusal = report['refusal'] = {**probe, 'outcome': 'unknown', 'replayed': False,
            'pointer_cleanup': policy,
            'lock_ack_ns': fixture.record['ack']['observed_ns'], 'deadline_ns': fixture.deadline_ns,
            'before': production_status(fixture.config)}
        stable_status(fixture.record['after'], refusal['before'], policy=policy)
        refusal['trace_before'] = start_trace()
        tracing = True
        refusal['runtime_started_ns'] = time.monotonic_ns()
        actor = launch('refusal')
        refusal['runtime_pid'] = actor.process.pid
        assert not actor.tool('start_session', {'session': spec['name']}).get('isError')
        fixture.locked()
        refusal['dispatch_ns'] = time.monotonic_ns()
        assert refusal['dispatch_ns'] - prepared_ns <= MAX_GROUNDING_AGE_NS, 'refusal grounding expired'
        click_once(actor, {**arguments, **spec['target'],
            'session': spec['name'], 'delivery_mode': 'background'}, refusal, save, 'refusal.json')
        close_owned(actor)
        fixture.locked()
        refusal.update(after=production_status(fixture.config), trace_after=trace.collect(), observed_ns=time.monotonic_ns())
        refusal['verification'] = verify_refusal(refusal)
        if policy == 'retained_inert':
            # Keep the same trace running through readback and graceful unlock.
            # The strict settled prefix is separate from raw transition analysis.
            stopped = stopped_prefix(trace.collect())
        else:
            trace.exchange('TRACE_STOP')
            tracing = False
            stopped = trace.collect()
        save('refusal-trace-prefix.json' if policy == 'retained_inert' else 'refusal-trace.json', stopped)
        refusal['analysis_uses_end_sentinel'] = policy == 'retained_inert'
        refusal['isolation'] = verify_refusal_cleanup(refusal['trace_after'], stopped)
        refusal['primary_readback'] = {'before': locked_primary, 'after': wm(),
            'foreground_before': locked_foreground, 'foreground_after': state(args.foreground_journal)}
        assert refusal['primary_readback']['after'] == locked_primary
        assert all(refusal['primary_readback']['foreground_after'][key] == locked_foreground[key]
                   for key in ('clicks', 'keys', 'scroll', 'held'))
        save('refusal.json', refusal)
        restoration = fixture.restore()
        save('restoration.json', restoration)
        if policy == 'retained_inert':
            trace.exchange('TRACE_STOP')
            tracing = False
            unlock_stopped = trace.collect()
            save('unlock-transition-trace.json', unlock_stopped)
            save('unlock-transition-analysis.json', verify_inert_transition(refusal['trace_before'], unlock_stopped))
        fixture.check_targets()
        power(fixture.config, True)
        # All intentional focus/cursor/grab setup precedes the recovery trace.
        setup = {'before': wm(), 'started_ns': time.monotonic_ns(), 'continuous_isolation': 'unproven'}
        foreground = grounded_snapshot(observer, plan['foreground'])['window_bounds']
        screen_response = observer.tool('get_desktop_state', {})
        assert not screen_response.get('isError')
        screen = screen_response['structuredContent']
        x, y = plan['primary_point']
        assert 0 < x < foreground['width'] and 0 < y < foreground['height']
        x, y = foreground['x'] + x, foreground['y'] + y
        assert 0 <= x < screen['screen_width'] and 0 <= y < screen['screen_height']
        deadline = time.monotonic_ns() + PRIMARY_LIFETIME_MS * 1_000_000
        grab = subprocess.Popen([str(args.primary_grab), str(x), str(y), str(screen['screen_width']),
            str(screen['screen_height']), str(PRIMARY_LIFETIME_MS)], stdout=subprocess.PIPE, text=True)
        assert primary_acknowledgement(grab.stdout) == 'HELD\n'
        wait_for(lambda: state(args.foreground_journal)['held'], timeout=3)
        primary, baseline = wm(), state(args.foreground_journal)
        assert primary['pid'] == plan['foreground']['pid'] and int(primary['address'], 16) == plan['foreground']['window_id']
        assert primary['cursor'] == {'x': x, 'y': y} and baseline['held']
        setup.update(after=primary, observed_ns=time.monotonic_ns())
        save('foreground-setup.json', setup)
        fixture.check_targets()
        stable_status(restoration['after'], production_status(fixture.config), policy=policy)
        initial = start_trace()
        tracing = True
        save('recovery-trace-initial.json', initial)
        fresh = launch('recovery')
        assert actor.process.poll() is not None and fresh.process.pid != actor.process.pid
        recovery = report['recovery']
        recovery.update(runtime_pid=fresh.process.pid, previous_runtime_pid=actor.process.pid, replayed=False)
        name = spec['name'] + '-recovery'
        assert not fresh.tool('start_session', {'session': name}).get('isError')
        grounding = prepare_click(fresh, {**spec, 'name': name})
        arguments, oracle, prepared_ns = (grounding[key] for key in ('arguments', 'oracle', 'prepared_ns'))
        save('recovery-grounding.json', grounding)
        require_primary_active(grab, deadline)
        dispatched_ns = time.monotonic_ns()
        assert dispatched_ns - prepared_ns <= MAX_GROUNDING_AGE_NS
        recovery['action'] = {'outcome': 'unknown', 'replayed': False, 'dispatch_ns': dispatched_ns,
                              'prepared_ns': prepared_ns, 'tool': grounding.get('tool', 'click')}
        response = click_once(fresh, {**arguments, **spec['target'], 'session': name,
            'delivery_mode': 'background'}, recovery['action'], save, 'recovery-action.json')
        check_response(response, {'kind': 'dispatched'})
        after = grounded_snapshot(observer, spec['target'], spec, session=False)
        save('recovery-after.json', after)
        recovery['app_effect'] = pointer_grounding.verify(after, pointer_grounding.read_pixels(after['proof_image']), oracle)
        prefix = trace.collect()
        save('recovery-trace-prefix.json', prefix)
        tool = recovery['action']['tool']
        recovery['trace'] = verify_recovery_trace(initial, prefix, capacity_lane(initial, prefix, tool), tool)
        close_owned(fresh)
        trace.exchange('TRACE_STOP')
        tracing = False
        stopped = trace.collect()
        save('recovery-trace.json', stopped)
        recovery['isolation'] = verify_recovery_cleanup(prefix, stopped)
        assert released_synthetic_input(stopped)
        require_primary_active(grab, deadline)
        assert wm() == primary
        current = state(args.foreground_journal)
        assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held'))
        save('recovery-primary-readback.json', {'before': primary, 'after': wm(), 'foreground_before': baseline, 'foreground_after': current})
        fixture.check_targets()
        final_status = production_status(fixture.config)
        # Exactly the one completed recovery click may advance dispatch counts.
        old = lanes(restoration['after'], cleared=True, allow_passive=policy == 'retained_inert')
        new = lanes(final_status, cleared=True, allow_passive=True)
        assert sum(new[k]['dispatches'] - old[k]['dispatches'] for k in old) == 1
        for key in old:
            assert new[key]['dispatches'] >= old[key]['dispatches'] and new[key].get('reserved') is False
            assert all(new[key][field] == old[key][field] for field in ('epoch', 'desktop_generation'))
        save('final-status.json', final_status)
        recovery['result'] = 'verified'
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        operations = []
        if fixture:
            operations.append(('graceful_unlock', lambda: fixture.restore(cleanup=True)))
        operations += [(f'close_runtime_{i}', lambda c=c: close_owned(c)) for i, c in enumerate(clients)]
        if trace:
            if tracing:
                def preserve_trace():
                    trace.exchange('TRACE_STOP')
                    save('failed-phase-trace.json', trace.collect())
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
    add_provenance_arguments(parser)
    raise SystemExit(run(parser.parse_args()))
