"""Synthetic pixels and semantic projections; no native operation is performed."""
import copy
import unittest

import production_pointer_grounding as pointer
from production_app_smoke_test import INKSCAPE, INKSCAPE_SELECTED
from primary_trace_test import START, STOP, trace


class Image:
    width = height = 500

    def __init__(self):
        self.points = {}

    def rgb(self, x, y):
        assert 0 <= x < self.width and 0 <= y < self.height
        return self.points.get((x, y), (255, 255, 255))

    def rectangle(self, x, y, w, h):
        self.points.update({(a, b): (51, 102, 153) for a in range(x, x + w) for b in range(y, y + h)})


def snapshot(app):
    return {'window_title': f'cua-smoke-{app}', 'window_bounds': {'x': 600, 'y': 400, 'width': 500, 'height': 500},
            'screenshot_width': 500, 'screenshot_height': 500}


def calc(selection='A2', scroll=0):
    state = snapshot('calc')
    state.update(elements=[
        {'role': 'panel', 'element_index': 1, 'enabled': True},
        {'role': 'text', 'element_index': 2, 'parent_index': 1, 'label': selection, 'enabled': True},
        {'role': 'combo box', 'element_index': 3, 'parent_index': 1, 'enabled': True},
        {'role': 'table', 'element_index': 4, 'label': 'Sheet Smoke',
         'frame': {'x': 645, 'y': 569, 'w': 340, 'h': 270}},
        {'role': 'scroll bar', 'value': str(scroll), 'frame': {'x': 990, 'y': 569, 'w': 14, 'h': 270}}],
        tree_markdown='\n'.join(['- tool bar = "Formula Tool Bar"',
                                 '  - [1] panel "" [actions=[]]',
                                 f'    - [2] text "{selection}" [actions=[]]',
                                 '- tool bar = "Standard"']))
    image = Image()
    for x in (129, 214, 299, 384):
        image.points.update({(x, y): (204, 204, 204) for y in range(169, 439)})
    for y in range(186, 439, 18):
        image.points.update({(x, y): (204, 204, 204) for x in range(45, 385)})
    return state, image


def highlighted_calc():
    state, image = calc('A1:B3')
    # Selection tint replaces both white interiors and gray grid pixels in
    # A1:B3. C and D retain independent evidence for the first two row lines.
    image.points.update({(x, y): (205, 226, 247)
                         for x in range(45, 215) for y in range(169, 222)})
    return state, image


def ink(selected=True, dx=0, dy=0, scroll_y=0):
    state = {**snapshot('inkscape'), **copy.deepcopy(INKSCAPE_SELECTED if selected else INKSCAPE)}
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


class PointerGroundingTests(unittest.TestCase):
    def test_recovery_scroll_chooses_visible_space_before_input(self):
        for offset, expected, movement in ((21, 'scroll_up', 80), (160, 'scroll_down', -80)):
            with self.subTest(offset=offset):
                before, image = ink(scroll_y=offset)
                stage = pointer.visible_inkscape_scroll_stage(before, image)
                self.assertEqual(stage, expected)
                args, oracle = pointer.action(before, image, 'inkscape', stage)
                self.assertEqual(args['amount'], 1)
                self.assertTrue(pointer.verify(*ink(scroll_y=offset + movement), oracle)['verified'])
        # Reproduce a top-toolbar clip while document W/H remain unchanged.
        _, oracle = pointer.action(*ink(scroll_y=21), 'inkscape', 'scroll_down')
        after, clipped = ink(scroll_y=-59)
        clipped.points = {point: color for point, color in clipped.points.items() if point[1] >= 94}
        with self.assertRaisesRegex(AssertionError, 'rectangle resized'):
            pointer.verify(after, clipped, oracle)

    def test_recovery_scroll_refuses_insufficient_margin_or_ambiguous_state(self):
        state, image = ink()
        image.height = 350
        state['screenshot_height'] = state['window_bounds']['height'] = 350
        with self.assertRaisesRegex(pointer.GroundingUnavailable, '100 pixels'):
            pointer.visible_inkscape_scroll_stage(state, image)
        with self.assertRaises(pointer.GroundingUnavailable):
            pointer.visible_inkscape_scroll_stage(*ink(False))
        state, image = ink()
        image.points.clear()
        with self.assertRaises(pointer.GroundingUnavailable):
            pointer.visible_inkscape_scroll_stage(state, image)

    def test_calc_selection_requires_positive_complete_semantic_evidence(self):
        for failure in ('missing_toolbar', 'truncated_toolbar', 'missing_field', 'wrong_parent',
                        'duplicate_field', 'duplicate_toolbar', 'disabled', 'wrong_terminator_depth'):
            with self.subTest(failure=failure):
                state, image = calc('B2')
                if failure == 'missing_toolbar':
                    state['tree_markdown'] = ''
                elif failure == 'truncated_toolbar':
                    state['tree_markdown'] = state['tree_markdown'].rsplit('\n', 1)[0]
                elif failure == 'missing_field':
                    state['elements'] = [row for row in state['elements'] if row.get('role') != 'text']
                elif failure == 'wrong_parent':
                    state['elements'][1]['parent_index'] = 999
                elif failure == 'duplicate_field':
                    state['elements'].append(copy.deepcopy(state['elements'][1]))
                elif failure == 'duplicate_toolbar':
                    state['tree_markdown'] += '\n- tool bar = "Formula Tool Bar"'
                elif failure == 'disabled':
                    state['elements'][1]['enabled'] = False
                else:
                    state['tree_markdown'] = state['tree_markdown'].replace(
                        '- tool bar = "Standard"', '  - tool bar = "Standard"')
                with self.assertRaises(pointer.GroundingUnavailable):
                    pointer.action(state, image, 'calc', 'select_range')
                with self.assertRaises(pointer.GroundingUnavailable):
                    pointer.verify(state, image, {'app': 'calc', 'stage': 'click_b2', 'selection': 'B2'})

    def test_calc_points_are_derived_from_the_current_grid(self):
        state, image = calc()
        self.assertEqual(pointer.calc_cells(state, image), {'A1': (86, 177), 'B2': (171, 195), 'B3': (171, 213)})
        args, oracle = pointer.action(state, image, 'calc', 'click_b2')
        self.assertEqual(args, {'x': 171, 'y': 195})
        self.assertTrue(pointer.verify(*calc('B2'), oracle)['verified'])
        with self.assertRaises(AssertionError):
            pointer.verify(*calc('A2'), oracle)
        args, oracle = pointer.action(*calc('B2'), 'calc', 'select_range')
        self.assertEqual(args, {'from_x': 86, 'from_y': 177, 'to_x': 171, 'to_y': 213,
                                'duration_ms': 1500, 'steps': 30})
        self.assertTrue(pointer.verify(*calc('A1:B3'), oracle)['verified'])

    def test_calc_highlighted_range_uses_unobscured_column_strips(self):
        state, image = highlighted_calc()
        # This is the native failure: neither early line meets full-width 70%.
        for y in (186, 204):
            self.assertLess(sum(image.rgb(x, y) == (204, 204, 204)
                                for x in range(45, 385)), 340 * .70)
        self.assertEqual(pointer.calc_cells(state, image), pointer.calc_cells(*calc()))
        args, oracle = pointer.action(state, image, 'calc', 'click_b2')
        self.assertEqual(args, {'x': 171, 'y': 195})
        self.assertTrue(pointer.verify(*calc('B2'), oracle)['verified'])
        with self.assertRaisesRegex(pointer.GroundingUnavailable, 'selection already matches'):
            pointer.action(state, image, 'calc', 'select_range')

    def test_calc_highlight_does_not_hide_irregular_or_ambiguous_grid_evidence(self):
        for failure in ('extra_row', 'conflicting_strips', 'missing_row', 'one_strip', 'irregular_column'):
            with self.subTest(failure=failure):
                state, image = highlighted_calc()
                if failure == 'extra_row':
                    image.points.update({(x, 195): (204, 204, 204) for x in range(215, 384)})
                elif failure == 'conflicting_strips':
                    # One clear strip disagrees with the other by one pixel.
                    image.points.update({(x, 186): (255, 255, 255) for x in range(215, 299)})
                    image.points.update({(x, 187): (204, 204, 204) for x in range(215, 299)})
                elif failure == 'missing_row':
                    image.points.update({(x, 186): (255, 255, 255) for x in range(215, 384)})
                elif failure == 'one_strip':
                    image.points.update({(x, y): (205, 226, 247)
                                         for x in range(215, 300) for y in range(169, 222)})
                else:
                    image.points.update({(140, y): (204, 204, 204) for y in range(222, 439)})
                with self.assertRaises(pointer.GroundingUnavailable):
                    pointer.action(state, image, 'calc', 'click_b2')

    def test_calc_grid_and_scroll_refuse_ambiguous_or_clipped_state(self):
        for failure in ('blank', 'duplicate', 'clipped', 'scrolled', 'irregular', 'already_selected', 'scaled'):
            with self.subTest(failure=failure):
                state, image = calc('B2' if failure == 'already_selected' else 'A2')
                if failure == 'blank':
                    image.points.clear()
                elif failure == 'duplicate':
                    state['elements'].append(copy.deepcopy(state['elements'][3]))
                elif failure == 'clipped':
                    state['elements'][3]['frame']['x'] = 900
                elif failure == 'scrolled':
                    state['elements'][4]['value'] = '5'
                elif failure == 'irregular':
                    image.points.update({(140, y): (204, 204, 204) for y in range(169, 439)})
                elif failure == 'scaled':
                    state['screenshot_width'] = 250
                with self.assertRaises((AssertionError, pointer.GroundingUnavailable)):
                    pointer.action(state, image, 'calc', 'click_b2')

    def test_calc_scroll_requires_an_observed_value_change(self):
        args, oracle = pointer.action(*calc(), 'calc', 'scroll_down')
        self.assertEqual(args['amount'], 1)
        self.assertTrue(pointer.verify(*calc(scroll=3), oracle)['verified'])
        with self.assertRaises(AssertionError):
            pointer.verify(*calc(), oracle)
        _, oracle = pointer.action(*calc(scroll=3), 'calc', 'scroll_up')
        self.assertTrue(pointer.verify(*calc(), oracle)['verified'])

    def test_inkscape_click_requires_an_actual_selection_transition(self):
        args, oracle = pointer.action(*ink(False), 'inkscape', 'click_rectangle')
        self.assertEqual(args, {'x': 169, 'y': 179})
        self.assertTrue(pointer.verify(*ink(), oracle)['verified'])
        with self.assertRaises(pointer.GroundingUnavailable):
            pointer.verify(*ink(False), oracle)
        with self.assertRaises(pointer.GroundingUnavailable):
            pointer.action(*ink(), 'inkscape', 'click_rectangle')

    def test_pixel_selection_does_not_require_keyboard_menu_discovery(self):
        state, image = ink(False)
        state['elements'] = [row for row in state['elements'] if row['role'] == 'table cell']
        state['tree_markdown'] = '\n'.join(line for line in state['tree_markdown'].splitlines()
                                           if 'menu ' not in line and 'menu item ' not in line)
        args, oracle = pointer.action(state, image, 'inkscape', 'click_rectangle')
        self.assertEqual(args, {'x': 169, 'y': 179})
        self.assertTrue(pointer.verify(*ink(), oracle)['verified'])
        # The separate Ctrl+A grounding contract still needs its actual menu.
        from production_app_smoke import inkscape_selection_command, rows
        self.assertFalse(inkscape_selection_command(state, rows(state)))

    def test_pixel_selection_requires_positive_consistent_unselected_evidence(self):
        for failure in ('missing_status', 'duplicate_status', 'missing_object',
                        'duplicate_object', 'disabled_object', 'missing_object_line',
                        'duplicate_object_line', 'wrong_object_index', 'selected_conflict', 'dialog'):
            with self.subTest(failure=failure):
                state, image = ink(False)
                object_row = next(row for row in state['elements'] if row['role'] == 'table cell')
                lines = state['tree_markdown'].splitlines()
                if failure == 'missing_status':
                    lines = [line for line in lines if 'No objects selected.' not in line]
                elif failure == 'duplicate_status':
                    lines.append(next(line for line in lines if 'No objects selected.' in line))
                elif failure == 'missing_object':
                    state['elements'].remove(object_row)
                elif failure == 'duplicate_object':
                    state['elements'].append(copy.deepcopy(object_row))
                elif failure == 'disabled_object':
                    object_row['enabled'] = False
                elif failure == 'missing_object_line':
                    lines = [line for line in lines if 'table cell' not in line]
                elif failure == 'duplicate_object_line':
                    lines.append(next(line for line in lines if 'table cell' in line))
                elif failure == 'wrong_object_index':
                    object_row['element_index'] = 999
                elif failure == 'selected_conflict':
                    lines.append('- label = "Rectangle  in root. Click selection again to toggle scale/rotation handles."')
                else:
                    state['elements'].append({'role': 'dialog'})
                state['tree_markdown'] = '\n'.join(lines)
                with self.assertRaises(pointer.GroundingUnavailable):
                    pointer.action(state, image, 'inkscape', 'click_rectangle')

    def test_inkscape_drag_and_scroll_need_pixels_and_semantics_to_agree(self):
        args, oracle = pointer.action(*ink(), 'inkscape', 'move_rectangle')
        self.assertEqual([args['from_x'], args['from_y']], [145, 165])
        self.assertEqual([args['to_x'] - args['from_x'], args['to_y'] - args['from_y']], [40, 30])
        self.assertTrue(pointer.verify(*ink(dx=40, dy=30), oracle)['verified'])
        # A client may anchor at a processed motion after button press.
        self.assertTrue(pointer.verify(*ink(dx=35, dy=27), oracle)['verified'])
        with self.assertRaises(AssertionError):
            pointer.verify(*ink(), oracle)
        state, image = ink(dx=35, dy=27)
        for row in state['elements']:
            if row.get('element_index') == 2:
                old = row['value']
                row.update(label='100.000', value='100.0')
                state['tree_markdown'] = state['tree_markdown'].replace(
                    f'[{2}] spin button "68.000" value="{old}"', '[2] spin button "100.000" value="100.0"')
        with self.assertRaisesRegex(AssertionError, 'pixel/document'):
            pointer.verify(state, image, oracle)
        _, oracle = pointer.action(*ink(), 'inkscape', 'scroll_down')
        self.assertTrue(pointer.verify(*ink(scroll_y=-20), oracle)['verified'])
        for state in (ink(), ink(scroll_y=20), ink(dx=10, dy=10)):
            with self.assertRaises(AssertionError):
                pointer.verify(*state, oracle)

    def test_inkscape_idle_pivot_hover_can_ground_a_new_action_without_replaying_drag(self):
        selected = 'Rectangle  in root. Click selection again to toggle scale/rotation handles.'
        center = ('Center of transformation: drag to reposition; scaling, rotation '
                  'and skew with Shift also uses this center')
        state, image = ink()
        state['tree_markdown'] = state['tree_markdown'].replace(selected, center)
        args, _ = pointer.action(state, image, 'inkscape', 'move_rectangle')
        self.assertEqual([args['from_x'], args['from_y']], [145, 165])
        with self.assertRaises(pointer.GroundingUnavailable):
            pointer.inkscape_geometry(state)
        _, oracle = pointer.action(state, image, 'inkscape', 'scroll_down')
        after, after_image = ink(scroll_y=-20)
        after['tree_markdown'] = after['tree_markdown'].replace(selected, center)
        self.assertTrue(pointer.verify(after, after_image, oracle)['verified'])
        with self.assertRaises(AssertionError):
            pointer.verify(state, image, oracle)
        # An interrupted drag is never reclassified as a committed move merely
        # because the later snapshot exposes an idle hover hint.
        _, drag = pointer.action(*ink(), 'inkscape', 'move_rectangle')
        after, after_image = ink(dx=35, dy=27)
        after['tree_markdown'] = after['tree_markdown'].replace(selected, center)
        with self.assertRaises(pointer.GroundingUnavailable):
            pointer.verify(after, after_image, drag)

    def test_inkscape_hover_does_not_admit_ambiguous_unselected_or_in_progress_state(self):
        selected = 'Rectangle  in root. Click selection again to toggle scale/rotation handles.'
        center = ('Center of transformation: drag to reposition; scaling, rotation '
                  'and skew with Shift also uses this center')
        for failure in ('duplicate_hint', 'duplicate_object', 'no_object', 'in_progress',
                        'unselected', 'wrong_geometry', 'disabled_geometry'):
            with self.subTest(failure=failure):
                state, image = ink()
                state['tree_markdown'] = state['tree_markdown'].replace(selected, center)
                if failure == 'duplicate_hint':
                    state['tree_markdown'] += f'\n- label = "{selected}"'
                elif failure == 'duplicate_object':
                    state['elements'].append(copy.deepcopy(state['elements'][0]))
                elif failure == 'no_object':
                    state['elements'].pop(0)
                elif failure == 'in_progress':
                    state['tree_markdown'] = state['tree_markdown'].replace(center, 'Move by 1 px, 1 px')
                elif failure == 'unselected':
                    state, image = ink(False)
                elif failure == 'wrong_geometry':
                    state['elements'][3]['value'] = '120.0'
                else:
                    state['elements'][3]['enabled'] = False
                with self.assertRaises(pointer.GroundingUnavailable):
                    pointer.action(state, image, 'inkscape', 'scroll_down')

    def test_wrong_or_ambiguous_blue_pixels_do_not_ground(self):
        for failure in ('wrong_title', 'dialog', 'blank', 'two_rectangles', 'clipped', 'semantic_mismatch'):
            with self.subTest(failure=failure):
                state, image = ink()
                if failure == 'wrong_title':
                    state['window_title'] = 'another.svg'
                elif failure == 'dialog':
                    state['elements'].append({'role': 'dialog'})
                elif failure == 'blank':
                    image.points.clear()
                elif failure == 'two_rectangles':
                    image.rectangle(300, 250, 80, 50)
                elif failure == 'clipped':
                    image.points.clear()
                    image.rectangle(120, 80, 100, 60)
                else:
                    state['elements'][1]['value'] = '999.0'
                with self.assertRaises((AssertionError, pointer.GroundingUnavailable)):
                    pointer.action(state, image, 'inkscape', 'move_rectangle')

    def test_rgb_accessor_honors_stride_and_alpha(self):
        image = pointer.Pixels(2, 2, bytes([10, 20, 30, 255, 40, 50, 60, 255, 0, 0,
                                          70, 80, 90, 255, 100, 110, 120, 255]), 10, 4)
        self.assertEqual(image.rgb(1, 1), (100, 110, 120))
        with self.assertRaises(AssertionError):
            image.rgb(2, 1)

    def test_drag_trace_proves_endpoint_separately_from_client_anchor(self):
        args, oracle = pointer.action(*ink(), 'inkscape', 'move_rectangle')
        effect = pointer.verify(*ink(dx=35, dy=27), oracle)
        data = trace(START, ('pointer_motion', 100, 200, 2, 0),
                     ('agent_drag_start', 100, 200, 2, 0), ('pointer_button', 100, 200, 2, 1),
                     ('pointer_motion', 100, 200, 2, 0), ('pointer_motion', 100, 200, 2, 0),
                     ('pointer_motion', 100, 200, 2, 0), ('pointer_button', 100, 200, 2, 0),
                     ('agent_drag_end', 100, 200, 2, 0), ('pointer_leave', 100, 200, 2, 0), STOP)
        for index, xy in ((1, [145, 165]), (4, [149, 168]), (5, [165, 180]), (6, [185, 195])):
            data['events'][index].extend(xy)
        result = pointer.verify_drag_trace(data, args, effect)
        self.assertEqual(result['lane'], 2)
        self.assertEqual(result['compatible_anchor_events'], [5])
        for expected_lane in (True, False, 0, 3, 1.0, '2', [], {}):
            with self.subTest(expected_lane=expected_lane), self.assertRaisesRegex(AssertionError, 'invalid expected'):
                pointer.verify_drag_trace(data, args, effect, expected_lane=expected_lane)
        with self.assertRaisesRegex(AssertionError, 'missing or ambiguous'):
            pointer.verify_drag_trace(data, args, effect, expected_lane=1)
        for canceled in (False, True):
            other = copy.deepcopy(data['events'][1:-1])
            for row in other:
                row[5] = 1
                if canceled and row[2] == 'agent_drag_end':
                    row[2] = 'agent_cancel'
            both = copy.deepcopy(data)
            both['events'][1:1] = other
            for sequence, row in enumerate(both['events'], 1):
                row[0], row[1] = sequence, sequence * 1_000_000
            both['count'] = len(both['events'])
            with self.subTest(canceled=canceled):
                self.assertEqual(pointer.verify_drag_trace(both, args, effect, expected_lane=2)['lane'], 2)
                with self.assertRaisesRegex(AssertionError, 'drag did not complete' if canceled else 'missing or ambiguous'):
                    pointer.verify_drag_trace(both, args, effect)
                if canceled:
                    with self.assertRaisesRegex(AssertionError, 'drag did not complete'):
                        pointer.verify_drag_trace(both, args, effect, expected_lane=1)
        for failure in ('missing_endpoint', 'wrong_start', 'missing_coords', 'reversed', 'off_path',
                        'cancelled', 'leave_during_drag', 'unreleased', 'no_anchor', 'incomplete'):
            with self.subTest(failure=failure):
                bad, changed_effect = copy.deepcopy(data), copy.deepcopy(effect)
                if failure == 'missing_endpoint':
                    bad['events'][6][7:9] = [184, 194.25]
                elif failure == 'wrong_start':
                    bad['events'][1][7] = 999
                elif failure == 'missing_coords':
                    bad['events'][4] = bad['events'][4][:7]
                elif failure == 'reversed':
                    bad['events'][5][7:9] = [147, 166.5]
                elif failure == 'off_path':
                    bad['events'][5][8] += 2
                elif failure == 'cancelled':
                    bad['events'][8][2] = 'agent_cancel'
                elif failure == 'leave_during_drag':
                    bad['events'][5] = bad['events'][5][:7]
                    bad['events'][5][2] = 'pointer_leave'
                elif failure == 'unreleased':
                    bad['events'][7][6] = 1
                elif failure == 'no_anchor':
                    changed_effect['observed_delta'] = [10, 25]
                else:
                    bad['overflow'] = True
                with self.assertRaises(AssertionError):
                    pointer.verify_drag_trace(bad, args, changed_effect)


if __name__ == '__main__':
    unittest.main()
