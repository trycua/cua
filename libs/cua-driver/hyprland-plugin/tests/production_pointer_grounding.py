"""Fresh-pixel grounding for bounded native Calc/Inkscape pointer proofs.

No captured coordinates are reused across calls. GTK's PNG reader is needed
only on the native test guest; the geometry algorithms have portable tests.
These helpers inspect synthetic documents, not arbitrary application content.
"""
import math
import re

from production_app_smoke import GroundingUnavailable, calc_formula_selection, rows


STAGES = {
    'calc': {'click_b2': 'click', 'click_a1': 'click', 'scroll_down': 'scroll',
             'scroll_up': 'scroll', 'select_range': 'drag'},
    'inkscape': {'click_rectangle': 'click', 'scroll_down': 'scroll',
                 'scroll_up': 'scroll', 'move_rectangle': 'drag'},
}


class Pixels:
    def __init__(self, width, height, data, stride, channels=3):
        self.width, self.height = width, height
        self.data, self.stride, self.channels = data, stride, channels

    def rgb(self, x, y):
        assert 0 <= x < self.width and 0 <= y < self.height
        offset = y * self.stride + x * self.channels
        return tuple(self.data[offset:offset + 3])


def read_pixels(path):
    import gi
    gi.require_version('GdkPixbuf', '2.0')
    from gi.repository import GdkPixbuf
    image = GdkPixbuf.Pixbuf.new_from_file(str(path))
    assert image.get_bits_per_sample() == 8 and image.get_n_channels() in (3, 4)
    assert 0 < image.get_width() <= 1568 and 0 < image.get_height() <= 1568
    return Pixels(image.get_width(), image.get_height(), image.get_pixels(),
                  image.get_rowstride(), image.get_n_channels())


def checked_snapshot(snapshot, image, app):
    elements = rows(snapshot)
    bounds = snapshot['window_bounds']
    assert (image.width, image.height) == (snapshot['screenshot_width'], snapshot['screenshot_height'])
    assert (image.width, image.height) == (bounds['width'], bounds['height']), 'only native 1:1 pixels qualified'
    assert f'cua-smoke-{app}' in snapshot['window_title'], 'wrong synthetic document'
    return elements


def local_frame(row, bounds, image):
    frame = row.get('frame', {})
    if set(frame) != {'x', 'y', 'w', 'h'} or not all(type(value) is int for value in frame.values()):
        raise GroundingUnavailable('missing integer pixel frame')
    x, y, w, h = frame['x'] - bounds['x'], frame['y'] - bounds['y'], frame['w'], frame['h']
    if not (0 <= x < x + w <= image.width and 0 <= y < y + h <= image.height):
        raise GroundingUnavailable('accessible frame is clipped or outside the snapshot')
    return x, y, w, h


def calc_table(snapshot, image):
    elements = checked_snapshot(snapshot, image, 'calc')
    tables = [row for row in elements if row.get('role') == 'table' and row.get('label') == 'Sheet Smoke']
    if len(tables) != 1:
        raise GroundingUnavailable('need exactly one synthetic Calc sheet')
    return local_frame(tables[0], snapshot['window_bounds'], image)


def calc_scroll(snapshot, image):
    candidates = []
    for row in rows(snapshot):
        if row.get('role') != 'scroll bar' or not row.get('frame'):
            continue
        frame = row['frame']
        if not (0 < frame.get('w', 0) < 30 and frame.get('h', 0) > 100):
            continue
        local_frame(row, snapshot['window_bounds'], image)
        candidates.append(float(row['value']))
    if len(candidates) != 1 or not math.isfinite(candidates[0]) or candidates[0] < 0:
        raise GroundingUnavailable('need one visible finite vertical scrollbar')
    return candidates[0]


def calc_cells(snapshot, image):
    x, y, w, h = calc_table(snapshot, image)
    if calc_scroll(snapshot, image) != 0:
        raise GroundingUnavailable('cell proof requires the top of the sheet')
    # The table frame excludes headers. First establish uniform columns from
    # their full-height lines. A short range selection can obscure the upper
    # part of these lines without hiding their positions below the selection.
    columns = [px for px in range(x, x + w)
               if sum(image.rgb(px, py) == (204, 204, 204) for py in range(y, y + h)) >= h * .70]
    if len(columns) < 3:
        raise GroundingUnavailable('cannot resolve visible spreadsheet grid')
    columns = [x - 1, *columns]

    def uniform(positions, minimum):
        gaps = [b - a for a, b in zip(positions, positions[1:])]
        if min(gaps) < minimum or max(gaps) - min(gaps) > 2:
            raise GroundingUnavailable('spreadsheet grid is ambiguous or irregular')

    uniform(columns, 30)
    # Inspect each complete column interior independently at the same 70%
    # coverage. A highlighted A1:B3 leaves C and D available to corroborate
    # the early row boundaries even when a full-width scan cannot see them.
    # Keep every observed boundary: conflicting strips must fail, not be
    # discarded in favor of a majority or an extrapolated hidden grid.
    support = {}
    for left, right in zip(columns, columns[1:]):
        for py in range(y, y + h):
            if sum(image.rgb(px, py) == (204, 204, 204)
                   for px in range(left + 1, right)) >= (right - left - 1) * .70:
                support[py] = support.get(py, 0) + 1
    if len(support) < 4:
        raise GroundingUnavailable('cannot resolve visible spreadsheet grid')
    lines = [y - 1, *sorted(support)]
    uniform(lines, 8)
    if min(support.values()) < 2:
        raise GroundingUnavailable('spreadsheet rows need two independent column strips')
    return {'A1': ((columns[0] + columns[1]) // 2, (lines[0] + lines[1]) // 2),
            'B2': ((columns[1] + columns[2]) // 2, (lines[1] + lines[2]) // 2),
            'B3': ((columns[1] + columns[2]) // 2, (lines[2] + lines[3]) // 2)}


def calc_selection(snapshot):
    """Resolve a positive name-field value from the complete fixture toolbar.

    A bounded tree without the field is unknown, not evidence that a desired
    range is unselected. The following Standard toolbar proves the Formula
    Tool Bar section was not cut short by the snapshot's node budget.
    """
    elements = rows(snapshot)
    lines = snapshot.get('tree_markdown', '').splitlines()
    headers = [index for index, line in enumerate(lines)
               if line.strip() == '- tool bar = "Formula Tool Bar"']
    if len(headers) != 1:
        raise GroundingUnavailable('need one complete Calc formula toolbar')
    start = headers[0]
    indent = len(lines[start]) - len(lines[start].lstrip())
    following = next((line for line in lines[start + 1:]
                      if line.strip() and len(line) - len(line.lstrip()) <= indent), None)
    if (following is None or following.strip() != '- tool bar = "Standard"'
            or len(following) - len(following.lstrip()) != indent):
        raise GroundingUnavailable('Calc formula toolbar may be truncated')
    values = [row['label'] for row in elements
              if row.get('role') == 'text' and isinstance(row.get('label'), str) and row['label']
              and calc_formula_selection(snapshot, elements, row['label'])]
    if len(values) != 1:
        raise GroundingUnavailable('need one positively identified Calc selection')
    return values[0]


def inkscape_geometry(snapshot, *, allow_transform_center=False):
    elements = rows(snapshot)
    lines = [line.strip() for line in snapshot.get('tree_markdown', '').splitlines()]
    objects = [row for row in elements if row.get('role') == 'table cell' and row.get('label') == 'smoke-rectangle']
    selected = '- label = "Rectangle  in root. Click selection again to toggle scale/rotation handles."'
    center = ('- label = "Center of transformation: drag to reposition; scaling, rotation '
              'and skew with Shift also uses this center"')
    # Inkscape replaces its idle selection message while the synthetic pointer
    # hovers over the rotation center. This does not deselect the rectangle.
    # Admit this exact idle hint for fresh grounding/scroll only; a drag's
    # committed-effect check still requires the ordinary selection message.
    hints = lines.count(selected) + lines.count(center)
    if (len(objects) != 1 or hints != 1 or
            (lines.count(center) and not allow_transform_center)):
        raise GroundingUnavailable('the synthetic rectangle is not uniquely selected')
    values = {}
    for axis in ('X', 'Y', 'W', 'H'):
        matches = []
        for index, line in enumerate(lines):
            if index == 0 or lines[index - 1] != f'- label = "{axis}:"':
                continue
            match = re.match(r'^- \[(\d+)\] spin button "([-\d.]+)" value="([-\d.]+)" ', line)
            if not match:
                continue
            controls = [row for row in elements if row.get('element_index') == int(match[1])
                        and row.get('role') == 'spin button' and row.get('label') == match[2]
                        and row.get('value') == match[3] and row.get('enabled') is True
                        and row.get('frame', {}).get('h', 0) > 0
                        and 0 <= row.get('frame', {}).get('y', -1) - snapshot['window_bounds']['y'] < 90]
            if len(controls) == 1 and math.isclose(float(match[2]), float(match[3]), abs_tol=.001):
                matches.append(float(match[3]))
        if len(matches) != 1 or not math.isfinite(matches[0]):
            raise GroundingUnavailable('selection geometry differs between semantic projections')
        values[axis] = matches[0]
    if values['W'] != 80 or values['H'] != 50:
        raise GroundingUnavailable('synthetic rectangle size changed')
    return values


def blue_rectangle(snapshot, image):
    checked_snapshot(snapshot, image, 'inkscape')
    # Only the synthetic canvas rectangle has this large solid color region.
    # Exclude toolbar and palette strips, then require one contiguous rectangle.
    points = {(x, y) for y in range(90, image.height - 90) for x in range(35, image.width - 10)
              if image.rgb(x, y) == (51, 102, 153)}
    if not points:
        raise GroundingUnavailable('synthetic blue rectangle is not visible')
    left, right = min(x for x, _ in points), max(x for x, _ in points)
    top, bottom = min(y for _, y in points), max(y for _, y in points)
    width, height = right - left + 1, bottom - top + 1
    if width < 40 or height < 25 or len(points) < .95 * width * height:
        raise GroundingUnavailable('blue pixels are clipped, obscured, or ambiguous')
    if not (left > 35 and top > 90 and right < image.width - 11 and bottom < image.height - 91):
        raise GroundingUnavailable('blue rectangle touches the canvas inspection boundary')
    return {'x': left, 'y': top, 'w': width, 'h': height,
            'center': [(left + right) // 2, (top + bottom) // 2]}


def inkscape_unselected(snapshot):
    """Prove the fixture's positive unselected status, not a keyboard command.

    Pixel selection does not use Edit > Select All. Native background snapshots
    can omit that menu while exposing the object and its exact unselected status.
    Both projections must identify the object; missing status is never treated
    as evidence that the object is unselected.
    """
    objects = [row for row in rows(snapshot) if row.get('role') == 'table cell'
               and row.get('label') == 'smoke-rectangle']
    if len(objects) != 1 or objects[0].get('enabled') is not True:
        return False
    lines = [line.strip() for line in snapshot.get('tree_markdown', '').splitlines()]
    status = ('- label = "No objects selected. Click, Shift+click, Alt+scroll mouse '
              'on top of objects, or drag around objects to select."')
    conflicts = ('- label = "Rectangle  in root. Click selection again to toggle scale/rotation handles."',
                 '- label = "Center of transformation: drag to reposition; scaling, rotation '
                 'and skew with Shift also uses this center"')
    prefix = f'- [{objects[0].get("element_index")}] table cell "smoke-rectangle" '
    return (lines.count(status) == 1 and sum(line.startswith(prefix) for line in lines) == 1
            and not any(line in conflicts for line in lines))


def action(snapshot, image, app, stage):
    checked_snapshot(snapshot, image, app)
    if stage not in STAGES[app]:
        raise GroundingUnavailable('unknown pointer qualification stage')
    oracle = {'app': app, 'stage': stage}
    if app == 'calc':
        if stage.startswith('scroll_'):
            x, y, w, h = calc_table(snapshot, image)
            point = [x + w // 2, y + h // 2]
            oracle['scroll'] = calc_scroll(snapshot, image)
        else:
            cells = calc_cells(snapshot, image)
            point = cells['B2' if stage == 'click_b2' else 'A1']
            oracle['selection'] = {'click_b2': 'B2', 'click_a1': 'A1', 'select_range': 'A1:B3'}[stage]
            if calc_selection(snapshot) == oracle['selection']:
                raise GroundingUnavailable('selection already matches; this would not prove a pointer effect')
    else:
        oracle['rectangle'] = blue_rectangle(snapshot, image)
        if stage == 'click_rectangle':
            if not inkscape_unselected(snapshot):
                raise GroundingUnavailable('click proof needs the unselected synthetic rectangle')
            oracle['geometry'] = None
        else:
            oracle['geometry'] = inkscape_geometry(snapshot, allow_transform_center=True)
        point = oracle['rectangle']['center']
        if stage == 'move_rectangle':
            # The center is an interactive pivot in rotation mode. Ground the
            # drag inside the same observed rectangle, away from every handle.
            rectangle = oracle['rectangle']
            point = [rectangle['x'] + rectangle['w'] // 4,
                     rectangle['y'] + rectangle['h'] // 4]
    if stage.startswith('scroll_'):
        args = {'x': point[0], 'y': point[1], 'direction': stage.split('_')[1], 'amount': 1, 'by': 'line'}
    elif STAGES[app][stage] == 'drag':
        end = cells['B3'] if app == 'calc' else [point[0] + 40, point[1] + 30]
        assert 0 < end[0] < image.width and 0 < end[1] < image.height
        args = {'from_x': point[0], 'from_y': point[1], 'to_x': end[0], 'to_y': end[1],
                'duration_ms': 1500, 'steps': 30}
        oracle['delta'] = [end[0] - point[0], end[1] - point[1]]
    else:
        args = {'x': point[0], 'y': point[1]}
    return args, oracle


def visible_inkscape_scroll_stage(snapshot, image):
    """Choose one recovery scroll before input, with room for its pixel oracle.

    The pinned fixture scrolls about 80 pixels per line. Reserve 100 pixels
    inside the existing inspection region, not a guarantee for arbitrary apps.
    After-action pixel and document-size checks remain unchanged.
    """
    rectangle = blue_rectangle(snapshot, image)
    inkscape_geometry(snapshot, allow_transform_center=True)
    margins = {'scroll_down': rectangle['y'] - 90,
               'scroll_up': image.height - 90 - (rectangle['y'] + rectangle['h'])}
    stage = max(margins, key=margins.get)
    if margins[stage] < 100:
        raise GroundingUnavailable('recovery scroll needs 100 pixels of visible canvas margin')
    return stage


def verify(snapshot, image, oracle):
    app, stage = oracle['app'], oracle['stage']
    elements = checked_snapshot(snapshot, image, app)
    if app == 'calc':
        if 'selection' in oracle:
            assert calc_selection(snapshot) == oracle['selection'], 'Calc selection did not change as expected'
        else:
            value = calc_scroll(snapshot, image)
            assert value > oracle['scroll'] if stage == 'scroll_down' else value < oracle['scroll'], 'Calc viewport did not scroll'
    else:
        rectangle = blue_rectangle(snapshot, image)
        geometry = inkscape_geometry(snapshot, allow_transform_center=stage.startswith('scroll_'))
        previous = oracle['rectangle']
        assert abs(rectangle['w'] - previous['w']) <= 1 and abs(rectangle['h'] - previous['h']) <= 1, 'rectangle resized'
        delta = [a - b for a, b in zip(rectangle['center'], previous['center'])]
        if stage == 'move_rectangle':
            # Inkscape 1.4.4 select-tool.cpp anchors _seltrans->grab(p) at the
            # first processed motion, NOT at button press. Do not equate object
            # translation with the complete pointer delta. Require a committed
            # selection and matching pixel/document translation here, then match
            # that translation to an actual motion anchor in verify_drag_trace.
            assert all(2 < a <= b + 2 for a, b in zip(delta, oracle['delta'])), 'rectangle did not follow drag direction'
            assert geometry['X'] > oracle['geometry']['X'] and geometry['Y'] > oracle['geometry']['Y']
            semantic_pixels = [(geometry[axis] - oracle['geometry'][axis]) * previous[size] / oracle['geometry'][extent]
                               for axis, size, extent in (('X', 'w', 'W'), ('Y', 'h', 'H'))]
            assert all(abs(a - b) <= 2 for a, b in zip(delta, semantic_pixels)), 'pixel/document translation disagrees'
        else:
            if oracle['geometry'] is not None:
                assert geometry == oracle['geometry'], 'pointer operation changed document geometry'
            if stage.startswith('scroll_'):
                assert abs(delta[0]) <= 1 and (delta[1] < -2 if stage == 'scroll_down' else delta[1] > 2), 'canvas did not scroll'
            else:
                assert all(abs(value) <= 1 for value in delta), 'click unexpectedly moved rectangle'
    result = {'verified': True, 'scope': 'fresh-snapshot-pointer-effect', **oracle}
    if app == 'inkscape' and stage == 'move_rectangle':
        result.update(observed_delta=delta, observed_geometry=geometry, pointer_delivery='requires_trace')
    return result


def verify_drag_trace(trace, arguments, effect, *, expected_lane=None):
    """Prove the wire endpoint independently of a client's drag threshold.

    Surface coordinates come from the Wayland protocol logger, not actuator
    intent or primary-cursor coordinates. Historical seven-field traces cannot
    satisfy this oracle. This bounded episode must have one unambiguous stroke.
    An expected lane binds cancellation proofs to the known surviving agent.
    """
    from primary_trace import analyze
    assert expected_lane is None or (type(expected_lane) is int and expected_lane in (1, 2)), \
        'invalid expected drag lane'
    assert analyze(trace).get('telemetry_complete') is True, 'incomplete pointer trace'
    start = [arguments['from_x'], arguments['from_y']]
    end = [arguments['to_x'], arguments['to_y']]
    matched = []
    for lane in ((expected_lane,) if expected_lane is not None else (1, 2)):
        events = [row for row in trace['events'] if row[5] == lane]
        for index, row in enumerate(events):
            if row[2] != 'agent_drag_start':
                continue
            preceding = [r for r in events[:index] if r[2] == 'pointer_motion']
            assert preceding and len(preceding[-1]) == 9, 'missing synthetic start coordinates'
            if math.dist(preceding[-1][7:9], start) > .01:
                continue
            ends = [i for i in range(index + 1, len(events)) if events[i][2] in ('agent_drag_end', 'agent_cancel')]
            assert ends and events[ends[0]][2] == 'agent_drag_end', 'drag did not complete'
            stroke = events[index + 1:ends[0]]
            assert not any(r[2] in ('pointer_leave', 'agent_drag_start') for r in stroke), 'drag focus changed'
            buttons = [r for r in stroke if r[2] == 'pointer_button']
            assert len(buttons) == 2 and [r[6] for r in buttons] == [1, 0], 'unbalanced drag buttons'
            motion = [r for r in stroke if r[2] == 'pointer_motion']
            assert len(motion) >= 2 and all(len(r) == 9 for r in motion), 'missing synthetic motion coordinates'
            assert buttons[0][0] < motion[0][0] < motion[-1][0] < buttons[1][0], 'motion outside button hold'
            assert math.dist(motion[-1][7:9], end) <= .01, 'synthetic pointer missed endpoint'
            vector = [b - a for a, b in zip(start, end)]
            norm = sum(v * v for v in vector)
            assert norm > 0
            last = 0.0
            for sample in motion:
                t = sum((p - a) * v for p, a, v in zip(sample[7:9], start, vector)) / norm
                assert last - .001 <= t <= 1.001, 'synthetic path reversed or overshot'
                assert math.dist(sample[7:9], [a + t * v for a, v in zip(start, vector)]) <= .01, 'synthetic path deviated'
                last = t
            if effect['app'] == 'inkscape':
                delta = effect['observed_delta']
                # Source: INKSCAPE_1_4_4/src/ui/tools/select-tool.cpp,
                # root_handler(MotionEvent), _seltrans->grab(p)/moveTo(p).
                # Coalescing may skip events. The translation must still match
                # endpoint minus a motion the client could actually receive.
                anchors = [r[0] for r in motion[:-1] if all(abs((e - p) - d) <= 2
                           for e, p, d in zip(end, r[7:9], delta))]
                assert anchors, 'object translation matches no delivered motion anchor'
            else:
                anchors = []
            matched.append({'lane': lane, 'start': start, 'end': end, 'motion_events': len(motion),
                            'compatible_anchor_events': anchors})
    assert len(matched) == 1, 'missing or ambiguous matching drag'
    return {'verified': True, 'scope': 'wire-pointer-endpoint-and-app-effect', **matched[0]}
