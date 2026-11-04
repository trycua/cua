"""Shared synthetic fixtures for the Hyprland proof-harness tests.

This is not a test module: discovery loads only ``*_test.py``. Proofs that
hash their own tests into ``provenance.json`` also hash this file, because
those tests are only reproducible together with it.
"""
from copy import deepcopy
import io
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch
import zipfile


# Trace pages. Rows are [sequence, timestamp_ns, kind, x, y, lane, value].

def page(rows, active=True):
    """One complete, healthy trace page around already-built rows."""
    return {'hook': True, 'active': active, 'overflow': False, 'timed_out': False,
            'count': len(rows), 'events': rows}


def trace(rows, active=True, *, point=(100, 100), unit=1_000_000):
    """Page from ``(time, kind, lane, value)`` rows; time is in ``unit`` ns."""
    return page([[i + 1, int(time * unit), kind, *point, lane, value]
                 for i, (time, kind, lane, value) in enumerate(rows)], active)


def conflict_trace(events=(), active=True):
    """Page with its own start (and stop when inactive) at the primary point."""
    rows = [(0, 'start', 0, 0), *events, *(() if active else ((20, 'stop', 0, 0),))]
    return trace(rows, active, point=(30, 40), unit=1)


START = ('start', 100, 200, 0, 0)
STOP = ('stop', 100, 200, 0, 0)


def primary_trace(*events):
    """Stopped page from ``(kind, x, y, lane, value)`` events, 1 ms apart."""
    return page([[i + 1, i * 1_000_000, kind, x, y, lane, value]
                 for i, (kind, x, y, lane, value) in enumerate(events)], active=False)


ACTIVE = [(0, 'start', 0, 0), (1, 'agent_admitted', 1, 0), (2, 'agent_drag_start', 1, 0),
          (3, 'pointer_button', 1, 1), (4, 'pointer_motion', 1, 0)]
CANCEL = ACTIVE + [(8, 'agent_cancel', 1, 0), (9, 'pointer_button', 1, 0), (10, 'pointer_leave', 1, 0)]


# Production v3 plugin status.

def lane(index, **values):
    """One inert, unreserved compositor lane; override fields by keyword."""
    return {'lane': index, 'epoch': str(index + 1) * 32, 'desktop_generation': 1, 'dispatches': 0,
            'held_button': 0, 'held_keys': 0, 'drag_active': False, 'lease_active': False,
            'pointer_focus': False, 'keyboard_focus': False, 'reserved': False, **values}


def lane_status(*lanes):
    """Complete ``cua:status`` for a production v3 build (two inert lanes by default)."""
    return {'state': 'input_v3_candidate', 'configured': True, 'transport': {'ready': True}, 'input': {
        'protocol': 3, 'test_only': False, 'seat_lifetime': 'compositor',
        'upgrade': 'desktop_restart', 'transport_ready': True,
        'lanes': list(lanes) or [lane(0), lane(1)]}}


HELD = {'held_button': 272, 'drag_active': True, 'lease_active': True,
        'pointer_focus': True, 'reserved': True}


def status(generation=1, held=False):
    """Two lanes at one desktop generation; ``held`` gives lane 0 a held drag."""
    return lane_status(*(lane(index, desktop_generation=generation, **(HELD if held and index == 0 else {}))
                         for index in (0, 1)))


def retained_status(generation=2, lane=1):
    """Cleared status that keeps passive pointer focus on one (1-based) lane."""
    value = status(generation)
    value['input']['lanes'][lane - 1]['pointer_focus'] = True
    return value


def motion_gate(record, lane=1):
    """A 13px surface-local movement, with the same held lane across status."""
    gate_page = trace([(0, 'start', 0, 0), (1, 'agent_admitted', lane, 0),
                       (2, 'agent_drag_start', lane, 0), (2.5, 'pointer_enter', lane, 0),
                       (3, 'pointer_button', lane, 1), (4, 'pointer_motion', lane, 0)])
    for row in gate_page['events']:
        if row[2] in ('pointer_enter', 'pointer_motion'):
            row.extend([10 if row[2] == 'pointer_enter' else 23, 20])
    gate = status(1, held=True)
    if lane == 2:
        gate['input']['lanes'].reverse()
        for index, row in enumerate(gate['input']['lanes']):
            row['lane'], row['epoch'] = index, str(index + 1) * 32
    record.update(pointer_cleanup='retained_inert', min_motion_px=12,
                  prefix=gate_page, gate_first=deepcopy(gate_page), lane=lane,
                  status_started_ns=5_000_000, gate_status=gate, after=retained_status(2, lane))
    boundary = deepcopy(gate_page)
    boundary['events'] += [[7, 8_000_000, 'agent_cancel', 100, 100, lane, 0],
                           [8, 9_000_000, 'pointer_button', 100, 100, lane, 0]]
    boundary['count'] = len(boundary['events'])
    return boundary


# Driver responses.

PARTIAL = {'structuredContent': {'effect': 'partial', 'route': 'synthetic_events',
                                 'delivery': {'mode': 'background', 'delivered_count': 1}}}
DELIVERED = {'structuredContent': {'effect': 'unverifiable', 'route': 'synthetic_events',
                                   'delivery': {'mode': 'background'}}}
REFUSED = {'isError': True, 'structuredContent': {'effect': 'refused', 'reason': 'session_unavailable'}}


def action(response=PARTIAL):
    """One unreplayed Driver response retained by a fault proof."""
    return {'outcome': 'response', 'response': deepcopy(response), 'replayed': False}


# Processes and runtimes.

def identity(pid, exe='/usr/bin/python3'):
    return {'pid': pid, 'uid': 1000, 'starttime': '123', 'exe': exe}


def client(pid, alive=True):
    """Owned Driver runtime whose kill/terminate are observable through ``poll``."""
    process = Mock(pid=pid, poll=Mock(return_value=None if alive else 0))
    process.kill.side_effect = lambda: setattr(process.poll, 'return_value', -9)
    process.terminate.side_effect = lambda: setattr(process.poll, 'return_value', -15)
    return Mock(directory=Path.cwd(), process=process, failed=False)


# Reviewed plans shared by several proofs.

BOUNDS = {'x': 10, 'y': 20, 'width': 800, 'height': 600}
VM = {'machine_id': 'a' * 32, 'boot_id': 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'}


def plan():
    """Two-lane Calc/Inkscape real-app plan."""
    return {'purpose': 'apps', 'foreground': {'pid': 10, 'window_id': 100},
            'agents': [{'app': 'calc', 'target': {'pid': 20, 'window_id': 200}},
                       {'app': 'inkscape', 'target': {'pid': 30, 'window_id': 300}}],
            'phases': [{'parallel': [{'agent': 0, 'tool': 'drag', 'arguments': {}},
                                     {'agent': 1, 'tool': 'drag', 'arguments': {}}]}],
            'outputs': [{'agent': 0, 'attributes': {'value': '96'}},
                        {'agent': 1, 'rect_translation': [[10, 20], [30, 40]]}]}


def capacity_plan():
    result = {**plan(), 'purpose': 'capacity', 'outputs': []}
    result['agents'].append({'app': 'inkscape', 'target': {'pid': 40, 'window_id': 400}})
    for spec in result['agents']:
        spec.update(name='same-public-name', profile={'mode': 'standard'},
                    bounds={'x': 0, 'y': 0, 'width': 600, 'height': 600})
    result['phases'] = [{'agent': index, 'tool': 'click', 'arguments': {'x': 20, 'y': 30}}
                        for index in range(3)]
    result['phases'][2]['expect'] = {'kind': 'refused', 'reason': 'lane_busy'}
    return result


def capacity_events(lane):
    return [('agent_admitted', 100, 200, lane, 0),
            ('pointer_button', 100, 200, lane, 1),
            ('pointer_button', 100, 200, lane, 0),
            ('agent_action_end', 100, 200, lane, 0)]


def policy_cache_plan():
    result = capacity_plan()
    result.update(purpose='policy_cache', agents=result['agents'][:1])
    result['agents'][0]['profile'].update(manifest='reviewed.yaml', approve_manifest=True)
    result['phases'] = [
        {'agent': 0, 'tool': 'click', 'arguments': {'x': 20, 'y': 30}},
        {'agent': 0, 'tool': 'press_key', 'arguments': {'key': 'ESC'},
         'expect': {'kind': 'refused', 'reason': 'permission_denied',
                    'message': "Permission denied: capability manifest denies tool 'press_key'"}},
        {'agent': 0, 'tool': 'click', 'arguments': {'x': 30, 'y': 40}},
    ]
    return result


def inkscape_plan(capacity=False):
    """The opt-in two-Inkscape real-app (or capacity) plan with saved SVG oracles."""
    candidate = capacity_plan() if capacity else plan()
    candidate['app_profile'] = 'inkscape-only'
    candidate['package_versions'] = {'inkscape': '1.4.4-6'}
    for index, spec in enumerate(candidate['agents']):
        spec.update(app='inkscape', document=f'/synthetic/lane-{index}.svg')
    candidate['outputs'] = [] if capacity else [
        {'agent': i, 'path': spec['document'], 'format': 'svg',
         'xpath': './/svg:rect[@id="smoke-rectangle"]',
         'namespaces': {'svg': 'http://www.w3.org/2000/svg'},
         'rect_translation': [[2, 2], [0, 0]]}
        for i, spec in enumerate(candidate['agents'])]
    return candidate


GEOMETRY_STAGES = {'calc': 'select_range', 'inkscape': 'move_rectangle'}


def geometry_plan(kind='move', app='calc'):
    return {'purpose': 'geometry_fault', 'disposable': True, 'compositor': {'pid': 50, 'instance': 'test_1'},
            'foreground': {'pid': 10, 'window_id': 100}, 'primary_point': [20, 20],
            'agents': [{'app': app, 'name': 'geometry', 'target': {'pid': 20, 'window_id': 200},
                        'bounds': dict(BOUNDS), 'pointer_stage': GEOMETRY_STAGES[app], 'drag': {}}],
            'fault': {'kind': kind, 'to': [30, 40] if kind == 'move' else [820, 620]},
            'recovery': {'pointer_stage': 'click_b2' if app == 'calc' else 'scroll_down'}}


MONITOR = {'id': 0, 'name': 'Virtual-1', 'width': 1280, 'height': 800,
           'x': 0, 'y': 0, 'scale': 1.0, 'transform': 0}


def session_plan():
    return {'purpose': 'session_fault', 'disposable': True, 'fault': {'kind': 'dpms'},
            'vm': {'machine_id': '1' * 32, 'boot_id': '12345678-1234-1234-1234-123456789abc'},
            'compositor': {**identity(50, '/usr/bin/Hyprland'), 'instance': 'test_1'},
            'foreground': {'pid': 10, 'window_id': 100}, 'primary_point': [20, 20],
            'identities': {'foreground': identity(10), 'target': identity(20, '/usr/bin/soffice.bin')},
            'foreground_fixture': {'sha256': 'a' * 64, 'journal': {'path': '/test/foreground.jsonl',
                'device': 1, 'inode': 2, 'uid': 1000}}, 'monitors': [dict(MONITOR)],
            'agents': [{'app': 'calc', 'name': 'session', 'target': {'pid': 20, 'window_id': 200},
                'bounds': dict(BOUNDS), 'pointer_stage': 'select_range', 'drag': {}}],
            'recovery': {'pointer_stage': 'click_b2'}}


def lock_plan():
    return {**session_plan(), 'purpose': 'lock_refusal', 'fault': {'kind': 'lock'},
            'lock_fixture': {'path': '/test/session_lock_fixture', 'device': 1, 'inode': 2,
                             'uid': 1000, 'sha256': 'b' * 64, 'source_sha256': 'c' * 64}}


def primary_conflict_plan(app='calc'):
    return {'purpose': 'primary_conflict', 'case': 'initial_refusal', 'disposable': True, 'vm': dict(VM),
            'compositor': {**identity(50, '/usr/bin/Hyprland'), 'instance': 'test_1'},
            'processes': {'target': identity(20), 'foreground': identity(10)},
            'foreground': {'pid': 10, 'window_id': 100}, 'primary_point': [20, 20], 'package_versions': {},
            'agents': [{'app': app, 'name': 'primary-conflict', 'target': {'pid': 20, 'window_id': 200},
                        'bounds': dict(BOUNDS), 'pointer_stage': GEOMETRY_STAGES[app], 'drag': {}}],
            'recovery': {'pointer_stage': 'click_b2' if app == 'calc' else 'scroll_down'}}


def inkscape_profile(candidate, *, drag=True):
    """Convert a Calc fault plan to the opt-in ``inkscape-only`` profile."""
    candidate = deepcopy(candidate)
    candidate['app_profile'] = 'inkscape-only'
    for index, spec in enumerate(candidate['agents']):
        spec.update(app='inkscape', document=f'/synthetic/agent-{index}/cua-smoke-inkscape.svg')
        if drag:
            spec.update(pointer_stage='move_rectangle', drag={})
    for key in ('identities', 'processes'):
        if key in candidate:
            candidate[key]['target']['exe'] = '/usr/bin/inkscape'
    if 'pointer_stage' in candidate.get('recovery', {}):
        candidate['recovery'] = {'pointer_stage': 'scroll_down'}
    return candidate


# Accessibility snapshots and synthetic pixels.

CALC = {'window_title': 'cua-smoke-calc.ods - LibreOffice Calc',
        'elements': [{'role': 'text', 'label': 'Name Box', 'value': 'A1'}]}
INKSCAPE = {
    'elements': [
        {'element_index': 10, 'role': 'menu', 'label': 'Edit', 'enabled': True},
        {'element_index': 11, 'parent_index': 10, 'role': 'menu item',
         'label': 'Select All', 'enabled': True},
        {'element_index': 12, 'role': 'table cell', 'label': 'smoke-rectangle', 'enabled': True}],
    'tree_markdown': '\n'.join([
        '  - [10] menu "Edit" [actions=[click]]',
        '    - [11] menu item "Select All" [actions=[click]]',
        '  - [12] table cell "smoke-rectangle" [actions=[activate]]',
        '  - label = "No objects selected. Click, Shift+click, Alt+scroll mouse on top of '
        'objects, or drag around objects to select."']),
}
INKSCAPE_SELECTED = {
    'elements': [{'element_index': 1, 'role': 'table cell',
                  'label': 'smoke-rectangle', 'enabled': True}] + [
        {'element_index': index, 'role': 'spin button', 'label': f'{value:.3f}',
         'value': f'{value:.1f}', 'enabled': True}
        for index, value in enumerate((40, 60, 80, 50), 2)],
    'tree_markdown': '\n'.join([
        '  - [1] table cell "smoke-rectangle" [actions=[activate]]',
        '  - label = "Rectangle  in root. Click selection again to toggle scale/rotation handles."',
        *[f'  - label = "{axis}:"\n  - [{index}] spin button "{value:.3f}" '
          f'value="{value:.1f}" [actions=[activate]]'
          for index, (axis, value) in enumerate((('X', 40), ('Y', 60), ('W', 80), ('H', 50)), 2)]]),
}


def changed_ods(original, text='abc', empty_first=False):
    destination = io.BytesIO()
    with zipfile.ZipFile(io.BytesIO(original)) as source, zipfile.ZipFile(destination, 'w') as target:
        for name in source.namelist():
            value = source.read(name)
            if name == 'content.xml':
                cell = (f'<table:table-cell office:value-type="string"><text:p>{text}</text:p>'
                        '</table:table-cell>')
                if empty_first:
                    cell = '<table:table-cell/>' + cell
                value = value.replace(b'<table:table-cell/>', cell.encode())
            target.writestr(name, value)
    return destination.getvalue()


class Image:
    width = height = 500

    def __init__(self):
        self.points = {}

    def rgb(self, x, y):
        assert 0 <= x < self.width and 0 <= y < self.height
        return self.points.get((x, y), (255, 255, 255))

    def rectangle(self, x, y, w, h):
        self.points.update({(a, b): (51, 102, 153) for a in range(x, x + w) for b in range(y, y + h)})


def app_snapshot(app):
    return {'window_title': f'cua-smoke-{app}', 'window_bounds': {'x': 600, 'y': 400, 'width': 500, 'height': 500},
            'screenshot_width': 500, 'screenshot_height': 500}


def ink(selected=True, dx=0, dy=0, scroll_y=0):
    """Inkscape snapshot and pixels for the smoke rectangle, optionally moved."""
    state = {**app_snapshot('inkscape'), **deepcopy(INKSCAPE_SELECTED if selected else INKSCAPE)}
    if selected:
        for row in state['elements']:
            if row['role'] == 'spin button':
                row['frame'] = {'x': 700, 'y': 435, 'w': 100, 'h': 34}
        for index, value in ((2, 40 + dx * .8), (3, 60 + dy * .8)):
            row = next(row for row in state['elements'] if row['element_index'] == index)
            state['tree_markdown'] = state['tree_markdown'].replace(
                f'[{index}] spin button "{row["label"]}" value="{row["value"]}"',
                f'[{index}] spin button "{value:.3f}" value="{value:.1f}"')
            row.update(label=f'{value:.3f}', value=f'{value:.1f}')
    image = Image()
    image.rectangle(120 + dx, 150 + dy + scroll_y, 100, 60)
    return state, image


# Real-app runner scaffolding (production_realapp_proof.run).

def harness_args(root, candidate, **overrides):
    """Write ``candidate`` as the reviewed plan and return runner arguments."""
    path = Path(root) / 'plan.json'
    path.write_text(json.dumps(candidate))
    values = {'plan': path, 'evidence': Path(root) / 'evidence', 'trace_socket': None,
              'driver': Path(root) / 'driver', 'primary_grab': Path(root) / 'primary-grab',
              'foreground_journal': Path(root) / 'journal', 'record_video': False}
    return SimpleNamespace(**{**values, **overrides})


def run_replacements(case, held, **overrides):
    """Desktop-free replacements for the runner's native primary and provenance reads."""
    return {'provenance': Mock(return_value={}),
            'subprocess.Popen': Mock(return_value=Mock(poll=Mock(return_value=None))),
            'primary_acknowledgement': Mock(return_value='HELD\n'),
            'wait_for': lambda predicate: case.assertTrue(predicate()),
            'state': lambda path: {'held': held[0], 'clicks': 0, 'keys': 0, 'scroll': 0},
            'wm': lambda: {'pid': 10, 'address': '0x10', 'workspace': 1, 'cursor': {'x': 100, 'y': 200}},
            'stop_process': lambda process: held.__setitem__(0, False),
            **overrides}


def patch_module(stack, module, replacements):
    for name, replacement in replacements.items():
        stack.enter_context(patch(module + '.' + name, replacement))


# Table-driven refusals.

def assert_rejects(case, check, base, mutations, error=AssertionError):
    """Each ``(pattern, mutate)`` row must fail ``check`` with its own message."""
    for pattern, mutate in mutations:
        candidate = deepcopy(base)
        mutate(candidate)
        with case.subTest(pattern=pattern), case.assertRaisesRegex(error, pattern):
            check(candidate)
