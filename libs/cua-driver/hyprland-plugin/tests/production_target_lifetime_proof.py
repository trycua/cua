"""TEST ONLY: destroy one exact disposable Calc target during a normal MCP drag.

CLI flags match production_geometry_fault_proof. Plan uses the exact VM,
compositor, processes, foreground, primary_point, package_versions and one
Calc agent from production_primary_conflict_proof, with purpose=target_lifetime,
case=active_drag and fault={kind:destroy,signal:SIGKILL}. recovery is
{mode:prepared_distinct_process,agent:<Calc spec>,identity:<exact /proc identity>}
where its pointer_stage is click_a1 or click_b2 and drag={}. Both Calc specs add
owned={document:{path,device,inode,uid,sha256},profile:<canonical directory>}.
Each document is an existing suite cua-smoke-calc.ods in a private directory;
each profile is that document's sibling calc-profile. Both processes must have
the suite's exact launch argv. The controller prepares both before this run.

Only normal Driver MCP sends application input. This runner sends one SIGKILL
through a revalidated pidfd, only after held-drag trace AND status gates. Saved
bytes are archived before injection. A destroyed client cannot acknowledge a
release: require cleared/pruned compositor state, never fabricate a wire release
or filter the raw trace to balance it. Require partial stale_target, unchanged
idle sibling input/authority (resource pruning is allowed), and continuous
primary isolation. Recovery is one fresh
click on the separately prepared process with a new runtime and screenshot.
Same-client recovery, relaunch, PID/address reuse, active sibling cancellation,
full desktop certification and physical hardware are explicitly unproven.
No app launches, signer material, policy changes, production edits or replay.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import os
from pathlib import Path
import platform
import select
import signal
import stat
import subprocess
import time

from desktop_faults import _hypr, _identity, _same_compositor
from driver_input_live import state, wait_for, wm
from primary_trace import analyze
from production_active_lock_proof import drag_once, held_status
from production_cancel_proof import (PROFILE, MAX_GROUNDING_AGE_NS, active_drags,
    close_owned, grounded_snapshot, poll_active, prepare_drag, stopped_prefix)
from production_desktop_fault_proof import guest_identity
from production_geometry_fault_proof import window_bounds
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
import production_pointer_grounding as pointer_grounding
from production_primary_conflict_proof import ExactDesktop, validate_identity, validate_plan as primary_plan
from production_realapp_proof import (PRIMARY_LIFETIME_MS, app_process_identity,
    capacity_lane, check_response, primary_acknowledgement, provenance,
    require_primary_active, trace_interval)
from production_session_fault_proof import lanes
from realapp_proof import cleanup_all


def validate_owned(spec):
    assert spec['app'] == 'calc' and spec['drag'] == {}
    owned = spec['owned']
    assert set(owned) == {'document', 'profile'}
    document = owned['document']
    assert set(document) == {'path', 'device', 'inode', 'uid', 'sha256'}
    path = Path(document['path'])
    assert path.is_absolute() and path.name == 'cua-smoke-calc.ods'
    assert all(type(document[k]) is int and document[k] >= 0 for k in ('device', 'inode', 'uid'))
    assert len(document['sha256']) == 64 and all(c in '0123456789abcdef' for c in document['sha256'])
    assert Path(owned['profile']) == path.parent / 'calc-profile'


def validate_plan(plan):
    assert plan['purpose'] == 'target_lifetime' and plan['case'] == 'active_drag'
    assert plan['fault'] == {'kind': 'destroy', 'signal': 'SIGKILL'}
    recovery = plan['recovery']
    assert set(recovery) == {'mode', 'agent', 'identity'}
    assert recovery['mode'] == 'prepared_distinct_process'
    fresh = recovery['agent']
    assert fresh['pointer_stage'] in ('click_a1', 'click_b2')
    base = {k: v for k, v in plan.items() if k != 'fault'}
    primary_plan({**base, 'purpose': 'primary_conflict', 'case': 'initial_refusal',
                  'recovery': {'pointer_stage': 'click_b2'}})
    primary_plan({**base, 'purpose': 'primary_conflict', 'case': 'initial_refusal',
                  'agents': [{**fresh, 'pointer_stage': 'select_range'}],
                  'processes': {**plan['processes'], 'target': recovery['identity']},
                  'recovery': {'pointer_stage': 'click_b2'}})
    old = plan['agents'][0]
    for spec in (old, fresh):
        validate_owned(spec)
    validate_identity(recovery['identity'])
    identities = [*plan['processes'].values(), recovery['identity'], plan['compositor']]
    assert len({p['pid'] for p in identities}) == 4, 'recovery must be a distinct process'
    assert len({s['window_id'] for s in (old['target'], fresh['target'], plan['foreground'])}) == 3
    assert old['name'] != fresh['name'] and old['owned']['profile'] != fresh['owned']['profile']
    assert old['owned']['document']['path'] != fresh['owned']['document']['path']


def saved_document(spec):
    """Read only the identity-bound saved output; selection never saves it."""
    expected = spec['owned']['document']
    path = Path(expected['path'])
    assert path.resolve(strict=True) == path, 'document alias refused'
    parent = path.parent.stat()
    assert parent.st_uid == os.getuid() and not parent.st_mode & 0o077, 'private test directory required'
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        assert stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and 0 < info.st_size <= 16 * 1024 * 1024
        assert [info.st_dev, info.st_ino, info.st_uid] == [expected[k] for k in ('device', 'inode', 'uid')]
        assert info.st_uid == os.getuid() and not info.st_mode & 0o022
        with os.fdopen(os.dup(fd), 'rb') as stream:
            data = stream.read(16 * 1024 * 1024 + 1)
        assert len(data) == info.st_size and hashlib.sha256(data).hexdigest() == expected['sha256']
        fields = ('st_dev', 'st_ino', 'st_size', 'st_mtime_ns', 'st_ctime_ns')
        assert all(getattr(now, k) == getattr(info, k) for now in (os.fstat(fd), path.lstat()) for k in fields)
        return data
    finally:
        os.close(fd)


def check_calc(spec, identity):
    assert _identity(identity['pid']) == identity, 'Calc process identity changed'
    assert identity['uid'] == os.getuid()
    profile = Path(spec['owned']['profile'])
    assert profile.resolve(strict=True) == profile and profile.is_dir()
    assert profile.stat().st_uid == os.getuid() and not profile.stat().st_mode & 0o077
    argv = Path(f'/proc/{identity["pid"]}/cmdline').read_bytes().rstrip(b'\0').split(b'\0')
    expected = [identity['exe'], f'-env:UserInstallation={profile.as_uri()}', '--norestore',
                '--nologo', '--calc', spec['owned']['document']['path']]
    assert [x.decode() for x in argv] == expected, 'not the exact dedicated suite Calc launch'


class TargetLifetime(ExactDesktop):
    """No PID-name matching, shell signals, fallback kills, relaunch or retries."""
    def __init__(self, plan):
        self.plan, self.instance = plan, plan['compositor']['instance']
        self.compositor = {k: v for k, v in plan['compositor'].items() if k != 'instance'}
        self.spec, self.fresh = plan['agents'][0], plan['recovery']['agent']
        self.fd, self.sent, self.destroyed = None, False, False
        self.record = {'result': 'unproven', 'target': self.spec['target']}
        assert subprocess.run(['systemd-detect-virt', '--vm', '--quiet'], timeout=2).returncode == 0
        self.guard()
        for spec, identity in ((self.spec, plan['processes']['target']), (self.fresh, plan['recovery']['identity'])):
            app_process_identity('calc', identity['pid'])
            check_calc(spec, identity)
            saved_document(spec)
        self.fd = os.pidfd_open(self.spec['target']['pid'])
        try:
            self.guard()  # Revalidate AFTER binding, closing PID reuse race.
        except BaseException:
            self.close()
            raise

    def guard(self):
        assert self.plan['disposable'] is True and platform.system() == 'Linux'
        assert guest_identity() == self.plan['vm'], 'wrong VM boot'
        assert os.environ.get('HYPRLAND_INSTANCE_SIGNATURE') == self.instance
        assert self.compositor['uid'] == os.getuid()
        _same_compositor(self.compositor, self.instance)
        checks = [('foreground', self.plan['foreground'], self.plan['processes']['foreground'], None),
                  ('replacement', self.fresh['target'], self.plan['recovery']['identity'], self.fresh)]
        if not self.sent:
            checks.append(('target', self.spec['target'], self.plan['processes']['target'], self.spec))
            if self.fd is not None:
                assert not select.select([self.fd], [], [], 0)[0], 'target already exited'
        windows = json.loads(_hypr(self.instance, '-j', 'clients'))
        for name, target, identity, spec in checks:
            assert _identity(identity['pid']) == identity, name + ' identity changed'
            selected = [w for w in windows if w.get('pid') == target['pid']]
            assert len(selected) == 1, 'ambiguous ' + name + ' windows'
            window = selected[0]
            assert int(window['address'], 16) == target['window_id'] and window.get('xwayland') is False
            if spec:
                check_calc(spec, identity)
                assert 'cua-smoke-calc.ods' in window.get('title', '')
                assert window_bounds(window) == spec['bounds'], 'reviewed bounds changed'
        return windows

    def status(self):
        self.guard()
        status = json.loads(_hypr(self.instance, '-j', 'cua:status'))
        lanes(status)
        return status

    def gone(self):
        windows = self.guard()
        target = self.spec['target']
        exited = bool(select.select([self.fd], [], [], 0)[0])
        absent = not any(w.get('pid') == target['pid'] or int(w['address'], 16) == target['window_id'] for w in windows)
        if exited and absent:
            self.destroyed = True
            return {'pidfd_exited': True, 'window_absent': True, 'observed_ns': time.monotonic_ns()}
        return None

    def inject(self, trace, initial, pending, guard):
        assert not self.sent and self.fd is not None, 'one termination attempt only'
        self.guard()
        saved_document(self.spec)
        page, active = poll_active(trace, initial, None, [pending])
        lane = next(iter(active))
        status_started = time.monotonic_ns()
        before = self.status()
        held_status(before, lane)
        self.guard()
        guard()
        assert not pending.done(), 'drag already returned'
        requested = time.monotonic_ns()
        assert 0 <= requested - page['events'][-1][1] <= 250_000_000
        assert 0 <= requested - status_started <= 250_000_000, 'stale held-state gate'
        self.record.update(prefix=page, lane=lane, gate_status=before,
                           status_started_ns=status_started, requested_ns=requested)
        self.sent = True  # A lost acknowledgement never authorizes a retry.
        signal.pidfd_send_signal(self.fd, signal.SIGKILL)
        self.record['gone'] = wait_for(self.gone, timeout=3)
        def cleared():
            observed = self.status()
            self.record['last_status'] = observed
            try:
                verify_cleared(before, observed, lane)
            except AssertionError:
                return None
            return observed
        self.record['after'] = wait_for(cleared, timeout=3)
        self.record['observed_ns'] = time.monotonic_ns()
        self.record['result'] = 'observed'
        guard()
        return lane

    def close(self):
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None


def verify_cleared(before, after, lane):
    held_status(before, lane)
    old, new = lanes(before), lanes(after, cleared=True)
    resources = {'seat_resources', 'pointer_resources', 'keyboard_resources'}
    for key in old:
        assert all(old[key][k] == new[key][k] for k in ('epoch', 'desktop_generation', 'dispatches'))
        assert all(type(new[key][k]) is int and 0 <= new[key][k] <= old[key][k] for k in resources)
        if key != lane - 1:
            # A toolkit can bind both globals. Its death may prune resources
            # from the idle lane too, but must not change its input/authority.
            assert {k: v for k, v in old[key].items() if k not in resources} == {
                k: v for k, v in new[key].items() if k not in resources}, 'idle sibling lane changed'
        else:
            assert new[key]['reserved'] is True, 'target loss unexpectedly lost runtime reservation'
            assert all(type(new[key][k]) is int and new[key][k] < old[key][k]
                       for k in resources), 'destroyed resources not pruned'


def isolation(page, *, stopped=False):
    result = analyze(page if stopped else stopped_prefix(page))
    assert result['result'] == 'passed', result
    return result


def verify_fault(boundary, record, action):
    assert record['result'] == 'observed'
    assert record['gone']['pidfd_exited'] is True and record['gone']['window_absent'] is True
    prefix, lane = record['prefix'], record['lane']
    assert set(active_drags(prefix)) == {lane}
    assert all(r[5] in (0, lane) for r in prefix['events'])
    assert all(r[2] in ('agent_approved', 'agent_admitted', 'agent_drag_start', 'pointer_enter',
                        'pointer_motion', 'pointer_button') for r in prefix['events'] if r[5] == lane)
    requested = record['requested_ns']
    assert 0 <= requested - prefix['events'][-1][1] <= 250_000_000
    assert 0 <= requested - record['status_started_ns'] <= 250_000_000
    assert requested <= record['gone']['observed_ns'] <= record['observed_ns']
    tail = trace_interval(prefix, boundary)
    synthetic = [r for r in tail if r[5] in (1, 2)]
    assert all(r[5] == lane for r in synthetic), 'cleanup crossed lanes'
    cancels = [r for r in synthetic if r[2] == 'agent_cancel']
    assert len(cancels) == 1 and requested <= cancels[0][1] <= record['observed_ns']
    allowed = {'agent_cancel', 'pointer_motion', 'pointer_enter', 'pointer_leave', 'keyboard_leave', 'pointer_button'}
    assert all(r[2] in allowed for r in synthetic), 'replay or false completion'
    assert not any(r[2] in ('pointer_motion', 'pointer_enter') and r[0] > cancels[0][0] for r in synthetic)
    releases = [r for r in synthetic if r[2] == 'pointer_button']
    assert len(releases) <= 1 and all(r[6] == 0 and r[0] > cancels[0][0] for r in releases)
    verify_cleared(record['gate_status'], record['after'], lane)
    assert action['replayed'] is False and action['outcome'] == 'response', 'unknown delivery; never replay'
    check_response(action['response'], {'kind': 'partial'})
    content = action['response']['structuredContent']
    assert content.get('reason') == 'stale_target' and content['delivery']['mode'] == 'background'
    assert content['delivery']['delivered_count'] > 0
    assert action['dispatch_ns'] <= prefix['events'][-1][1] <= action['observed_ns']
    return {'result': 'verified', 'reason': 'stale_target', 'lane': lane,
            'continuous_isolation': isolation(boundary), 'own_state': 'cleared_and_pruned',
            'wire_release_events': len(releases), 'destroyed_client_release_ack': 'unprovable',
            'unsaved_drag_effect': 'unproven'}


def cleanup_trace(before, after, *, stopped=False):
    if stopped:
        assert after['events'][:before['count']] == before['events'], 'trace history changed'
        rows = after['events'][before['count']:]
    else:
        rows = trace_interval(before, after)
    assert all(r[2] in ('agent_cancel', 'pointer_leave', 'keyboard_leave')
               for r in rows if r[5] in (1, 2)), 'unexpected input after action'
    return isolation(after, stopped=stopped)


def recover(client, observer, victim, fault, trace, boundary, lane, guard, save, result):
    assert fault.destroyed and victim.process.poll() is not None
    assert victim.process.pid not in assert_distinct_runtimes([client, observer])
    spec = fault.fresh
    fault.guard()
    assert not client.tool('start_session', {'session': spec['name']}).get('isError')
    prepared = prepare_drag(client, spec)
    assert prepared['target'] != fault.spec['target']
    result.update(grounding=prepared, runtime_pid=client.process.pid, replayed=False)
    save('recovery-grounding.json', result)
    guard()
    fault.guard()
    dispatch = time.monotonic_ns()
    assert 0 <= dispatch - prepared['prepared_ns'] <= MAX_GROUNDING_AGE_NS
    result['action'] = {'outcome': 'unknown', 'replayed': False, 'dispatch_ns': dispatch}
    save('recovery-action.json', result['action'])
    try:
        response = client.tool('click', {**prepared['arguments'], **spec['target'],
            'session': spec['name'], 'delivery_mode': 'background'})
        result['action'].update(outcome='response', response=response)
    except Exception as error:
        result['action']['error'] = str(error)
    finally:
        save('recovery-action.json', result['action'])
    after = grounded_snapshot(observer, spec['target'], spec, session=False)
    save('recovery-after.json', after)
    assert result['action']['outcome'] == 'response', 'recovery unknown; never replay'
    check_response(response, {'kind': 'dispatched'})
    assert after['snapshot_id'] != prepared['snapshot']['snapshot_id'], 'reused screenshot'
    result['app_effect'] = pointer_grounding.verify(after, pointer_grounding.read_pixels(after['proof_image']), prepared['oracle'])
    page = trace.collect()
    save('recovery-prefix.json', page)
    rows = trace_interval(boundary, page)
    assert capacity_lane(boundary, page, 'click') == lane
    synthetic = [r for r in rows if r[5] in (1, 2)]
    assert all(r[5] == lane for r in synthetic)
    assert sum(r[2] == 'agent_admitted' for r in synthetic) == 1
    assert sum(r[2] == 'agent_action_end' for r in synthetic) == 1
    assert all(r[2] in ('agent_admitted', 'agent_approved', 'agent_action_end', 'pointer_enter',
                        'pointer_motion', 'pointer_button', 'pointer_leave', 'keyboard_leave') for r in synthetic)
    assert [r[6] for r in synthetic if r[2] == 'pointer_button'] == [1, 0]
    recovered = lanes(fault.status())
    for key, row in recovered.items():
        assert row['held_button'] == row['held_keys'] == 0
        assert all(row[k] is False for k in ('drag_active', 'lease_active', 'keyboard_focus'))
        assert row['reserved'] is (key == lane - 1)
        if key != lane - 1:
            assert row['pointer_focus'] is False
    result['continuous_isolation'] = isolation(page)
    guard()
    result['result'] = 'verified'
    return page


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2) + '\n')
    report = {'result': 'failed', 'scope': 'exact-target-destruction-and-prepared-distinct-recovery',
              'recovery': {'result': 'unproven'}, 'same_client_recovery': 'unproven',
              'pid_or_address_reuse': 'unproven', 'active_sibling': 'unproven',
              'full_desktop_matrix': False, 'physical_hardware': False}
    clients, observer, fault, trace, grab, pool, future = [], None, None, None, None, None, None
    prefix = baseline = primary_before = deadline = None
    action = {'outcome': 'unknown', 'replayed': False}
    def guard():
        require_primary_active(grab, deadline)
    try:
        plan = json.loads(args.plan.read_text())
        save('plan.json', plan)
        validate_plan(plan)
        fault = TargetLifetime(plan)
        origin = provenance(args, plan)
        origin['ownership'] = {key: plan[key] for key in ('vm', 'compositor', 'processes', 'recovery')}
        for name in ('production_target_lifetime_proof.py', 'production_target_lifetime_proof_test.py'):
            path = Path(__file__).with_name(name)
            origin['files'][name] = {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        save('provenance.json', origin)
        for name, spec in (('target', fault.spec), ('replacement', fault.fresh)):
            (args.evidence / (name + '-saved-before.ods')).write_bytes(saved_document(spec))
        save('prepared-app-identities.json', {
            name: app_process_identity('calc', spec['target']['pid'])
            for name, spec in (('target', fault.spec), ('replacement', fault.fresh))})
        def launch(name):
            directory = args.evidence / name
            directory.mkdir()
            return DirectMCP(args.driver, directory, PROFILE)
        clients.append(launch('agent'))
        observer = launch('observer')
        report['driver_processes'] = assert_distinct_runtimes([clients[0], observer])
        assert not clients[0].tool('start_session', {'session': fault.spec['name']}).get('isError')
        bounds = grounded_snapshot(observer, plan['foreground'])['window_bounds']
        desktop = observer.tool('get_desktop_state', {})
        assert not desktop.get('isError')
        desktop = desktop['structuredContent']
        x, y = plan['primary_point']
        assert 0 < x < bounds['width'] and 0 < y < bounds['height']
        x, y = x + bounds['x'], y + bounds['y']
        assert 0 <= x < desktop['screen_width'] and 0 <= y < desktop['screen_height']
        deadline = time.monotonic_ns() + PRIMARY_LIFETIME_MS * 1_000_000
        grab = subprocess.Popen([str(args.primary_grab), str(x), str(y), str(desktop['screen_width']),
            str(desktop['screen_height']), str(PRIMARY_LIFETIME_MS)], stdout=subprocess.PIPE, text=True)
        assert primary_acknowledgement(grab.stdout) == 'HELD\n'
        wait_for(lambda: state(args.foreground_journal)['held'], timeout=3)
        primary_before, baseline = wm(), state(args.foreground_journal)
        assert primary_before['pid'] == plan['foreground']['pid'] and baseline['held']
        save('initial-primary.json', {'primary': primary_before, 'foreground': baseline})
        status = fault.status()
        lanes(status, cleared=True)
        save('initial-status.json', status)
        trace = fault.trace(args.trace_socket)
        start = time.monotonic_ns()
        trace.exchange('TRACE_START')
        initial = trace.collect()
        save('initial-trace.json', initial)
        assert start <= initial['events'][0][1] <= time.monotonic_ns()
        assert not any(r[5] in (1, 2) for r in initial['events'])
        prepared = prepare_drag(clients[0], fault.spec)
        save('drag-grounding.json', prepared)
        guard()
        pool = ThreadPoolExecutor(max_workers=1)
        future = pool.submit(drag_once, clients[0], fault.spec, prepared, action, save)
        lane = fault.inject(trace, initial, future, guard)
        save('fault.json', fault.record)
        future.result(timeout=5)
        report['action'] = action
        boundary = trace.collect()
        save('fault-prefix.json', boundary)
        report['fault'] = verify_fault(boundary, fault.record, action)
        for name, spec in (('target', fault.spec), ('replacement', fault.fresh)):
            (args.evidence / (name + '-saved-after.ods')).write_bytes(saved_document(spec))
        report['saved_output'] = 'identity_and_bytes_unchanged; archived_before_and_after'
        close_owned(clients[0])
        teardown = trace.collect()
        cleanup_trace(boundary, teardown)
        save('pre-recovery-prefix.json', teardown)
        clients.append(launch('recovery'))
        prefix = recover(clients[-1], observer, clients[0], fault, trace, teardown, lane, guard, save, report['recovery'])
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        operations = [(f'close_agent_{i}', lambda c=c: close_owned(c)) for i, c in enumerate(clients)]
        if future:
            operations.append(('wait_action', lambda: future.result(timeout=5)))
        operations.append(('preserve_action', lambda: save('drag-action.json', action)))
        if pool:
            operations.append(('shutdown_pool', lambda: pool.shutdown(wait=False, cancel_futures=True)))
        if fault:
            operations.append(('preserve_fault', lambda: save('fault.json', fault.record)))
        if trace:
            def finish():
                trace.exchange('TRACE_STOP')
                stopped = trace.collect()
                save('trace.json', stopped)
                report['continuous_isolation'] = cleanup_trace(prefix, stopped, stopped=True) if prefix else isolation(stopped, stopped=True)
                guard()
                primary_after = wm()
                assert primary_after == primary_before
                current = state(args.foreground_journal)
                assert all(current[k] == baseline[k] for k in ('clicks', 'keys', 'scroll', 'held'))
                save('final-primary.json', {'primary': primary_after, 'foreground': current})
                final = fault.status()
                save('final-status.json', final)
                assert all(r['reserved'] is False for r in lanes(final, cleared=True).values())
            operations.extend([('finish_trace', finish), ('close_trace', trace.close)])
        if fault:
            operations.append(('close_pidfd', fault.close))
        def release():
            if grab:
                if grab.poll() is None:
                    grab.terminate()
                stop_process(grab)
                wait_for(lambda: not state(args.foreground_journal)['held'], timeout=3)
        operations.append(('release_primary', release))
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
