"""Fresh-pixel grounding for bounded native Calc/Inkscape pointer proofs.

No captured coordinates are reused across calls. GTK's PNG reader is needed
only on the native test guest; the geometry algorithms have portable tests.
These helpers inspect synthetic documents, not arbitrary application content.
"""
import math
import re

from production_app_smoke import (GroundingUnavailable, calc_formula_selection,
                                  inkscape_selection_command, rows)


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
    # The table frame excludes headers. Corroborate it with the actual uniform
    # cell grid; a one-cell blue selection may cover less than 30% of a line.
    columns = [px for px in range(x, x + w)
               if sum(image.rgb(px, py) == (204, 204, 204) for py in range(y, y + h)) >= h * .70]
    lines = [py for py in range(y, y + h)
             if sum(image.rgb(px, py) == (204, 204, 204) for px in range(x, x + w)) >= w * .70]
    if len(columns) < 3 or len(lines) < 4:
        raise GroundingUnavailable('cannot resolve visible spreadsheet grid')
    columns, lines = [x - 1, *columns], [y - 1, *lines]
    for positions, minimum in ((columns, 30), (lines, 8)):
        gaps = [b - a for a, b in zip(positions, positions[1:])]
        if min(gaps) < minimum or max(gaps) - min(gaps) > 2:
            raise GroundingUnavailable('spreadsheet grid is ambiguous or irregular')
    return {'A1': ((columns[0] + columns[1]) // 2, (lines[0] + lines[1]) // 2),
            'B2': ((columns[1] + columns[2]) // 2, (lines[1] + lines[2]) // 2),
            'B3': ((columns[1] + columns[2]) // 2, (lines[2] + lines[3]) // 2)}


def inkscape_geometry(snapshot):
    elements = rows(snapshot)
    lines = [line.strip() for line in snapshot.get('tree_markdown', '').splitlines()]
    objects = [row for row in elements if row.get('role') == 'table cell' and row.get('label') == 'smoke-rectangle']
    if len(objects) != 1 or lines.count('- label = "Rectangle  in root. Click selection again to toggle scale/rotation handles."') != 1:
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
            if calc_formula_selection(snapshot, rows(snapshot), oracle['selection']):
                raise GroundingUnavailable('selection already matches; this would not prove a pointer effect')
    else:
        oracle['rectangle'] = blue_rectangle(snapshot, image)
        if stage == 'click_rectangle':
            if not inkscape_selection_command(snapshot, rows(snapshot)):
                raise GroundingUnavailable('click proof needs the unselected synthetic rectangle')
            oracle['geometry'] = None
        else:
            oracle['geometry'] = inkscape_geometry(snapshot)
        point = oracle['rectangle']['center']
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


def verify(snapshot, image, oracle):
    app, stage = oracle['app'], oracle['stage']
    elements = checked_snapshot(snapshot, image, app)
    if app == 'calc':
        if 'selection' in oracle:
            assert calc_formula_selection(snapshot, elements, oracle['selection']), 'Calc selection did not change as expected'
        else:
            value = calc_scroll(snapshot, image)
            assert value > oracle['scroll'] if stage == 'scroll_down' else value < oracle['scroll'], 'Calc viewport did not scroll'
    else:
        rectangle, geometry = blue_rectangle(snapshot, image), inkscape_geometry(snapshot)
        previous = oracle['rectangle']
        assert abs(rectangle['w'] - previous['w']) <= 1 and abs(rectangle['h'] - previous['h']) <= 1, 'rectangle resized'
        delta = [a - b for a, b in zip(rectangle['center'], previous['center'])]
        if stage == 'move_rectangle':
            assert all(abs(a - b) <= 2 for a, b in zip(delta, oracle['delta'])), 'rectangle did not follow drag'
            assert geometry['X'] > oracle['geometry']['X'] and geometry['Y'] > oracle['geometry']['Y']
        else:
            if oracle['geometry'] is not None:
                assert geometry == oracle['geometry'], 'pointer operation changed document geometry'
            if stage.startswith('scroll_'):
                assert abs(delta[0]) <= 1 and (delta[1] < -2 if stage == 'scroll_down' else delta[1] > 2), 'canvas did not scroll'
            else:
                assert all(abs(value) <= 1 for value in delta), 'click unexpectedly moved rectangle'
    return {'verified': True, 'scope': 'fresh-snapshot-pointer-effect', **oracle}
