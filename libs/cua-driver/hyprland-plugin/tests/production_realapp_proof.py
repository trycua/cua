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
import os
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


def manifest_tool_messages(tool):
    # authorization.rs -> AuthorizationError::Denied -> permission_denied_result.
    return {f"Permission denied: capability manifest denies tool '{tool}'",
            f"Permission denied: tool '{tool}' is outside the capability manifest"}


def validate_plan(plan):
    assert plan['purpose'] in ('apps', 'policy', 'policy_cache', 'negative_control', 'capacity')
    assert type(plan.get('moving_primary', False)) is bool
    assert not (plan.get('moving_primary') and plan['purpose'] == 'negative_control'), \
        'negative control requires a parked primary'
    capacity = plan['purpose'] == 'capacity'
    policy_cache = plan['purpose'] == 'policy_cache'
    assert (len(plan['agents']) == 3 if capacity else 1 <= len(plan['agents']) <= 2)
    if policy_cache:
        assert len(plan['agents']) == 1, 'policy_cache needs one persistent runtime'
        assert not plan.get('moving_primary') and not plan.get('require_overlap'), \
            'policy_cache requires serial actions and a parked primary'
        spec = plan['agents'][0]
        assert spec['app'] in ('calc', 'inkscape')
        assert isinstance(spec['name'], str) and spec['name'], 'policy_cache needs a fixed session'
        profile = spec['profile']
        assert profile['mode'] in ('standard', 'bounded', 'unrestricted')
        assert profile.get('manifest') and profile.get('approve_manifest') is True
        assert profile['mode'] != 'unrestricted' or profile.get('acknowledge_unrestricted') is True
        assert len(plan['phases']) == 3, 'policy_cache needs allow, deny, fresh allow'
        for index, step in enumerate(plan['phases']):
            assert 'parallel' not in step and step.get('agent') == 0, 'policy_cache must reuse agent 0 serially'
            expected = step.get('expect', {'kind': 'dispatched'})
            if index == 1:
                assert expected.get('kind') == 'refused' and expected.get('reason') == 'permission_denied'
                assert expected.get('message') in manifest_tool_messages(step['tool']), \
                    'policy_cache needs an exact manifest tool-ceiling refusal'
            else:
                assert expected == {'kind': 'dispatched'}, 'policy_cache permitted calls must dispatch'
        tools = [step['tool'] for step in plan['phases']]
        assert tools[0] == tools[2] and tools[0] != tools[1], 'policy_cache needs a distinct denied tool'
    if capacity:
        assert not plan.get('moving_primary'), 'capacity requires a parked primary'
        assert not plan.get('require_overlap'), 'capacity establishes persistent lanes serially'
        assert {spec['app'] for spec in plan['agents'][:2]} == {'calc', 'inkscape'}
        assert all(spec['app'] in ('calc', 'inkscape') for spec in plan['agents'])
        assert len(plan['phases']) == 3, 'capacity needs two admissions and one refusal'
        for index, step in enumerate(plan['phases']):
            assert 'parallel' not in step and step.get('agent') == index, 'capacity must run agents 0, 1, 2 serially'
            expected = {'kind': 'dispatched'} if index < 2 else {'kind': 'refused', 'reason': 'lane_busy'}
            assert step.get('expect', {'kind': 'dispatched'}) == expected, 'incorrect capacity expectation'
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
        if capacity or policy_cache:
            assert type(target['window_id']) is int and target['window_id'] > 0
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
        policy_refusal = content.get('status') == 'refused' and isinstance(content.get('refusal'), dict)
        reason = content['refusal'].get('code') if policy_refusal else content.get('reason')
        assert result.get('isError') is True and reason == expected['reason'], result
        assert content.get('effect') == 'refused' or (policy_refusal and 'effect' not in content), result
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


def trace_interval(before, after):
    """Validate complete active prefixes before interpreting their difference."""
    for page in (before, after):
        assert isinstance(page, dict), 'missing dispatch telemetry'
        assert page.get('hook') is True and page.get('active') is True
        assert page.get('overflow') is False and page.get('timed_out') is False
        rows = page.get('events')
        assert isinstance(rows, list) and rows, 'empty dispatch telemetry'
        assert type(page.get('count')) is int and page['count'] == len(rows), 'incomplete dispatch telemetry'
        assert isinstance(rows[-1], list) and len(rows[-1]) == 7, 'malformed dispatch telemetry'
        # Reuse the canonical row/order validator with a local end sentinel;
        # the real stopped trace is still required during cleanup.
        last = rows[-1]
        assert type(last[0]) is int, 'malformed trace sequence'
        stopped = {**page, 'active': False, 'count': len(rows) + 1,
                   'events': rows + [[last[0] + 1, last[1], 'stop', *last[3:5], 0, 0]]}
        assert analyze(stopped).get('telemetry_complete') is True, 'incomplete dispatch telemetry'
        assert not any(row[2] == 'stop' for row in rows), 'trace already stopped'
    assert after['events'][:before['count']] == before['events'], 'trace history changed'
    return after['events'][before['count']:]


def assert_no_dispatch(before, after):
    """Zero completions alone is insufficient: reject every synthetic event."""
    events = trace_interval(before, after)
    assert not any(row[5] in (1, 2) or row[2] == 'agent_admitted' for row in events), \
        'denied call reached a synthetic lane'


def capacity_lane(before, after, tool):
    """Identify one admitted, exercised, completed compositor lane, not a PID."""
    events = trace_interval(before, after)
    synthetic = [row for row in events if row[5] in (1, 2)]
    lanes = {row[5] for row in synthetic}
    assert len(lanes) == 1, 'capacity action must exercise exactly one compositor lane'
    input_kind = {'click': 'pointer_button', 'drag': 'pointer_button',
                  'scroll': 'pointer_axis', 'press_key': 'keyboard_key', 'hotkey': 'keyboard_key'}[tool]
    completion = 'agent_drag_end' if tool == 'drag' else 'agent_action_end'
    admissions = [row[0] for row in synthetic if row[2] == 'agent_admitted']
    inputs = [row[0] for row in synthetic if row[2] == input_kind]
    completions = [row[0] for row in synthetic if row[2] == completion]
    assert admissions and inputs and completions, 'capacity needs admission, input, and completion evidence'
    assert min(admissions) < min(inputs) <= max(inputs) < max(completions), 'unordered capacity dispatch'
    return lanes.pop()


def verify_capacity(actions):
    assert len(actions) == 3 and [row['agent'] for row in actions] == [0, 1, 2], 'incomplete capacity actions'
    assert all(row['expected'] == 'dispatched' for row in actions[:2])
    for row in actions[:2]:
        check_response({'structuredContent': row['observed']}, {'kind': 'dispatched'})
    assert {row.get('compositor_lane') for row in actions[:2]} == {1, 2}, 'capacity needs distinct compositor lanes'
    refusal = actions[2]
    assert refusal['expected'] == 'refused' and refusal.get('no_dispatch') == 'verified'
    check_response({'isError': True, 'structuredContent': refusal['observed']},
                   {'kind': 'refused', 'reason': 'lane_busy'})
    return {'result': 'verified', 'lanes': [row['compositor_lane'] for row in actions[:2]],
            'refused_agent': 2, 'reason': 'lane_busy'}


def check_manifest_refusal(response, expected, tool):
    result = check_response(response, expected)
    content = result['observed']
    assert content.get('status') == 'refused' and isinstance(content.get('refusal'), dict), \
        'policy_cache needs the common authorization refusal envelope'
    assert expected['reason'] == 'permission_denied'
    assert expected['message'] in manifest_tool_messages(tool)
    assert content['refusal'].get('message') == expected['message'], 'wrong manifest tool-ceiling refusal'
    return result


def verify_policy_cache(actions):
    assert len(actions) == 3 and [row['agent'] for row in actions] == [0, 0, 0]
    assert [row['expected'] for row in actions] == ['dispatched', 'refused', 'dispatched']
    assert actions[0]['tool'] == actions[2]['tool'] != actions[1]['tool']
    assert len({row['runtime_pid'] for row in actions}) == 1, 'policy_cache runtime changed'
    assert len({row['session'] for row in actions}) == 1, 'policy_cache session changed'
    lanes = [actions[index].get('compositor_lane') for index in (0, 2)]
    assert lanes[0] in (1, 2) and lanes[0] == lanes[1], 'policy_cache compositor lane changed'
    for index in (0, 2):
        check_response({'structuredContent': actions[index]['observed']}, {'kind': 'dispatched'})
    denied = actions[1]
    assert denied.get('no_dispatch') == 'verified'
    check_manifest_refusal({'isError': True, 'structuredContent': denied['observed']},
                           denied['expect'], denied['tool'])
    return {'result': 'verified', 'runtime_pid': actions[0]['runtime_pid'],
            'session': actions[0]['session'], 'compositor_lane': lanes[0],
            'reason': denied['expect']['reason'], 'message': denied['expect']['message'],
            'scope': 'cached-connection-manifest-tool-ceiling'}


def primary_trajectory(bounds, point, desktop):
    """The independent primary-grab helper follows the historical 160px square."""
    offsets = ([(x, 0) for x in range(20, 161, 20)] + [(160, y) for y in range(20, 161, 20)]
               + [(x, 160) for x in range(140, -1, -20)] + [(0, y) for y in range(140, -1, -20)])
    positions = [[int(bounds['x'] + point[0] + dx), int(bounds['y'] + point[1] + dy)]
                 for dx, dy in offsets]
    for x, y in positions:
        assert bounds['x'] < x < bounds['x'] + bounds['width'], 'primary trajectory leaves foreground'
        assert bounds['y'] < y < bounds['y'] + bounds['height'], 'primary trajectory leaves foreground'
        assert 0 <= x < desktop['screen_width'] and 0 <= y < desktop['screen_height'], \
            'primary trajectory leaves desktop'
    return positions


def primary_acknowledgement(stream):
    """Bound partial lines too: select followed by readline can block forever."""
    deadline, data = time.monotonic() + 2, b''
    while not data.endswith(b'\n'):
        remaining = deadline - time.monotonic()
        assert remaining > 0 and select.select([stream], [], [], remaining)[0], \
            f'primary command acknowledgement missing or incomplete: {data!r}'
        chunk = os.read(stream.fileno(), 128)
        assert chunk, f'primary command acknowledgement ended early: {data!r}'
        data += chunk
        assert len(data) <= 128, 'oversized primary command acknowledgement'
    return data.decode('ascii')


def move_primary(grab, trajectory, done, ready, commands, mark):
    """Send independent primary-seat commands, retaining every issued command."""
    while not done.wait(0.1):
        x, y = trajectory[len(commands) % len(trajectory)]
        row = {'sequence': len(commands) + 1, 'x': x, 'y': y,
               'command_ns': time.monotonic_ns(), 'acknowledgement': None, 'ack_ns': None}
        commands.append(row)
        mark('primary_motion_command', **row)
        grab.stdin.write(f'MOVE {x} {y}\n')
        grab.stdin.flush()
        row['acknowledgement'] = primary_acknowledgement(grab.stdout)
        row['ack_ns'] = time.monotonic_ns()
        mark('primary_motion_acknowledgement', **row)
        assert row['acknowledgement'] == f'MOVED {x} {y}\n', 'malformed primary command acknowledgement'
        ready.set()


def expected_primary_motion(commands, trajectory):
    """Only a complete, ordered command/acknowledgement log can define motion."""
    assert isinstance(commands, list) and commands, 'missing primary command log'
    expected, previous_ack = [], 0
    for index, row in enumerate(commands):
        assert isinstance(row, dict), 'malformed primary command log'
        assert type(row.get('sequence')) is int and row['sequence'] == index + 1, \
            'incomplete or reordered primary command log'
        assert all(type(row.get(key)) is int for key in ('x', 'y', 'command_ns', 'ack_ns')), \
            'incomplete primary command acknowledgement'
        point = [row['x'], row['y']]
        assert point == trajectory[index % len(trajectory)], 'primary command differs from trajectory'
        assert previous_ack < row['command_ns'] <= row['ack_ns'], 'unordered primary command timestamps'
        assert row.get('acknowledgement') == f'MOVED {point[0]} {point[1]}\n', \
            'malformed primary command acknowledgement'
        previous_ack = row['ack_ns']
        expected.append(point)
    return expected


def assert_primary_state(before, after, moving):
    keys = ('pid', 'address', 'workspace') if moving else before.keys()
    assert all(after[key] == before[key] for key in keys), 'primary cursor/focus/workspace changed'


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
    files = {'driver': args.driver, 'plugin': args.plugin, 'primary-grab': args.primary_grab}
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
    capacity = plan['purpose'] == 'capacity'
    policy_cache = plan['purpose'] == 'policy_cache'
    args.evidence.mkdir(parents=True, exist_ok=False)
    def save(name, value):
        (args.evidence / name).write_text(json.dumps(value, indent=2))
    save('plan.json', plan)
    clients, recorder, grab, trace = [], None, None, None
    moving = plan.get('moving_primary', False)
    mover = None
    motion_done, motion_ready = threading.Event(), threading.Event()
    commands, motion_errors, action_intervals = [], [], []
    capacity_traces = []
    policy_cache_traces = []
    trajectory = None
    recording = False
    baseline_outputs = {}
    report = {'result': 'failed', 'scope': 'native-production-input-proof',
              'full_desktop_matrix': False, 'actions': [], 'outputs': [],
              'continuous_isolation': 'unproven', 'synthetic_cleanup': 'unproven',
              'primary_mode': 'moving' if moving else 'parked'}
    if capacity:
        report['capacity'] = {'result': 'unproven'}
    if policy_cache:
        report['policy_cache'] = {'result': 'unproven'}
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
        if capacity or policy_cache:
            windows = mcp.tool('list_windows', {})
            assert not windows.get('isError'), windows
            matches = [window for window in windows['structuredContent']['windows']
                       if window.get('pid') == target['pid']]
            assert len(matches) == 1 and matches[0].get('window_id') == target['window_id'], \
                'reviewed PID/window identity is stale or ambiguous'
        result = mcp.tool('get_window_state', {**target, 'max_elements': 100, 'max_depth': 6,
                          **({'session': session} if session else {})})
        assert not result.get('isError'), result
        content = result['structuredContent']
        assert content.get('screenshot_width', 0) > 0, 'missing grounding image'
        return content
    def action(step, barrier=None):
        index = step['agent']
        spec, mcp = plan['agents'][index], clients[index]
        if policy_cache:
            assert assert_distinct_runtimes(clients) == report['driver_processes'], 'policy_cache runtime changed'
            current_trace = trace.collect()
            trace_before = policy_cache_traces[-1] if policy_cache_traces else current_trace
            assert_no_dispatch(trace_before, current_trace)
        before = snapshot(mcp, spec['target'], spec['name'])
        assert before['window_bounds'] == spec['bounds'], 'reviewed geometry is stale'
        if policy_cache:
            # Keep the full interval quiet through fresh grounding, including
            # delayed events after the preceding denied response.
            assert_no_dispatch(trace_before, trace.collect())
        expected = step.get('expect', {'kind': 'dispatched'})
        if capacity:
            assert_distinct_runtimes(clients)
        if not policy_cache:
            trace_before = trace.collect() if trace and (capacity or expected['kind'] == 'refused') else None
        if capacity:
            assert_no_dispatch(capacity_traces[-1] if capacity_traces else trace_before, trace_before)
        if barrier:
            barrier.wait(timeout=20)
        mark('action_start', agent=index, tool=step['tool'], runtime_pid=mcp.process.pid)
        action_start = time.monotonic_ns()
        try:
            response = mcp.tool(step['tool'], {**step['arguments'], **spec['target'],
                                'session': spec['name'], 'delivery_mode': 'background'})
        except Exception:
            # A transport failure poisons that runtime. Preserve a fresh independent
            # after-snapshot when available without retrying the mutation.
            with observer_lock:
                snapshot(recorder, spec['target'])
            raise
        action_intervals.append((action_start, time.monotonic_ns()))
        mark('action_response', agent=index, response=response.get('structuredContent'), error=response.get('isError', False))
        after = snapshot(mcp, spec['target'], spec['name'])
        result = check_response(response, expected)
        if policy_cache:
            trace_after = trace.collect()
            save(f'policy-cache-phase-{len(policy_cache_traces)}-trace.json',
                 {'before': trace_before, 'after': trace_after})
            policy_cache_traces.append(trace_after)
            if expected['kind'] == 'dispatched':
                result['compositor_lane'] = capacity_lane(trace_before, trace_after, step['tool'])
            else:
                result = check_manifest_refusal(response, expected, step['tool'])
                assert_no_dispatch(trace_before, trace_after)
                result.update(no_dispatch='verified', expect=expected)
            result.update(runtime_pid=mcp.process.pid, session=spec['name'])
            assert assert_distinct_runtimes(clients) == report['driver_processes'], 'policy_cache runtime changed'
        if capacity and expected['kind'] == 'dispatched':
            trace_after = trace.collect()
            save(f'capacity-agent-{index}-trace.json', {'before': trace_before, 'after': trace_after})
            capacity_traces.append(trace_after)
            result['compositor_lane'] = capacity_lane(trace_before, trace_after, step['tool'])
            assert result['compositor_lane'] not in {row.get('compositor_lane') for row in report['actions']}, \
                'capacity needs distinct compositor lanes'
        if expected['kind'] == 'refused' and not policy_cache:
            result['no_dispatch'] = 'unproven'
            if trace:
                trace_after = trace.collect()
                if capacity:
                    save(f'capacity-agent-{index}-trace.json', {'before': trace_before, 'after': trace_after})
                    capacity_traces.append(trace_after)
                assert_no_dispatch(trace_before, trace_after)
                result['no_dispatch'] = 'verified'
        if capacity:
            assert_distinct_runtimes(clients)
        assert after['window_bounds'] == before['window_bounds']
        assert_primary_state(primary_before, wm(), moving)
        current = state(args.foreground_journal)
        assert all(current[key] == baseline[key] for key in ('clicks', 'keys', 'scroll', 'held')), current
        return {'agent': index, 'tool': step['tool'], **result}
    try:
        assert not moving or args.trace_socket, 'moving primary requires continuous trace'
        assert not capacity or args.trace_socket, 'capacity requires continuous trace'
        assert not policy_cache or args.trace_socket, 'policy_cache requires continuous trace'
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
        recorder = client('observer', {'mode': 'unrestricted', 'acknowledge_unrestricted': True})
        fg = snapshot(recorder, plan['foreground'])['window_bounds']
        desktop = recorder.tool('get_desktop_state', {})['structuredContent']
        point = plan.get('primary_point', [300, 300])
        assert 0 < point[0] < fg['width'] and 0 < point[1] < fg['height']
        if moving:
            trajectory = primary_trajectory(fg, point, desktop)
        grab_args = [str(args.primary_grab), str(fg['x'] + point[0]), str(fg['y'] + point[1]),
                     str(desktop['screen_width']), str(desktop['screen_height'])]
        snapshot(recorder, plan['foreground'])
        grab = subprocess.Popen(grab_args + ['60000'] + (['controlled'] if moving else []),
                                stdin=subprocess.PIPE if moving else None,
                                stdout=subprocess.PIPE, text=True)
        assert primary_acknowledgement(grab.stdout) == 'HELD\n'
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
        if moving:
            def movement():
                try:
                    move_primary(grab, trajectory, motion_done, motion_ready, commands, mark)
                except Exception as error:
                    motion_errors.append(str(error))
                    motion_ready.set()
            mover = threading.Thread(target=movement, daemon=True)
            mover.start()
            assert motion_ready.wait(3), 'primary motion did not start'
            assert not motion_errors, motion_errors
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
        if capacity:
            report['capacity'] = verify_capacity(report['actions'])
        if policy_cache:
            report['policy_cache'] = verify_policy_cache(report['actions'])
        for index, oracle in enumerate(plan.get('outputs', [])):
            report['outputs'].append(verify_output(baseline_outputs[index], Path(oracle['path']).read_bytes(), oracle))
        report['result'] = 'passed'
    except Exception as error:
        report['error'] = str(error)
    finally:
        # Preserve app files even after partial/unknown responses or failed assertions.
        operations = []
        if moving:
            def stop_motion():
                motion_done.set()
                try:
                    if mover:
                        mover.join(timeout=3)
                        assert not mover.is_alive(), 'primary motion worker did not stop'
                finally:
                    save('primary-motion-commands.json', commands)
                assert not motion_errors, motion_errors
            operations.append(('stop_primary_motion', stop_motion))
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
                if capacity_traces:
                    last = capacity_traces[-1]
                    assert data['events'][:last['count']] == last['events'], 'capacity trace history changed at cleanup'
                if policy_cache_traces:
                    last = policy_cache_traces[-1]
                    assert data['events'][:last['count']] == last['events'], 'policy_cache trace history changed at cleanup'
                expected_motion = None
                if moving:
                    assert not mover or not mover.is_alive(), 'primary motion worker did not stop'
                    logged = json.loads((args.evidence / 'primary-motion-commands.json').read_text())
                    expected_motion = expected_primary_motion(logged, trajectory)
                    save('expected-primary-motion.json', expected_motion)
                    overlaps = sum(any(start <= row['command_ns'] <= row['ack_ns'] <= end
                                       for start, end in action_intervals) for row in logged)
                    report['primary_commands_during_actions'] = overlaps
                    assert overlaps > 0, 'no primary movement during a Driver action'
                isolation = analyze(data, expected_motion=expected_motion)
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
        if mover:
            def join_motion():
                # Reaping the helper also unblocks a pending acknowledgement.
                # Retain late failure evidence even when the first join failed.
                try:
                    mover.join(timeout=3)
                    assert not mover.is_alive(), 'primary motion worker did not stop after release'
                finally:
                    save('primary-motion-commands.json', commands)
            operations.append(('join_primary_motion', join_motion))
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
