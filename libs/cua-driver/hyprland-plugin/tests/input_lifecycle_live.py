"""Held-input failure proof through Driver MCP in a disposable Hyprland VM.

The reviewed plan binds two already-observed native fixtures and canvas points.
Only the host operator signs grants. Raw wire logs stay local; public results
contain categorical outcomes and timing. This supplements the desktop matrix.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import select
import subprocess
import time
from types import SimpleNamespace

from driver_input_live import MCP, journal, state, wait_for, wm
from input_transport_test import connect, exchange, refused
from lifecycle_evidence import held_release, primary_wire_events, unchanged_primary
from realapp_proof import cleanup_all
from compositor_stall import stall_past_expiry


def validate_plan(plan):
    targets = [plan[name] for name in ('background', 'foreground')]
    assert all(set(target) == {'pid', 'window_id'} for target in targets)
    assert all(type(target['pid']) is int and target['pid'] > 0 for target in targets)
    assert targets[0]['pid'] != targets[1]['pid'], 'primary and agent must use separate fixture processes'
    for name in ('background', 'foreground'):
        bounds = plan[name + '_bounds']
        assert all(type(bounds[key]) in (int, float) for key in ('x', 'y', 'width', 'height'))
        assert bounds['width'] > 0 and bounds['height'] > 0
    for name in ('from', 'to'):
        point = plan[name]
        assert len(point) == 2 and all(type(value) in (int, float) and value >= 0 for value in point)
        # Driver uses snapshot pixels; exact point-to-image bounds are checked
        # again against the fresh grounding response in run().
    point = plan['foreground_point']
    bounds = plan['foreground_bounds']
    assert len(point) == 2 and 0 < point[0] < bounds['width'] and 0 < point[1] < bounds['height']


def run(args):
    args.evidence.mkdir(parents=True, exist_ok=False)
    plan = json.loads(args.plan.read_text())
    validate_plan(plan)
    target, foreground = plan['background'], plan['foreground']
    label = 'lifecycle-' + args.evidence.name
    client = observer = grab = operator = executor = None
    recording = False
    unloaded = False
    result = {'result': 'failed', 'case': args.case, 'full_desktop_matrix': False}

    def new_client(name):
        directory = args.evidence / name
        directory.mkdir()
        return MCP(SimpleNamespace(evidence=directory, driver=args.driver, driver_socket=args.driver_socket))

    def snapshot(mcp, selection):
        value = mcp.tool('get_window_state', {**selection, 'max_elements': 80})
        assert not value.get('isError'), value
        data = value['structuredContent']
        assert data.get('screenshot_width', 0) > 0, 'missing grounding screenshot'
        return data

    def input_args():
        return {**target, 'session': label, 'delivery_mode': 'background'}

    def request_grant(mcp, suffix):
        snapshot(mcp, target)
        response = mcp.tool('press_key', {**input_args(), 'key': 'Escape'})
        snapshot(observer, target)
        pending = response.get('structuredContent', {})
        assert response.get('isError') and pending.get('reason') == 'pending_operator_approval', response
        (args.evidence / f'request-{suffix}.json').write_text(json.dumps(pending))
        return pending

    def read_new_wire(offset):
        with args.foreground_wire.open('rb') as stream:
            stream.seek(offset)
            return stream.read().decode('utf-8')

    try:
        observer, client = new_client('observer'), new_client('agent')
        bg = snapshot(client, target)
        fg = snapshot(observer, foreground)
        assert bg['window_bounds'] == plan['background_bounds'], 'stale target geometry'
        assert fg['window_bounds'] == plan['foreground_bounds'], 'stale foreground geometry'
        for point in (plan['from'], plan['to']):
            assert point[0] < bg['screenshot_width'] and point[1] < bg['screenshot_height']
        desktop = observer.tool('get_desktop_state', {})['structuredContent']
        point = plan['foreground_point']
        box = fg['window_bounds']
        grab = subprocess.Popen([str(args.primary_grab), str(box['x'] + point[0]),
                                 str(box['y'] + point[1]), str(desktop['screen_width']),
                                 str(desktop['screen_height']), '60000'],
                                stdout=subprocess.PIPE, text=True)
        wait_for(lambda: select.select([grab.stdout], [], [], 0)[0])
        assert grab.stdout.readline().strip() == 'HELD'
        wait_for(lambda: state(args.foreground_journal)['held'])
        snapshot(observer, foreground)
        primary = wm()
        assert primary['pid'] == foreground['pid']
        before_primary = state(args.foreground_journal)
        # A missing or disabled wire logger must not turn into an empty pass.
        assert 'wl_pointer.button' in primary_wire_events(args.foreground_wire.read_text())
        pending = request_grant(client, 'initial')
        grant_path = args.evidence / 'grant-initial.json'
        print('GRANT_REQUIRED', flush=True)
        wait_for(grant_path.exists, 35)  # Primary adversary has a 60-second ceiling.
        grant = json.loads(grant_path.read_text())
        assert all(grant[key] == pending[key] for key in ('epoch', 'challenge', 'target'))
        assert grant['capabilities'] == 10, 'grant must cover only drag and recovery key probe'
        lane = pending['lane']
        input_path = args.input_directory / ('cua-input-test.sock' if lane == 0 else 'cua-input-test-2.sock')
        operator, _ = connect(input_path)
        assert exchange(operator, grant['packet'])['ok']
        # Native recording is supporting evidence, never the cleanup oracle.
        if args.record_video:
            response = observer.tool('start_recording', {'output_dir': str(args.evidence / 'recording')})
            assert not response.get('isError'), response
            recording = True
        if args.case == 'expiry':
            assert args.compositor_pid, 'expiry fault requires an explicitly selected disposable compositor'
            wait_for(lambda: grant['expires_unix_ms'] / 1000 - time.time() <= 2.9, 60)
        snapshot(client, target)
        background_offset = len(journal(args.background_journal))
        wire_offset = args.foreground_wire.stat().st_size
        executor = ThreadPoolExecutor(max_workers=1)
        started_ns = time.monotonic_ns()
        future = executor.submit(client.tool, 'drag', {**input_args(),
            'from_x': plan['from'][0], 'from_y': plan['from'][1],
            'to_x': plan['to'][0], 'to_y': plan['to'][1], 'duration_ms': 2000})
        wait_for(lambda: any(row['kind'] == 'button-press' for row in journal(args.background_journal)[background_offset:]), 3)
        assert time.monotonic_ns() - started_ns < 900_000_000, 'too late to distinguish cancellation from completion'
        fault_ns = time.monotonic_ns()
        if args.case == 'disconnect':
            client.process.terminate()
            client.process.wait(timeout=3)
        elif args.case == 'stop':
            assert exchange(operator, 'STOP')['ok']
        elif args.case == 'expiry':
            stall = stall_past_expiry(args.compositor_pid, grant['expires_unix_ms'])
            fault_ns = stall['resume_ns']
            result.update(compositor_stall_ms=stall['stall_ms'], release_bound_origin='compositor_resume',
                          uninterrupted_desktop=False)
        elif args.case == 'reload':
            assert args.reload_module and args.reload_module.is_file()
            output = subprocess.check_output(['hyprctl', 'plugin', 'unload', str(args.reload_module)], text=True, timeout=5)
            assert output.strip() == 'ok', output
            unloaded = True
            operator.close()
            operator = None
        else:
            raise ValueError('unsupported fault')
        wait_for(lambda: any(row['kind'] == 'button-release' for row in journal(args.background_journal)[background_offset:]), 3)
        try:
            response = future.result(timeout=5)
            result['action_refusal'] = response.get('structuredContent', {}).get('reason')
            if args.case == 'stop':
                assert response.get('isError') and result['action_refusal'] == 'stopped', response
            if args.case == 'expiry':
                assert response.get('isError') and result['action_refusal'] == 'lease_expired', response
            if args.case == 'reload':
                assert response.get('isError'), response
        except (RuntimeError, OSError, ValueError) as error:
            if args.case != 'disconnect':
                raise
            result['action_transport_closed'] = type(error).__name__
        snapshot(observer, target)
        snapshot(observer, foreground)
        assert wm() == primary, 'primary compositor state changed'
        result.update(unchanged_primary(before_primary, state(args.foreground_journal), read_new_wire(wire_offset)))
        rows = journal(args.background_journal)[background_offset:]
        (args.evidence / 'release-evidence.json').write_text(json.dumps({'fault_ns': fault_ns, 'rows': rows}))
        result.update(held_release(rows, fault_ns=fault_ns, maximum_latency_ms=5000))
        assert result['release_latency_ms'] <= 750, 'held input was not released within the cancellation bound'
        assert not state(args.background_journal)['held'], 'application still holds synthetic button'
        result['observed_release'] = True
        if args.case == 'reload':
            output = subprocess.check_output(['hyprctl', 'plugin', 'load', str(args.reload_module)], text=True, timeout=5)
            assert output.strip() == 'ok', output
            unloaded = False
            # The disposable config already explicitly enables the experiment.
            assert subprocess.check_output(['hyprctl', 'reload'], text=True, timeout=5).strip() == 'ok'
            snapshot(observer, target)
            snapshot(observer, foreground)
            assert wm() == primary, 'reload changed primary compositor state'
            result.update(unchanged_primary(before_primary, state(args.foreground_journal), read_new_wire(wire_offset)))
            operator, hello = connect(input_path)
            assert hello['epoch'] != pending['epoch'], 'reload retained the old epoch'
            result['new_epoch'] = True
        if args.case in ('disconnect', 'reload'):
            client.close()
            client = new_client('reconnected')
        fresh = request_grant(client, 'recovery')
        if args.case in ('disconnect', 'reload'):
            assert fresh['challenge'] != pending['challenge'], 'reconnect inherited old authority'
            refused(exchange(operator, grant['packet']), 'stale_target', 'invalid_grant')
        else:
            refused(exchange(operator, grant['packet']), 'invalid_grant')
        result.update(result='passed', fresh_approval_required=True, old_grant_refused=True)
    except Exception as error:
        result['error_type'] = type(error).__name__
        result['error'] = str(error)
        raise
    finally:
        def restore_module():
            if unloaded:
                output = subprocess.check_output(['hyprctl', 'plugin', 'load', str(args.reload_module)], text=True, timeout=5)
                assert output.strip() == 'ok', output
                assert subprocess.check_output(['hyprctl', 'reload'], text=True, timeout=5).strip() == 'ok'
        def stop_operator():
            if operator:
                try:
                    exchange(operator, 'STOP')
                finally:
                    operator.close()
        def release_primary():
            if grab and grab.poll() is None:
                grab.terminate()
                grab.wait(timeout=5)
                wait_for(lambda: not state(args.foreground_journal)['held'])
        def stop_recording():
            if recording:
                response = observer.tool('stop_recording', {})
                assert not response.get('isError'), response
        operations = [('operator_stop', stop_operator), ('primary_release', release_primary),
                      ('recording_stop', stop_recording), ('restore_module', restore_module)]
        if executor:
            operations.append(('action_thread', lambda: executor.shutdown(wait=True, cancel_futures=True)))
        if client:
            operations.append(('client_close', client.close))
        if observer:
            operations.append(('observer_close', observer.close))
        errors = cleanup_all(operations)
        result['cleanup_errors'] = errors
        if errors:
            result['result'] = 'failed'
        (args.evidence / 'result.json').write_text(json.dumps(result, indent=2))
        print(json.dumps(result), flush=True)
        if errors:
            raise AssertionError('lifecycle cleanup failed')
    return result


if __name__ == '__main__':
    if not __debug__:
        raise RuntimeError('assertions must remain enabled')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', choices=('disconnect', 'stop', 'expiry', 'reload'), required=True)
    for name in ('plan', 'evidence', 'driver', 'driver-socket', 'input-directory',
                 'primary-grab', 'background-journal', 'foreground-journal', 'foreground-wire'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--record-video', action='store_true')
    parser.add_argument('--compositor-pid', type=int)
    parser.add_argument('--reload-module', type=Path)
    run(parser.parse_args())
