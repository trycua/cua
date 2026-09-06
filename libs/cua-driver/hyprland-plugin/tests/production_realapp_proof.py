"""Reviewed native Calc/Inkscape plans through normal direct Driver MCP.

Run only in a prepared disposable Hyprland desktop. This runner never signs a
grant, sends TARGET/input packets, or installs policy. A trace socket is optional
for package smoke and mandatory for continuous isolation/no-dispatch proof.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
from pathlib import Path
import re
import select
import subprocess
import threading
import time
import xml.etree.ElementTree as ET
import zipfile

from driver_input_live import state, wait_for, wm
from primary_trace import Trace, analyze
from production_mcp import DirectMCP, assert_distinct_runtimes, stop_process
from realapp_proof import cleanup_all, rect_position, released_synthetic_input


TOOLS = {'click', 'press_key', 'hotkey', 'scroll', 'drag'}
RESERVED = {'pid', 'window_id', 'session', 'delivery_mode'}


def validate_plan(plan):
    assert plan['purpose'] in ('apps', 'policy', 'negative_control')
    assert 1 <= len(plan['agents']) <= 2
    if plan['purpose'] == 'apps':
        assert len(plan['agents']) == 2
        assert {spec['app'] for spec in plan['agents']} == {'calc', 'inkscape'}
        assert {oracle['agent'] for oracle in plan['outputs']} == {0, 1}
    targets = [spec['target'] for spec in plan['agents']]
    assert len({target['pid'] for target in targets}) == len(targets), 'apps must be distinct processes'
    assert plan['foreground']['pid'] not in {target['pid'] for target in targets}
    for target in targets:
        assert set(target) == {'pid', 'window_id'}
        assert type(target['pid']) is int and target['pid'] > 0
    assert plan['phases'], 'empty plan cannot pass'
    for phase in plan['phases']:
        if phase.get('negative_control'):
            assert plan['purpose'] == 'negative_control'
            continue
        assert plan['purpose'] != 'negative_control'
        steps = phase.get('parallel', [phase])
        indexes = [step['agent'] for step in steps]
        assert len(indexes) == len(set(indexes)), 'concurrent calls need separate runtimes'
        assert all(type(i) is int and 0 <= i < len(targets) for i in indexes)
        for step in steps:
            assert step['tool'] in TOOLS
            assert not RESERVED.intersection(step['arguments']), 'action overrides reviewed ownership'
            expected = step.get('expect', {'kind': 'dispatched'})
            assert expected['kind'] in ('dispatched', 'refused', 'partial', 'unknown')
            if expected['kind'] == 'refused':
                assert expected['reason']
                assert len(steps) == 1, 'no-dispatch proof needs a quiet measurement interval'
            if plan['purpose'] == 'policy':
                assert expected['kind'] == 'refused', 'policy plans measure denial, not app effects'
    for oracle in plan.get('outputs', []):
        assert 0 <= oracle['agent'] < len(targets)
        assert oracle.get('attributes') or oracle.get('rect_translation') or 'text' in oracle
        if 'rect_translation' in oracle:
            assert len(oracle['rect_translation']) == 2
            assert all(len(bounds) == 2 and bounds[0] <= bounds[1] for bounds in oracle['rect_translation'])


def check_response(result, expected):
    """Classify delivery independently from app effect; never retry any case."""
    content = result.get('structuredContent', {})
    kind = expected['kind']
    delivery = content.get('delivery')
    if kind == 'refused':
        assert result.get('isError') is True and content.get('reason') == expected['reason'], result
        assert content.get('effect') == 'refused', result
        assert delivery is None, 'refusal must not imply acknowledged delivery'
    elif kind == 'partial':
        assert content.get('effect') == 'partial', result
        assert isinstance(delivery, dict) and type(delivery.get('delivered_count')) is int, result
        assert delivery['delivered_count'] >= 0
    elif kind == 'unknown':
        assert isinstance(delivery, dict) and delivery.get('mode') == 'unknown', result
        assert content.get('effect') in ('partial', 'unverifiable'), result
    else:
        assert not result.get('isError') and content.get('effect') in ('confirmed', 'unverifiable'), result
        assert delivery is None or delivery.get('mode') == 'background', result
    if kind != 'refused':
        assert content.get('route') == 'synthetic_events', 'not plugin input evidence'
    return {'expected': kind, 'observed': content, 'app_effect_verified': False}


def assert_no_dispatch(before, after):
    """Zero completions alone is insufficient: reject every synthetic event."""
    for page in (before, after):
        assert page.get('hook') is True and page.get('active') is True
        assert page.get('overflow') is False and page.get('timed_out') is False
        assert page['count'] == len(page['events']), 'incomplete dispatch telemetry'
    assert after['events'][:before['count']] == before['events'], 'trace history changed'
    events = after['events'][before['count']:]
    assert not any(row[5] in (1, 2) for row in events), 'denied call reached a synthetic lane'


def document_root(content, oracle):
    if oracle.get('zip_member'):
        with zipfile.ZipFile(io.BytesIO(content)) as archive:
            content = archive.read(oracle['zip_member'])
    return ET.fromstring(content)


def verify_output(before, after, oracle):
    assert before != after, 'application did not save a changed file'
    node = document_root(after, oracle).find(oracle['xpath'], oracle.get('namespaces', {}))
    assert node is not None, 'saved document lacks expected node'
    for key, expected in oracle.get('attributes', {}).items():
        assert node.get(key) == expected, (key, node.attrib)
    if 'text' in oracle:
        assert ''.join(node.itertext()) == oracle['text']
    if 'rect_translation' in oracle:
        original = document_root(before, oracle).find(oracle['xpath'], oracle.get('namespaces', {}))
        assert original is not None
        delta = [b - a for a, b in zip(rect_position(original), rect_position(node))]
        assert all(low <= value <= high for value, (low, high) in zip(delta, oracle['rect_translation'])), delta
        assert all(node.get(key) == original.get(key) for key in ('width', 'height'))
    return {'agent': oracle['agent'], 'file': Path(oracle['path']).name, 'verified': True}


def provenance(args, plan):
    def read(command):
        return subprocess.check_output(command, text=True, timeout=10).strip()
    source = args.source.resolve()
    assert Path(read(['git', '-C', str(source), 'rev-parse', '--show-toplevel'])).resolve() == source
    sha = read(['git', '-C', str(source), 'rev-parse', 'HEAD'])
    assert sha == args.source_sha, 'source SHA differs from declared candidate'
    files = {'driver': args.driver, 'plugin': args.plugin}
    for name in ('production_realapp_proof.py', 'production_mcp.py', 'driver_input_live.py',
                 'realapp_proof.py', 'primary_trace.py'):
        files[name] = Path(__file__).with_name(name)
    windows = json.loads(read(['hyprctl', '-j', 'clients']))
    identities = {}
    for index, spec in enumerate(plan['agents']):
        matches = [window for window in windows if window.get('pid') == spec['target']['pid']]
        assert len(matches) == 1 and matches[0].get('xwayland') is False, 'need one exact native window per app'
        executable = Path(f'/proc/{spec["target"]["pid"]}/exe').resolve(strict=True)
        if spec.get('app') in ('calc', 'inkscape'):
            assert executable.name == {'calc': 'soffice.bin', 'inkscape': 'inkscape'}[spec['app']]
        identities[str(index)] = {'pid': spec['target']['pid'], 'executable': str(executable),
                                  'hyprland_window': matches[0],
                                  'sha256': hashlib.sha256(executable.read_bytes()).hexdigest()}
    packages = read(['pacman', '-Q', 'libreoffice-fresh', 'inkscape'])
    expected = plan['package_versions']
    assert expected == {'libreoffice-fresh': '26.2.5-3', 'inkscape': '1.4.4-6'}, 'unqualified package selection'
    assert dict(line.split() for line in packages.splitlines()) == expected, 'package qualification mismatch'
    driver_source = (source / 'libs/cua-driver/rust/Cargo.toml').read_text()
    plugin_source = (source / 'libs/cua-driver/hyprland-plugin/CMakeLists.txt').read_text()
    return {'source': str(source), 'source_sha': sha,
            'branch': read(['git', '-C', str(source), 'branch', '--show-current']),
            'dirty': read(['git', '-C', str(source), 'status', '--porcelain']),
            'source_versions': {'driver': re.search(r'(?m)^version = "([^"]+)"', driver_source)[1],
                                'plugin': re.search(r'project\(cua_hyprland_plugin VERSION ([\d.]+)', plugin_source)[1]},
            'driver_version': read([str(args.driver), '--version']),
            'hyprland': read(['hyprctl', 'version']), 'packages': packages,
            'loaded_plugins': read(['hyprctl', 'plugin', 'list']),
            'files': {name: {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
                      for name, path in files.items()}, 'app_processes': identities}


def run(args):
    if not __debug__:
        raise RuntimeError('assertions must be enabled')
    plan = json.loads(args.plan.read_text())
    validate_plan(plan)
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2))
    save('plan.json', plan)
    clients, recorder, grab, trace = [], None, None, None
    recording = False
    baseline_outputs = {}
    report = {'result': 'failed', 'scope': 'native-production-input-proof',
              'full_desktop_matrix': False, 'actions': [], 'outputs': [],
              'continuous_isolation': 'unproven', 'synthetic_cleanup': 'unproven'}
    timeline_lock = threading.Lock()
    observer_lock = threading.Lock()
    def mark(event, **fields):
        with timeline_lock, (args.evidence / 'timeline.jsonl').open('a') as stream:
            stream.write(json.dumps({'monotonic_ns': time.monotonic_ns(), 'event': event, **fields}) + '\n')
    def client(name, profile):
        directory = args.evidence / name
        directory.mkdir()
        return DirectMCP(args.driver, directory, profile)
    def snapshot(mcp, target, session=None):
        result = mcp.tool('get_window_state', {**target, 'max_elements': 100, 'max_depth': 6,
                          **({'session': session} if session else {})})
        assert not result.get('isError'), result
        content = result['structuredContent']
        assert content.get('screenshot_width', 0) > 0, 'missing grounding image'
        return content
    def action(step, barrier=None):
        index = step['agent']
        spec, mcp = plan['agents'][index], clients[index]
        before = snapshot(mcp, spec['target'], spec['name'])
        assert before['window_bounds'] == spec['bounds'], 'reviewed geometry is stale'
        expected = step.get('expect', {'kind': 'dispatched'})
        trace_before = trace.collect() if trace and expected['kind'] == 'refused' else None
        if barrier:
            barrier.wait(timeout=20)
        mark('action_start', agent=index, tool=step['tool'], runtime_pid=mcp.process.pid)
        try:
            response = mcp.tool(step['tool'], {**step['arguments'], **spec['target'],
                                'session': spec['name'], 'delivery_mode': 'background'})
        except Exception:
            # A transport failure poisons that runtime. Preserve a fresh independent
            # after-snapshot when available without retrying the mutation.
            with observer_lock:
                snapshot(recorder, spec['target'])
            raise
        mark('action_response', agent=index, response=response.get('structuredContent'), error=response.get('isError', False))
        after = snapshot(mcp, spec['target'], spec['name'])
        result = check_response(response, expected)
        if expected['kind'] == 'refused':
            result['no_dispatch'] = 'unproven'
            if trace:
                assert_no_dispatch(trace_before, trace.collect())
                result['no_dispatch'] = 'verified'
        assert after['window_bounds'] == before['window_bounds']
        assert wm() == primary_before, 'primary cursor/focus/workspace changed'
        current = state(args.foreground_journal)
        assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held')), current
        return {'agent': index, 'tool': step['tool'], **result}
    try:
        save('provenance.json', provenance(args, plan))
        for index, oracle in enumerate(plan.get('outputs', [])):
            baseline_outputs[index] = Path(oracle['path']).read_bytes()
        for index, spec in enumerate(plan['agents']):
            mcp = client(f'agent-{index}', spec['profile'])
            clients.append(mcp)
            started = mcp.tool('start_session', {'session': spec['name']})
            assert not started.get('isError'), started
            assert snapshot(mcp, spec['target'], spec['name'])['window_bounds'] == spec['bounds']
        report['driver_processes'] = assert_distinct_runtimes(clients)
        recorder = client('observer', {'mode': 'standard'})
        fg = snapshot(recorder, plan['foreground'])['window_bounds']
        desktop = recorder.tool('get_desktop_state', {})['structuredContent']
        point = plan.get('primary_point', [300, 300])
        assert 0 < point[0] < fg['width'] and 0 < point[1] < fg['height']
        grab_args = [str(args.primary_grab), str(fg['x'] + point[0]), str(fg['y'] + point[1]),
                     str(desktop['screen_width']), str(desktop['screen_height'])]
        snapshot(recorder, plan['foreground'])
        grab = subprocess.Popen(grab_args + ['60000'], stdout=subprocess.PIPE, text=True)
        wait_for(lambda: select.select([grab.stdout], [], [], 0)[0])
        assert grab.stdout.readline().strip() == 'HELD'
        wait_for(lambda: state(args.foreground_journal)['held'])
        snapshot(recorder, plan['foreground'])
        primary_before, baseline = wm(), state(args.foreground_journal)
        assert primary_before['pid'] == plan['foreground']['pid']
        if args.trace_socket:
            assert args.trace_socket.name in ('cua-input-v3.sock', 'cua-input-v3-2.sock')
            trace = Trace(args.trace_socket)
            assert trace.hello['protocol'] == 3
            trace.exchange('TRACE_START')
        if args.record_video:
            video = recorder.tool('start_recording', {'output_dir': str(args.evidence / 'video'), 'record_video': True})
            assert not video.get('isError') and video['structuredContent']['video_active'], video
            recording = True
        for phase in plan['phases']:
            if phase.get('negative_control'):
                assert trace, 'warp-and-return detector requires continuous trace'
                snapshot(recorder, plan['foreground'])
                subprocess.run(grab_args + ['100', 'canary'], check=True, timeout=10)
                snapshot(recorder, plan['foreground'])
                assert wm() == primary_before, 'control failed to return to identical endpoints'
            elif 'parallel' in phase:
                barrier = threading.Barrier(len(phase['parallel']))
                with ThreadPoolExecutor(max_workers=len(clients)) as pool:
                    futures = [pool.submit(action, step, barrier) for step in phase['parallel']]
                    report['actions'].extend(future.result() for future in futures)
            else:
                report['actions'].append(action(phase))
        for index, oracle in enumerate(plan.get('outputs', [])):
            report['outputs'].append(verify_output(baseline_outputs[index], Path(oracle['path']).read_bytes(), oracle))
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        # Preserve app files even after partial/unknown responses or failed assertions.
        operations = []
        def preserve(index, oracle):
            directory = args.evidence / 'saved-outputs'
            directory.mkdir(exist_ok=True)
            suffix = Path(oracle['path']).suffix
            if index in baseline_outputs:
                (directory / f'{index}-before{suffix}').write_bytes(baseline_outputs[index])
            (directory / f'{index}-after{suffix}').write_bytes(Path(oracle['path']).read_bytes())
        operations.extend((f'preserve_output_{i}', lambda i=i, o=o: preserve(i, o))
                          for i, o in enumerate(plan.get('outputs', [])))
        if recording:
            def stop_video():
                result = recorder.tool('stop_recording', {})
                assert not result.get('isError') and not result['structuredContent'].get('last_error'), result
            operations.append(('stop_video', stop_video))
        operations.extend((f'close_agent_{i}', mcp.close) for i, mcp in enumerate(clients))
        if trace:
            def finish_trace():
                trace.exchange('TRACE_STOP')
                data = trace.collect()
                save('trace.json', data)
                isolation = analyze(data)
                save('isolation.json', isolation)
                report['continuous_isolation'] = isolation['result']
                if plan['purpose'] == 'negative_control':
                    assert isolation['result'] == 'failed' and isolation['uncommanded_motion_events'] > 0
                    assert data['events'][0][3:5] == data['events'][-1][3:5]
                    report['negative_control_detected'] = True
                else:
                    assert isolation['result'] == 'passed', isolation
                if plan.get('require_overlap'):
                    assert isolation['agent_drag_overlap_ms'] >= 100, 'no proven two-lane overlap'
                assert released_synthetic_input(data)
                report['synthetic_cleanup'] = 'verified'
            operations += [('finish_trace', finish_trace), ('close_trace', trace.close)]
        def release_primary():
            if grab:
                if grab.poll() is None:
                    grab.terminate()
                stop_process(grab)
                wait_for(lambda: not state(args.foreground_journal)['held'])
        operations.append(('release_primary', release_primary))
        if recorder:
            operations.append(('close_observer', recorder.close))
        errors = cleanup_all(operations)
        save('cleanup.json', {'errors': errors})
        if errors:
            report['result'] = 'failed'
        elif report['result'] == 'passed' and not trace:
            report['scope'] = 'production-package-smoke'
            if plan['purpose'] != 'apps' or plan.get('require_overlap'):
                report['result'] = 'inconclusive'
        save('result.json', report)
    print(json.dumps(report), flush=True)
    return 0 if report['result'] == 'passed' else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('driver', 'plugin', 'source', 'primary-grab', 'plan', 'evidence', 'foreground-journal'):
        parser.add_argument('--' + name, required=True, type=Path)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--trace-socket', type=Path)
    parser.add_argument('--record-video', action='store_true')
    raise SystemExit(run(parser.parse_args()))
