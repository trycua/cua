import argparse
import json
import re
import selectors
import subprocess
import time
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('--binary', required=True)
p.add_argument('--fixture', required=True)
p.add_argument('--sha', required=True)
p.add_argument('--label', required=True)
p.add_argument('--block', type=int, required=True)
p.add_argument('--samples', type=int, default=20)
p.add_argument('--output', required=True)
a = p.parse_args()


mcp_log = Path(a.output).with_name('mcp-' + a.label + '-' + str(a.block) + '.log').open('w')
socket = str(Path.home() / 'Library/Caches/cua-driver-local/cua-driver-local.sock')
mcp = subprocess.Popen([a.binary, 'mcp', '--socket', socket], stdin=subprocess.PIPE,
                       stdout=subprocess.PIPE, stderr=mcp_log, text=True, bufsize=1)
selector = selectors.DefaultSelector()
selector.register(mcp.stdout, selectors.EVENT_READ)
sequence = 0


def rpc(method, params):
    global sequence
    sequence += 1
    mcp.stdin.write(json.dumps(dict(jsonrpc='2.0', id=sequence, method=method, params=params)) + '\n')
    mcp.stdin.flush()
    deadline = time.monotonic() + 30
    while True:
        assert selector.select(max(0, deadline - time.monotonic())), 'MCP response timeout'
        line = mcp.stdout.readline()
        assert line, 'MCP process exited'
        response = json.loads(line)
        if response.get('id') == sequence:
            assert 'error' not in response, response
            return response['result']


def call(tool, **args):
    result = rpc('tools/call', dict(name=tool, arguments=args))
    assert not result.get('isError'), result
    state = dict(result.get('structuredContent') or {})
    state['text'] = '\n'.join(c['text'] for c in result.get('content', []) if c.get('type') == 'text')
    for content in result.get('content', []):
        if content.get('type') == 'image':
            state['screenshot_png_b64'] = content['data']
    return state


rpc('initialize', {})
assert call('get_config')['source_sha'] == a.sha
app = subprocess.Popen([a.fixture], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    deadline = time.monotonic() + 10
    while True:
        windows = call('list_windows', pid=app.pid)['windows']
        windows = [w for w in windows if 'CuaTestHarness' in w.get('title', '')]
        if windows:
            break
        assert time.monotonic() < deadline, 'fixture window missing'
        time.sleep(.1)
    wid = windows[0]['window_id']
    target = {'pid': app.pid, 'window_id': wid}
    call('bring_to_front', **target)

    def snapshot():
        state = call('get_window_state', **target, include_screenshot=True)
        assert state['screenshot_png_b64'] and state['elements'] and state['snapshot_id']
        return state

    def element(state, identifier):
        line = next(line for line in state['tree_markdown'].splitlines() if 'id=' + identifier in line)
        index = int(re.search(r'\[(\d+)\]', line).group(1))
        return next(e for e in state['elements'] if e['element_index'] == index)

    def counter(state):
        return int(re.search(r'counter=(\d+)', state['tree_markdown']).group(1))

    with Path(a.output).open('a') as out:
        for iteration in range(a.samples + 5):
            for workload in ('snapshot', 'semantic', 'pixel_background', 'text'):
                start = time.perf_counter_ns()
                before = snapshot()
                if workload == 'snapshot':
                    after = before
                else:
                    control = element(before, 'txt-input' if workload == 'text' else 'btn-increment')
                    args = dict(target, delivery_mode='foreground')
                    if workload == 'pixel_background':
                        args['delivery_mode'] = 'background'
                        frame, bounds = control['frame'], before['window_bounds']
                        scale = before['screenshot_scale']
                        args.update(x=(frame['x'] + frame['w'] / 2 - bounds['x']) * scale,
                                    y=(frame['y'] + frame['h'] / 2 - bounds['y']) * scale)
                        assert 0 <= args['x'] < before['screenshot_width'] and 0 <= args['y'] < before['screenshot_height']
                    else:
                        args['element_token'] = control['element_token']
                    if workload == 'text':
                        value = 'latency-' + str(iteration)
                        action_result = call('set_value', **args, value=value)
                    else:
                        if workload == 'semantic':
                            args['action'] = 'press'
                        action_result = call('click', **args)
                    deadline = time.monotonic() + 3
                    while True:
                        after = snapshot()
                        correct = value in after['tree_markdown'] if workload == 'text' else counter(after) == counter(before) + 1
                        if correct:
                            break
                        if time.monotonic() >= deadline:
                            failure = dict(workload=workload, arguments=args, action_result=action_result,
                                           before={k: v for k, v in before.items() if k != 'screenshot_png_b64'},
                                           after={k: v for k, v in after.items() if k != 'screenshot_png_b64'})
                            Path(a.output).with_name('failure.json').write_text(json.dumps(failure, indent=2))
                            raise AssertionError('native action effect missing: ' + str(action_result))
                        time.sleep(.01)
                elapsed = time.perf_counter_ns() - start
                row = dict(label=a.label, sha=a.sha, block=a.block, iteration=iteration,
                           warmup=iteration < 5, workload=workload, ns=elapsed,
                           screenshot_width=after['screenshot_width'], screenshot_height=after['screenshot_height'])
                out.write(json.dumps(row) + '\n')
                out.flush()
finally:
    app.terminate()
    try:
        app.wait(timeout=5)
    except subprocess.TimeoutExpired:
        app.kill()
        app.wait()
    mcp.stdin.close()
    try:
        mcp.wait(timeout=5)
    except subprocess.TimeoutExpired:
        mcp.terminate()
        mcp.wait(timeout=5)
    selector.close()
    mcp_log.close()
