"""Run reviewed, snapshot-grounded app tasks through independent Driver MCPs.

The plan records exact observed window identities/bounds and actions. It is a
deterministic regression runner, not an autonomous model benchmark. No signing
key enters this process. Each input call has a fresh before/after snapshot;
app output assertions live in the reviewed plan and are independently read.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import math
import os
from pathlib import Path
import re
import select
import subprocess
import threading
import time
from types import SimpleNamespace
import xml.etree.ElementTree as ET

from driver_input_live import MCP, wait_for, state, wm
from primary_trace import Trace, analyze


def cleanup_all(operations):
    """Attempt every independent cleanup even after transport/recorder failure."""
    errors = []
    for name, operation in operations:
        try:
            operation()
        except Exception as error:
            errors.append({'operation': name, 'error': str(error)})
    return errors


def released_synthetic_input(data):
    """Balance wire-level press/release events independently for both seats."""
    balances = {(actor, kind): 0 for actor in (1, 2) for kind in ('pointer_button', 'keyboard_key')}
    for row in data['events']:
        key = (row[5], row[2])
        if key in balances:
            balances[key] += 1 if row[6] else -1
            assert balances[key] >= 0, 'release without a captured synthetic press'
    assert not any(balances.values()), 'synthetic input remains held at trace end'
    return True


def rect_position(node):
    """Small independent oracle for the reviewed SVG rectangle fixture only."""
    x, y = float(node.get('x', '0')), float(node.get('y', '0'))
    transform = node.get('transform', '')
    if transform:
        match = re.fullmatch(r'translate\(\s*([-+\d.eE]+)[,\s]+([-+\d.eE]+)\s*\)', transform)
        assert match, 'unsupported SVG transform; cannot establish saved geometry'
        x += float(match[1]); y += float(match[2])
    assert math.isfinite(x) and math.isfinite(y)
    return [x, y]


def validate_plan(plan):
    assert len(plan['agents']) <= 2, 'only two independent lanes exist'
    for phase in plan['phases']:
        steps = phase.get('parallel', [] if phase.get('negative_control') else [phase])
        agents = [step['agent'] for step in steps]
        assert len(agents) == len(set(agents)), 'one MCP connection cannot serve concurrent steps'
        assert all(type(index) is int and 0 <= index < len(plan['agents']) for index in agents)
        if 'control' in phase:
            assert 'parallel' in phase and len(agents) == 2
            assert phase['control']['command'] in ('CANCEL', 'STOP')
            assert phase['control']['agent'] in agents
    for oracle in plan.get('outputs', []):
        if 'rect_translation' in oracle:
            assert len(oracle['rect_translation']) == 2
            assert all(len(bounds) == 2 and bounds[0] <= bounds[1] for bounds in oracle['rect_translation'])


def run(args):
    args.evidence.mkdir(parents=True, exist_ok=False)
    plan = json.loads(args.plan.read_text())
    validate_plan(plan)
    (args.evidence / 'plan.json').write_text(json.dumps(plan, indent=2))
    # A public label cannot be adopted by a new transport after a prior run.
    # Evidence directories are exclusive, making these names run-specific.
    for spec in plan['agents']:
        spec['name'] += '-' + args.evidence.name
    clients, operators = [], []
    foreground = plan['foreground']
    recorder = None
    grab = None
    watcher = None
    mover = None
    result = None
    motion_done = threading.Event()
    expected_motion = [] if plan.get('moving_primary') else None
    motion_errors = []
    done = threading.Event()
    trace = Trace(args.input_directory / 'cua-input-test.sock')
    before_app_files = {}
    timeline_lock = threading.Lock()

    def mark(event, **fields):
        with timeline_lock, (args.evidence / 'timeline.jsonl').open('a') as stream:
            stream.write(json.dumps({'monotonic_ns': time.monotonic_ns(), 'event': event, **fields}) + '\n')
    for oracle in plan.get('outputs', []):
        before_app_files[oracle['path']] = Path(oracle['path']).read_bytes()

    def client(directory):
        directory.mkdir()
        return MCP(SimpleNamespace(evidence=directory, driver=args.driver, driver_socket=args.driver_socket))

    def snapshot(mcp, target, label=None):
        result = mcp.tool('get_window_state', {**target, 'max_elements': 100, 'max_depth': 6,
                                             **({'session': label} if label else {})})
        assert not result.get('isError'), result
        assert result['structuredContent'].get('screenshot_width', 0) > 0, 'missing grounding image'
        return result['structuredContent']

    def action(index, step, ready=None):
        mcp, spec = clients[index], plan['agents'][index]
        before = snapshot(mcp, spec['target'], spec['name'])
        assert before['window_bounds'] == spec['bounds'], 'target geometry changed since grounding'
        if ready:
            ready.wait(timeout=20)
        mark('action_start', agent=index, tool=step['tool'])
        result = mcp.tool(step['tool'], {**spec['target'], **step['arguments'],
                          'session': spec['name'], 'delivery_mode': 'background'})
        mark('action_response', agent=index, tool=step['tool'], error=bool(result.get('isError')))
        after = snapshot(mcp, spec['target'], spec['name'])
        if 'expect_refusal' in step:
            assert result.get('isError') and result.get('structuredContent', {}).get('reason') == step['expect_refusal'], result
        else:
            assert not result.get('isError'), result
        assert result['structuredContent']['route'] == 'synthetic_events', 'not plugin input evidence'
        assert after['window_bounds'] == before['window_bounds']
        primary_now = wm()
        keys = ('pid', 'address', 'workspace') if expected_motion is not None else primary_before.keys()
        assert all(primary_now[key] == primary_before[key] for key in keys), 'primary state changed'
        current = state(args.foreground_journal)
        assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'held')), current
        return result['structuredContent']

    def watch():
        observer = Trace(args.input_directory / 'cua-input-test.sock')
        rows = []
        try:
            while not done.wait(0.05):
                page = observer.exchange('TRACE_READ ' + str(len(rows)))
                rows.extend(page['events'])
                if not rows:
                    continue
                primary = [r for r in rows if r[5] == 0]
                motions = [r[3:5] for r in primary if r[2] == 'cursor']
                violations = (sum(math.dist(point, rows[0][3:5]) > .01 for point in motions)
                              if expected_motion is None else sum(i >= len(expected_motion) or math.dist(point, expected_motion[i]) > .01
                                                                  for i, point in enumerate(motions)))
                metrics = {'status': 'recording' if page['active'] else 'stopped',
                           'cursor_violations': violations,
                           'focus_events': sum(r[2] in ('pointer_focus', 'keyboard_focus', 'pointer_enter', 'pointer_leave', 'keyboard_enter', 'keyboard_leave') for r in primary),
                           'releases': sum(r[2] == 'pointer_button' and r[6] == 0 for r in primary),
                           'actions': sum(r[2] in ('agent_action_end', 'agent_drag_end') for r in rows)}
                if page['overflow'] or page['timed_out'] or not page['hook']:
                    metrics['status'] = 'inconclusive'
                if args.metrics:
                    temporary = args.metrics.with_suffix('.pending')
                    temporary.write_text(json.dumps(metrics))
                    temporary.replace(args.metrics)
        finally:
            observer.close()

    def move_primary():
        # Known commands from an independent primary-seat test source, not an
        # agent overlay or a background input call. Every command is read back.
        offsets = ([(x, 0) for x in range(20, 161, 20)] + [(160, y) for y in range(20, 161, 20)]
                   + [(x, 160) for x in range(140, -1, -20)] + [(0, y) for y in range(140, -1, -20)])
        try:
            index = 0
            while not motion_done.wait(0.1):
                dx, dy = offsets[index % len(offsets)]; index += 1
                x, y = int(fg['x'] + point[0] + dx), int(fg['y'] + point[1] + dy)
                assert fg['x'] < x < fg['x'] + fg['width'] and fg['y'] < y < fg['y'] + fg['height']
                expected_motion.append([x, y])
                mark('primary_motion_command', x=x, y=y)
                grab.stdin.write(f'MOVE {x} {y}\n'); grab.stdin.flush()
                assert select.select([grab.stdout], [], [], 2)[0], 'primary command acknowledgement missing'
                assert grab.stdout.readline().strip() == f'MOVED {x} {y}'
        except Exception as error:
            motion_errors.append(str(error))

    try:
        for index, spec in enumerate(plan['agents']):
            mcp = client(args.evidence / ('agent-' + str(index)))
            clients.append(mcp)
            started_session = mcp.tool('start_session', {'session': spec['name']})
            assert not started_session.get('isError'), started_session
            first = snapshot(mcp, spec['target'], spec['name'])
            assert first['window_bounds'] == spec['bounds'], 'plan geometry is stale'
            pending = mcp.tool('press_key', {**spec['target'], 'key': 'Escape', 'session': spec['name']})
            snapshot(mcp, spec['target'], spec['name'])
            request = pending.get('structuredContent', {})
            assert pending.get('isError') and request.get('reason') == 'pending_operator_approval', pending
            (args.evidence / f'grant-request-{index}.json').write_text(json.dumps(request))
        print('GRANTS_REQUIRED', flush=True)
        wait_for(lambda: all((args.evidence / f'grant-{i}.json').exists() for i in range(len(clients))), 180)
        recorder = client(args.evidence / 'observer')
        snapshot(recorder, foreground)
        desktop = recorder.tool('get_desktop_state', {})['structuredContent']
        fg = snapshot(recorder, foreground)['window_bounds']
        point = plan.get('primary_point', [300, 300])
        grab = subprocess.Popen([str(args.primary_grab), str(fg['x'] + point[0]), str(fg['y'] + point[1]),
                                 str(desktop['screen_width']), str(desktop['screen_height']), '60000']
                                + (['controlled'] if expected_motion is not None else []),
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        assert wait_for(lambda: select.select([grab.stdout], [], [], 0)[0])
        assert grab.stdout.readline().strip() == 'HELD'
        wait_for(lambda: state(args.foreground_journal)['held'])
        snapshot(recorder, foreground)
        primary_before = wm()
        assert primary_before['pid'] == foreground['pid']
        baseline = state(args.foreground_journal)
        # Establish the independent primary hold before approval. A grant for a
        # currently foreground client is correctly revoked by the compositor.
        for index in range(len(clients)):
            request = json.loads((args.evidence / f'grant-request-{index}.json').read_text())
            grant = json.loads((args.evidence / f'grant-{index}.json').read_text())
            assert all(grant[key] == request[key] for key in ('epoch', 'challenge', 'target'))
            endpoint = 'cua-input-test.sock' if request['lane'] == 0 else 'cua-input-test-2.sock'
            operator = Trace(args.input_directory / endpoint)
            operators.append(operator)
            operator.exchange(grant['packet'])
        trace.exchange('TRACE_START')
        watcher = threading.Thread(target=watch)
        watcher.start()
        mark('video_start_call')
        recording = recorder.tool('start_recording', {'output_dir': str(args.evidence / 'video'), 'record_video': True})
        mark('video_start_response')
        assert not recording.get('isError'), recording
        assert recording['structuredContent']['video_active'], recording
        if expected_motion is not None:
            mover = threading.Thread(target=move_primary)
            mover.start()
        started = time.monotonic()
        results = []
        for phase in plan['phases']:
            time.sleep(phase.get('pause_before', 0))
            if phase.get('negative_control'):
                assert expected_motion is None, 'negative control requires a parked baseline'
                snapshot(recorder, foreground)
                mark('negative_control_start')
                subprocess.run([str(args.primary_grab), str(fg['x'] + point[0]), str(fg['y'] + point[1]),
                                str(desktop['screen_width']), str(desktop['screen_height']), '100', 'canary'], check=True)
                mark('negative_control_end')
                snapshot(recorder, foreground)
                assert wm() == primary_before, 'negative control must return to identical endpoints'
            elif 'parallel' in phase:
                ready = threading.Barrier(len(phase['parallel']))
                control_offset = trace.exchange('TRACE_READ 0')['count']
                with ThreadPoolExecutor(max_workers=len(clients)) as pool:
                    futures = [pool.submit(action, step['agent'], step, ready) for step in phase['parallel']]
                    if 'control' in phase:
                        started_actors = set()
                        def both_drags_started():
                            nonlocal control_offset
                            page = trace.exchange('TRACE_READ ' + str(control_offset))
                            control_offset += len(page['events'])
                            started_actors.update(row[5] for row in page['events'] if row[2] == 'agent_drag_start')
                            return started_actors == {1, 2}
                        wait_for(both_drags_started, 12)
                        time.sleep(0.25)
                        control = phase['control']
                        mark('operator_control', command=control['command'], agent=control['agent'])
                        operators[control['agent']].exchange(control['command'])
                    results.extend(future.result() for future in futures)
            else:
                results.append(action(phase['agent'], phase))
        time.sleep(max(0, plan.get('minimum_video_seconds', 30) - (time.monotonic() - started)))
        motion_done.set()
        if mover: mover.join(timeout=5)
        assert not motion_errors, motion_errors
        snapshot(recorder, foreground)
        mark('video_stop_call')
        stopped = recorder.tool('stop_recording', {})
        mark('video_stop_response')
        assert not stopped.get('isError') and not stopped['structuredContent'].get('last_error'), stopped
        trace.exchange('TRACE_STOP')
        done.set(); watcher.join(timeout=5)
        data = trace.collect()
        report = analyze(data, expected_motion=expected_motion)
        (args.evidence / 'expected-primary-motion.json').write_text(json.dumps(expected_motion))
        if args.metrics:
            args.metrics.write_text(json.dumps({'status': report['result'],
                'cursor_violations': report.get('uncommanded_motion_events'),
                'focus_events': report.get('focus_events'),
                'releases': report.get('unexpected_button_releases'),
                'actions': report.get('agent_action_completions')}))
        (args.evidence / 'trace.json').write_text(json.dumps(data, indent=2))
        (args.evidence / 'isolation.json').write_text(json.dumps(report, indent=2))
        if plan.get('expect_detected_theft'):
            assert report['result'] == 'failed' and report['uncommanded_motion_events'] > 0, report
            assert 49 < report['max_primary_displacement_px'] <= 50, report
            assert data['events'][0][3:5] == data['events'][-1][3:5], 'negative control endpoints differ'
        else:
            assert report['result'] == 'passed', report
        if plan.get('require_overlap'):
            assert report['agent_drag_overlap_ms'] >= 100, report
        assert released_synthetic_input(data)
        outputs = []
        for output_index, oracle in enumerate(plan.get('outputs', [])):
            content = Path(oracle['path']).read_bytes()
            saved = args.evidence / 'saved-outputs'
            saved.mkdir(exist_ok=True)
            (saved / f'{output_index}-before{Path(oracle["path"]).suffix}').write_bytes(before_app_files[oracle['path']])
            (saved / f'{output_index}-after{Path(oracle["path"]).suffix}').write_bytes(content)
            assert content != before_app_files[oracle['path']], 'application did not save a changed file'
            tree = ET.fromstring(content)
            node = tree.find(oracle['xpath'], oracle.get('namespaces', {}))
            assert node is not None, oracle
            for key, expected in oracle.get('attributes', {}).items():
                assert node.get(key) == expected, (key, node.attrib)
            if 'rect_translation' in oracle:
                original = ET.fromstring(before_app_files[oracle['path']]).find(oracle['xpath'], oracle.get('namespaces', {}))
                assert original is not None
                before_xy, after_xy = rect_position(original), rect_position(node)
                delta = [b - a for a, b in zip(before_xy, after_xy)]
                assert all(low <= value <= high for value, (low, high) in zip(delta, oracle['rect_translation'])), delta
                assert all(node.get(key) == original.get(key) for key in ('width', 'height')), 'shape was resized'
            outputs.append({'path': Path(oracle['path']).name, 'verified': True, 'attributes': node.attrib})
        result = {'result': 'passed', 'isolation': report, 'outputs': outputs,
                  'driver_actions': len(results), 'autonomous_model_benchmark': False,
                  'synthetic_input_released': True,
                  'harness_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  'negative_control': bool(plan.get('expect_detected_theft')),
                  'video': str(args.evidence / 'video' / 'recording.mp4')}
    finally:
        motion_done.set()
        if mover: mover.join(timeout=5)
        done.set()
        if watcher: watcher.join(timeout=6)
        try:
            trace.exchange('TRACE_STOP')
            data = trace.collect()
            if not (args.evidence / 'trace.json').exists():
                (args.evidence / 'trace.json').write_text(json.dumps(data, indent=2))
                (args.evidence / 'isolation.json').write_text(json.dumps(analyze(data, expected_motion=expected_motion), indent=2))
        except (OSError, RuntimeError): pass
        operations = []
        if recorder:
            operations += [('stop_video', lambda: recorder.tool('stop_recording', {})), ('close_recorder', recorder.close)]
        for index, operator in enumerate(operators):
            operations += [(f'cancel_{index}', lambda op=operator: op.exchange('CANCEL')),
                           (f'close_operator_{index}', operator.close)]
        def release_primary():
            if grab and grab.poll() is None:
                grab.terminate(); grab.wait(timeout=5)
            if grab:
                wait_for(lambda: not state(args.foreground_journal)['held'])
        operations.append(('release_primary', release_primary))
        operations.extend((f'close_agent_{i}', mcp.close) for i, mcp in enumerate(clients))
        operations.append(('close_trace', trace.close))
        cleanup_errors = cleanup_all(operations)
        (args.evidence / 'cleanup.json').write_text(json.dumps({'errors': cleanup_errors}, indent=2))
        if args.metrics and (result is None or cleanup_errors):
            args.metrics.write_text(json.dumps({'status': 'failed', 'cursor_violations': None,
                'focus_events': None, 'releases': None, 'actions': None}))
    assert not cleanup_errors, cleanup_errors
    (args.evidence / 'result.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('driver', 'driver-socket', 'input-directory', 'primary-grab', 'plan', 'evidence', 'foreground-journal'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--metrics', type=Path)
    run(parser.parse_args())
