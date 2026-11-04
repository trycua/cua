"""No native applications or input: fixtures and fail-closed orchestration only."""
import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

import proofs_path  # noqa: F401  Puts ../proofs on sys.path.
from production_app_smoke import (
    GroundingUnavailable, OBSERVATION_TIMEOUT_MS, OPEN_OBJECTS_DESCRIPTION, OFFSCREEN_DESCRIPTION, check_delivery,
    OBSERVATION_ATTEMPTS, create_documents, grounding_outcome, ground, inkscape_app_id_tag, input_step, kernel_file_identity,
    launch_arguments, mapped_plugin, observe, package_owner, prepare_inkscape_objects,
    require_background_target, require_enabled_plugin, run_app, verify_calc, verify_inkscape,
)
from proof_fixtures import CALC as CALC_STATE, INKSCAPE_SELECTED, changed_ods


TARGET = {'pid': 123, 'window_id': 456}
GOOD_DELIVERY = {'structuredContent': {'route': 'synthetic_events',
                                      'effect': 'unverifiable',
                                      'delivery': {'mode': 'background'}}}
WINDOWS = {'structuredContent': {'windows': [TARGET]}}


def complete(state):
    """Mark a fixture as a full walk under the explicit observation budget."""
    return {**state, 'truncated': False, 'elements_complete': True,
            'timeout_ms': OBSERVATION_TIMEOUT_MS}


def observed(state):
    return [{'structuredContent': state}, WINDOWS]


CALC = complete(CALC_STATE)
# The pinned closed Edit menu omits its Select All child.
INKSCAPE = complete({
    'window_title': 'cua-smoke-inkscape.svg - Inkscape',
    'elements': [
        {'element_index': 10, 'role': 'menu', 'label': 'Edit', 'enabled': True},
        {'element_index': 12, 'role': 'table cell', 'label': 'smoke-rectangle', 'enabled': True,
         'selected': False}],
    'tree_markdown': '\n'.join([
        '  - [10] menu "Edit" [actions=[click]]',
        '  - [12] table cell "smoke-rectangle" [actions=[activate]]',
        '  - label = "No objects selected. Click, Shift+click, Alt+scroll mouse on top of '
        'objects, or drag around objects to select."'])})
# Objects panel not yet open: only the semantic opener exists.
INKSCAPE_NO_OBJECTS = complete({
    'window_title': 'cua-smoke-inkscape.svg - Inkscape',
    'elements': [
        {'element_index': 10, 'role': 'menu', 'label': 'Edit', 'enabled': True},
        {'element_index': 20, 'element_token': 'token-20', 'role': 'button',
         'label': OPEN_OBJECTS_DESCRIPTION, 'description': OPEN_OBJECTS_DESCRIPTION,
         'enabled': True, 'actions': ['click']}],
    'tree_markdown': '\n'.join([
        '  - [10] menu "Edit" [actions=[click]]',
        '  - [20] button "Open Objects" [actions=[click]]'])})
SETUP_CLICK = {'structuredContent': {'route': 'accessibility', 'effect': 'unverifiable',
                                   'delivery': {'mode': 'background'}}}


class FixtureTests(unittest.TestCase):
    def test_enabled_option_uses_exact_pinned_v2_boolean_contract(self):
        enabled = {'option': 'plugin:cua:enabled', 'bool': True, 'set': True}
        require_enabled_plugin(enabled)
        for value in ({}, [], {'option': 'plugin:cua:enabled', 'int': 1},
                      {**enabled, 'option': 'some:other:option'}, {**enabled, 'int': 1},
                      *({**enabled, 'bool': value} for value in (False, 1, 'true', None))):
            with self.subTest(value=value), self.assertRaisesRegex(AssertionError, 'not enabled'):
                require_enabled_plugin(value)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name).resolve()
        self.documents = create_documents(self.directory)

    def test_native_fixtures_and_exclusive_creation(self):
        with zipfile.ZipFile(self.documents['calc']) as archive:
            self.assertEqual(archive.infolist()[0].filename, 'mimetype')
            self.assertEqual(archive.infolist()[0].compress_type, zipfile.ZIP_STORED)
            self.assertEqual(archive.read('mimetype'), b'application/vnd.oasis.opendocument.spreadsheet')
        with self.assertRaises(FileExistsError):
            create_documents(self.directory)

    def test_launched_foreground_app_is_retained_without_input(self):
        mcp = Mock()
        mcp.tool.return_value = {'structuredContent': {}}
        original = self.documents['calc'].read_bytes()
        with patch('production_app_smoke.Path.iterdir', return_value=iter([])), \
                patch('production_app_smoke.discover', return_value=(TARGET, {})), \
                patch('production_app_smoke.read', return_value='{"pid": 123}'):
            with self.assertRaisesRegex(GroundingUnavailable, 'separate foreground fixture'):
                run_app(mcp, 'calc', self.documents['calc'], self.directory)
        self.assertEqual([call.args[0] for call in mcp.tool.call_args_list], ['launch_app'])
        self.assertEqual((self.directory / 'after.ods').read_bytes(), original)

    def test_calc_requires_exact_saved_a1(self):
        original = self.documents['calc'].read_bytes()
        self.assertTrue(verify_calc(original, changed_ods(original))['verified'])
        for value in (original, changed_ods(original, 'b'), changed_ods(original, empty_first=True)):
            with self.assertRaises(AssertionError):
                verify_calc(original, value)

    def test_inkscape_requires_exact_translation_and_size(self):
        original = self.documents['inkscape'].read_bytes()
        self.assertTrue(verify_inkscape(original, original.replace(b'x="40"', b'x="42"'))['verified'])
        self.assertTrue(verify_inkscape(original, original.replace(
            b'id="smoke-rectangle"', b'id="smoke-rectangle" transform="translate(2,0)"'))['verified'])
        for changed in (original, original.replace(b'x="40"', b'x="41"'),
                        original.replace(b'x="40"', b'x="42"').replace(b'width="80"', b'width="81"'),
                        original.replace(b'x="40"', b'x="nan"'),
                        original.replace(b'id="smoke-rectangle"',
                                         b'id="smoke-rectangle" transform="scale(2)"')):
            with self.assertRaises(AssertionError):
                verify_inkscape(original, changed)

    def test_loaded_plugin_requires_current_device_inode_and_path(self):
        plugin = self.directory / 'plugin.so'
        plugin.write_bytes(b'synthetic test fixture')
        stat = plugin.stat()
        device = f'{os.major(stat.st_dev):x}:{os.minor(stat.st_dev):x}'
        row = f'1000-2000 r-xp 00000000 {device} {stat.st_ino} {plugin}'
        expected = (os.major(stat.st_dev), os.minor(stat.st_dev), stat.st_ino)
        with patch('production_app_smoke.kernel_file_identity', return_value=expected):
            self.assertEqual(mapped_plugin(row, plugin), [row])
            for invalid in ('', row + ' (deleted)', row.replace(str(stat.st_ino), '0'),
                            row.replace(str(plugin), '/elsewhere/plugin.so'),
                            row.replace(device, f'{expected[0]:x}:{expected[1] + 1:x}')):
                with self.assertRaises(AssertionError):
                    mapped_plugin(invalid, plugin)

    def test_kernel_identity_handles_btrfs_device_presentation_without_ignoring_device(self):
        plugin = self.directory / 'plugin.so'
        plugin.write_bytes(b'synthetic test fixture')
        inode = plugin.stat().st_ino
        # The kernel reports a device different from stat(), as on Btrfs.
        device = os.minor(plugin.stat().st_dev) + 1
        row = f'1000-2000 rw-p 00000000 0:{device:x} {inode} {plugin}'
        fake_mapping = Mock()
        fake_mapping.__enter__ = Mock(return_value=bytearray(b'x'))
        fake_mapping.__exit__ = Mock(return_value=False)
        with patch('production_app_smoke.mmap.mmap', return_value=fake_mapping), \
                patch('production_app_smoke.ctypes.addressof', return_value=0x1000), \
                patch('production_app_smoke.Path.read_text', return_value=row):
            self.assertEqual(kernel_file_identity(plugin), (0, device, inode))
            self.assertEqual(mapped_plugin(row, plugin), [row])
            with self.assertRaisesRegex(AssertionError, 'mapped plugin identity differs'):
                mapped_plugin(row.replace(f'0:{device:x}', f'0:{device + 1:x}'), plugin)

    def test_reference_mapping_rejects_wrong_range_path_offset_inode_and_replacement(self):
        plugin = self.directory / 'plugin.so'
        plugin.write_bytes(b'synthetic test fixture')
        inode = plugin.stat().st_ino
        row = f'1000-2000 rw-p 00000000 0:20 {inode} {plugin}'
        fake_mapping = Mock()
        fake_mapping.__enter__ = Mock(return_value=bytearray(b'x'))
        fake_mapping.__exit__ = Mock(return_value=False)
        with patch('production_app_smoke.mmap.mmap', return_value=fake_mapping), \
                patch('production_app_smoke.ctypes.addressof', return_value=0x1000):
            for invalid in ('', row + '\n' + row, row + ' (deleted)',
                            row.replace('1000-2000', '2000-3000'),
                            row.replace('00000000', '00001000'),
                            row.replace(str(inode), '0'),
                            row.replace(str(plugin), '/elsewhere/plugin.so')):
                with self.subTest(maps=invalid), patch('production_app_smoke.Path.read_text', return_value=invalid):
                    with self.assertRaises(AssertionError):
                        kernel_file_identity(plugin)
            replacement = self.directory / 'replacement.so'
            replacement.write_bytes(b'different current file')
            def replace_path():
                replacement.replace(plugin)
                return row
            with patch('production_app_smoke.Path.read_text', side_effect=replace_path):
                with self.assertRaisesRegex(AssertionError, 'candidate.*changed'):
                    kernel_file_identity(plugin)

    @unittest.skipUnless(Path('/proc/self/maps').is_file(), 'requires native Linux proc maps')
    def test_reference_identity_from_real_kernel_mapping(self):
        plugin = self.directory / 'plugin.so'
        plugin.write_bytes(b'synthetic test fixture')
        identity = kernel_file_identity(plugin)
        self.assertEqual(identity[2], plugin.stat().st_ino)
        self.assertTrue(all(type(value) is int and value >= 0 for value in identity))

    def test_package_executable_rejects_symlink_and_nonexecutable(self):
        executable = self.directory / 'app'
        executable.write_bytes(b'synthetic executable')
        with self.assertRaisesRegex(AssertionError, 'not executable'):
            package_owner(executable, 'inkscape')
        executable.chmod(0o700)
        alias = self.directory / 'alias'
        alias.symlink_to(executable)
        with self.assertRaisesRegex(AssertionError, 'noncanonical package executable'):
            package_owner(alias, 'inkscape')


class InputTests(unittest.TestCase):
    def test_foreground_target_or_missing_foreground_is_inspection_only(self):
        for active in ('{}', 'null', '[]', '{"pid": 123}', '{"pid": 0}',
                       '{"pid": true}', '{"pid": "456"}'):
            with self.subTest(active=active), \
                    patch('production_app_smoke.read', return_value=active), \
                    patch('production_app_smoke.save_json') as save:
                with self.assertRaisesRegex(GroundingUnavailable, 'separate foreground fixture'):
                    require_background_target(TARGET, Path('/evidence'))
                save.assert_called_once()
        with patch('production_app_smoke.read', return_value='{"pid": 456}') as read, \
                patch('production_app_smoke.save_json'):
            require_background_target(TARGET, Path('/evidence'))
            read.assert_called_once_with(['hyprctl', '-j', 'activewindow'])

    def test_inkscape_launch_uses_run_unique_app_id_tag_and_positional_document(self):
        launch = launch_arguments('inkscape', Path('/docs/smoke.svg'), Path('/evidence/a/inkscape'))
        tag = inkscape_app_id_tag(Path('/evidence/a/inkscape'))
        self.assertRegex(tag, r'^cua-smoke-[0-9a-f]{16}$')
        self.assertEqual(launch, {'launch_path': '/usr/bin/inkscape',
                                  'additional_arguments': [f'--app-id-tag={tag}', '/docs/smoke.svg']})
        self.assertNotEqual(tag, inkscape_app_id_tag(Path('/evidence/b/inkscape')))
        self.assertTrue(all('app-id-tag' not in word for word in
                            launch_arguments('calc', Path('/docs/a.ods'), Path('/evidence/calc'))
                            ['additional_arguments']))

    def test_calc_formula_name_field_requires_matching_toolbar_and_rows(self):
        state = {'elements': [
            {'element_index': 10, 'role': 'panel', 'enabled': True},
            {'element_index': 11, 'parent_index': 10, 'role': 'text', 'label': 'A1', 'enabled': True},
            {'element_index': 12, 'parent_index': 10, 'role': 'combo box', 'enabled': True},
            {'element_index': 13, 'role': 'table', 'label': 'Sheet Smoke'}],
            'tree_markdown': '\n'.join([
                '  - tool bar = "Formula Tool Bar"',
                '    - [10] panel "" value="0.0" [actions=[press]]',
                '      - [11] text "A1" [actions=[activate]]',
                '      - [12] combo box "" [actions=[press]]',
                '  - table = "Sheet Smoke"'])}
        ground(state, 'calc', 'insert')
        for old, new in [('Formula Tool Bar', 'Other toolbar'), ('[11]', '[99]'),
                         ('[10]', '[99]'), ('text "A1"', 'text "B1"')]:
            with self.subTest(old=old), self.assertRaises(GroundingUnavailable):
                ground({**state, 'tree_markdown': state['tree_markdown'].replace(old, new)}, 'calc', 'insert')
        for index, replacement in [(0, {'role': 'menu'}), (1, {'parent_index': 9}),
                                   (1, {'enabled': False}), (1, {'label': 'B1'}),
                                   (2, {'parent_index': 9}), (3, {'label': 'Sheet Other'})]:
            bad = copy.deepcopy(state)
            bad['elements'][index].update(replacement)
            with self.subTest(index=index, replacement=replacement), self.assertRaises(GroundingUnavailable):
                ground(bad, 'calc', 'insert')
        for markdown in ['', state['tree_markdown'] + '\n' + state['tree_markdown'],
                         state['tree_markdown'].replace('    - [10]', '  - panel = "Other"\n    - [10]')]:
            with self.subTest(markdown=markdown), self.assertRaises(GroundingUnavailable):
                ground({**state, 'tree_markdown': markdown}, 'calc', 'insert')
        with self.assertRaises(GroundingUnavailable):
            ground({**state, 'elements': state['elements'] + [state['elements'][1]]}, 'calc', 'insert')

    def test_inkscape_requires_document_command_and_exact_initial_status(self):
        ground(INKSCAPE, 'inkscape', 'select')
        # A closed menu has no Select All child; an open one with it still grounds.
        ground({**INKSCAPE, 'elements': INKSCAPE['elements'] + [
            {'element_index': 11, 'parent_index': 10, 'role': 'menu item',
             'label': 'Select All', 'enabled': True}]}, 'inkscape', 'select')
        for index, replacement in [(0, {'role': 'label'}), (0, {'enabled': False}),
                                   (0, {'label': 'File'}),
                                   (1, {'label': 'other-rectangle'}), (1, {'role': 'label'}),
                                   (1, {'enabled': False}), (1, {'selected': True})]:
            bad = copy.deepcopy(INKSCAPE)
            bad['elements'][index].update(replacement)
            with self.subTest(index=index, replacement=replacement), self.assertRaises(GroundingUnavailable):
                ground(bad, 'inkscape', 'select')
        for old, new in [('[10]', '[99]'), ('[12]', '[99]'),
                         ('No objects selected.', '1 object selected.'), ('- label =', '- button =')]:
            with self.subTest(old=old), self.assertRaises(GroundingUnavailable):
                ground({**INKSCAPE, 'tree_markdown': INKSCAPE['tree_markdown'].replace(old, new)},
                       'inkscape', 'select')
        for markdown in ('', INKSCAPE['tree_markdown'] + '\n' + INKSCAPE['tree_markdown']):
            with self.assertRaises(GroundingUnavailable):
                ground({**INKSCAPE, 'tree_markdown': markdown}, 'inkscape', 'select')
        for row in INKSCAPE['elements']:
            with self.assertRaises(GroundingUnavailable):
                ground({**INKSCAPE, 'elements': INKSCAPE['elements'] + [row]}, 'inkscape', 'select')
        for role in ('canvas', 'drawing area'):
            with self.assertRaises(GroundingUnavailable):
                ground({'elements': [{'role': role}]}, 'inkscape', 'select')

    def test_only_acknowledged_synthetic_background_delivery_accepted(self):
        check_delivery(GOOD_DELIVERY)
        for replacement in ({'route': 'atspi'}, {'effect': 'partial'},
                            {'delivery': {'mode': 'unknown'}}, {'delivery': {'mode': 'foreground'}}):
            with self.assertRaises(AssertionError):
                check_delivery({'structuredContent': {**GOOD_DELIVERY['structuredContent'], **replacement}})
        with self.assertRaises(AssertionError):
            check_delivery({**GOOD_DELIVERY, 'isError': True})

    def test_selected_rectangle_requires_exact_status_object_and_geometry(self):
        ground(INKSCAPE_SELECTED, 'inkscape', 'move')
        for index, replacement in [(0, {'label': 'other-rectangle'}), (0, {'enabled': False}),
                                   (1, {'value': '42.0'}), (2, {'role': 'text'}),
                                   (3, {'enabled': False}), (4, {'element_index': 99})]:
            bad = copy.deepcopy(INKSCAPE_SELECTED)
            bad['elements'][index].update(replacement)
            with self.subTest(index=index, replacement=replacement), self.assertRaises(GroundingUnavailable):
                ground(bad, 'inkscape', 'move')
        for old, new in [('Rectangle  in root.', '2 objects selected.'),
                         ('Rectangle  in root.', 'Rectangle in layer.'),
                         ('- label = "Rectangle', '- text = "Rectangle'),
                         ('[1]', '[99]'), ('"X:"', '"Y:"'), ('value="80.0"', 'value="81.0"')]:
            bad = {**INKSCAPE_SELECTED,
                   'tree_markdown': INKSCAPE_SELECTED['tree_markdown'].replace(old, new)}
            with self.subTest(old=old, new=new), self.assertRaises(GroundingUnavailable):
                ground(bad, 'inkscape', 'move')
        for extra in ('\n' + INKSCAPE_SELECTED['tree_markdown'], '\n- label = "No objects selected."'):
            with self.assertRaises(GroundingUnavailable):
                ground({**INKSCAPE_SELECTED, 'tree_markdown': INKSCAPE_SELECTED['tree_markdown'] + extra},
                       'inkscape', 'move')
        for row in INKSCAPE_SELECTED['elements']:
            with self.assertRaises(GroundingUnavailable):
                ground({**INKSCAPE_SELECTED, 'elements': INKSCAPE_SELECTED['elements'] + [row]},
                       'inkscape', 'move')
        for state in ({**INKSCAPE_SELECTED, 'tree_markdown': ''},
                      {'elements': [{'role': 'status bar', 'value': '1 object selected'}]}, INKSCAPE):
            with self.assertRaises(GroundingUnavailable):
                ground(state, 'inkscape', 'move')

    def test_selected_rectangle_accepts_only_per_axis_semantic_labels(self):
        names = ['Horizontal coordinate of selection', 'Vertical coordinate of selection',
                 'Width of selection', 'Height of selection']

        def relabel(state, labels):
            state = copy.deepcopy(state)
            markdown = state['tree_markdown']
            for row, label in zip(state['elements'][1:], labels):
                markdown = markdown.replace(f'spin button "{row["label"]}"', f'spin button "{label}"')
                row['label'] = label
            return {**state, 'tree_markdown': markdown}

        ground(relabel(INKSCAPE_SELECTED, names), 'inkscape', 'move')
        # Swapped axes, a duplicate label, or a wrong value must not ground.
        for labels in (names[1::-1] + names[2:], [names[0]] * 2 + names[2:],
                       names[:3] + ['Depth of selection']):
            with self.subTest(labels=labels), self.assertRaises(GroundingUnavailable):
                ground(relabel(INKSCAPE_SELECTED, labels), 'inkscape', 'move')
        good = relabel(INKSCAPE_SELECTED, names)
        bad = copy.deepcopy(good)
        bad['elements'][3]['value'] = '81.0'
        bad['tree_markdown'] = bad['tree_markdown'].replace('value="80.0"', 'value="81.0"')
        with self.assertRaises(GroundingUnavailable):
            ground(bad, 'inkscape', 'move')
        with self.assertRaises(GroundingUnavailable):
            ground({**good, 'elements': good['elements'] + [good['elements'][1]]}, 'inkscape', 'move')
        with self.assertRaises(GroundingUnavailable):
            ground({**good, 'tree_markdown': good['tree_markdown'].replace('"Y:"', '"X:"')},
                   'inkscape', 'move')

    def test_calc_grounding_rejects_empty_tree_dialog_and_wrong_selection(self):
        # Inkscape stages are owned by the two Inkscape grounding tables above.
        ground(CALC, 'calc', 'insert')
        for state in ({'elements': []},
                      {'elements': [{'role': 'dialog', 'label': 'Recover documents'}]},
                      {'elements': [{'role': 'text', 'label': 'Name Box', 'value': 'B1'}]}):
            with self.subTest(state=state), self.assertRaises(GroundingUnavailable):
                ground(state, 'calc', 'insert')

    def test_snapshot_action_snapshot_and_no_replay(self):
        mcp = Mock()
        mcp.tool.side_effect = [{'structuredContent': CALC}, WINDOWS, GOOD_DELIVERY,
                                {'structuredContent': CALC}, WINDOWS]
        input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'type_text', {'text': 'abc'})
        calls = mcp.tool.call_args_list
        self.assertEqual([call.args[0] for call in calls],
                         ['get_window_state', 'list_windows', 'type_text', 'get_window_state', 'list_windows'])
        self.assertEqual(calls[2].args[1], {**TARGET, 'text': 'abc', 'delivery_mode': 'background'})
        for call in (calls[0], calls[3]):
            self.assertEqual(call.args[1], {**TARGET, 'timeout_ms': OBSERVATION_TIMEOUT_MS,
                                            'full_output': True})
        self.assertEqual(OBSERVATION_TIMEOUT_MS, 15000)

    def test_truncated_or_unproven_tree_sends_no_input(self):
        for change in ({'truncated': True}, {'elements_complete': False}, {'degraded': True},
                       {'timeout_ms': 1000}, {'truncated': None}, {'timeout_ms': None}):
            mcp = Mock()
            mcp.tool.side_effect = observed({**CALC, **change})
            with self.subTest(change=change), self.assertRaisesRegex(GroundingUnavailable, 'not proven complete'):
                input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'type_text', {'text': 'abc'})
            self.assertEqual([call.args[0] for call in mcp.tool.call_args_list], ['get_window_state'])
        for missing in ('truncated', 'elements_complete', 'timeout_ms'):
            state = {key: value for key, value in CALC.items() if key != missing}
            mcp = Mock()
            mcp.tool.side_effect = observed(state)
            with self.subTest(missing=missing), self.assertRaises(GroundingUnavailable):
                input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'type_text', {'text': 'abc'})
            self.assertEqual(mcp.tool.call_count, 1)

    def test_truncated_tree_after_input_still_fails_delivery_check_first(self):
        mcp = Mock()
        partial = {'structuredContent': {**GOOD_DELIVERY['structuredContent'], 'effect': 'partial'}}
        mcp.tool.side_effect = [*observed(CALC), partial, *observed({**CALC, 'truncated': True})]
        with self.assertRaisesRegex(AssertionError, 'partial/refused input cannot pass'):
            input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'type_text', {'text': 'abc'})

    def test_missing_grounding_sends_no_input(self):
        mcp = Mock()
        mcp.tool.side_effect = [{'structuredContent': {**CALC, 'elements': []}}, WINDOWS]
        with self.assertRaises(GroundingUnavailable):
            input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'type_text', {'text': 'abc'})
        self.assertEqual(mcp.tool.call_count, 2)

    def test_partial_delivery_stays_failure_even_if_dialog_appears(self):
        mcp = Mock()
        partial = {'structuredContent': {**GOOD_DELIVERY['structuredContent'], 'effect': 'partial'}}
        dialog = {**CALC, 'elements': [{'role': 'dialog'}]}
        mcp.tool.side_effect = [{'structuredContent': CALC}, WINDOWS, partial,
                                {'structuredContent': dialog}, WINDOWS]
        with self.assertRaisesRegex(AssertionError, 'partial/refused input cannot pass'):
            input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'type_text', {'text': 'abc'})
        self.assertEqual(mcp.tool.call_count, 5)

    def test_unknown_transport_outcome_is_not_replayed(self):
        mcp = Mock()
        mcp.tool.side_effect = [{'structuredContent': CALC}, WINDOWS, TimeoutError('unknown')]
        with patch('production_app_smoke.read', side_effect=OSError('unavailable')):
            with self.assertRaises(TimeoutError):
                input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'type_text', {'text': 'abc'})
        self.assertEqual(mcp.tool.call_count, 3)


class ObjectsSetupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name).resolve()

    def prepare(self, side_effect):
        mcp = Mock()
        mcp.directory = self.directory
        mcp.tool.side_effect = side_effect
        return mcp, lambda: prepare_inkscape_objects(mcp, TARGET, 'cua-smoke-inkscape.svg', self.directory)

    @staticmethod
    def names(mcp):
        return [call.args[0] for call in mcp.tool.call_args_list]

    def test_existing_object_needs_no_setup_click(self):
        mcp, run = self.prepare(observed(INKSCAPE))
        self.assertEqual(run(), {'performed': False})
        self.assertEqual(self.names(mcp), ['get_window_state', 'list_windows'])

    def test_missing_object_uses_one_semantic_background_click_then_fresh_verification(self):
        mcp, run = self.prepare([*observed(INKSCAPE_NO_OBJECTS), SETUP_CLICK, *observed(INKSCAPE)])
        setup = run()
        self.assertEqual(self.names(mcp), ['get_window_state', 'list_windows', 'click',
                                           'get_window_state', 'list_windows'])
        self.assertEqual(mcp.tool.call_args_list[2].args[1],
                         {**TARGET, 'element_token': 'token-20', 'delivery_mode': 'background'})
        self.assertEqual((setup['kind'], setup['plugin_input_proof'], setup['performed']),
                         ('accessibility_setup', False, True))
        self.assertTrue((self.directory / 'objects-panel-setup.json').is_file())

    def test_ungrounded_opener_sends_nothing(self):
        def mutate(**change):
            state = copy.deepcopy(INKSCAPE_NO_OBJECTS)
            state['elements'][1].update(change)
            return state
        duplicate = copy.deepcopy(INKSCAPE_NO_OBJECTS)
        duplicate['elements'].append({**duplicate['elements'][1], 'element_index': 21})
        cases = [mutate(description='Open Objects'), mutate(description=OPEN_OBJECTS_DESCRIPTION + ' '),
                 mutate(role='menu item'), mutate(enabled=False), mutate(actions=[]),
                 mutate(element_token=''), mutate(element_index=99), duplicate]
        cases.append({**INKSCAPE_NO_OBJECTS, 'elements': INKSCAPE_NO_OBJECTS['elements'][:1]})
        cases.append({**INKSCAPE_NO_OBJECTS, 'tree_markdown': INKSCAPE_NO_OBJECTS['tree_markdown'].replace(
            'button', 'label')})
        for state in cases:
            mcp, run = self.prepare(observed(state))
            with self.subTest(state=state), self.assertRaises(GroundingUnavailable):
                run()
            self.assertEqual(self.names(mcp), ['get_window_state', 'list_windows'])

    def test_exact_offscreen_annotation_allows_semantic_setup(self):
        state = copy.deepcopy(INKSCAPE_NO_OBJECTS)
        state['elements'][1]['description'] += OFFSCREEN_DESCRIPTION
        mcp, run = self.prepare([*observed(state), SETUP_CLICK, *observed(INKSCAPE)])
        self.assertTrue(run()['performed'])
        self.assertEqual(self.names(mcp).count('click'), 1)

    def test_setup_refuses_incomplete_or_nonsemantic_delivery(self):
        for change in ({'effect': 'partial'}, {'effect': 'refused'}, {'effect': 'unknown'},
                       {'route': 'synthetic_events'}, {'delivery': None},
                       {'delivery': {'mode': 'unknown'}}):
            response = {'structuredContent': {**SETUP_CLICK['structuredContent'], **change}}
            mcp, run = self.prepare([*observed(INKSCAPE_NO_OBJECTS), response])
            with self.subTest(change=change), self.assertRaises(AssertionError):
                run()
            self.assertEqual(self.names(mcp).count('click'), 1)

    def test_setup_requires_complete_tree_single_window_and_no_dialog(self):
        mcp, run = self.prepare(observed({**INKSCAPE_NO_OBJECTS, 'truncated': True}))
        with self.assertRaisesRegex(GroundingUnavailable, 'not proven complete'):
            run()
        self.assertEqual(self.names(mcp), ['get_window_state'])
        extra = {'structuredContent': {'windows': [TARGET, {'pid': 123, 'window_id': 789}]}}
        mcp, run = self.prepare([{'structuredContent': INKSCAPE_NO_OBJECTS}, extra])
        with self.assertRaisesRegex(GroundingUnavailable, 'extra app window'):
            run()
        self.assertNotIn('click', self.names(mcp))
        dialog = {**INKSCAPE_NO_OBJECTS,
                  'elements': INKSCAPE_NO_OBJECTS['elements'] + [{'role': 'dialog'}]}
        mcp, run = self.prepare(observed(dialog))
        with self.assertRaisesRegex(GroundingUnavailable, 'unexpected dialog'):
            run()
        self.assertNotIn('click', self.names(mcp))

    def test_ambiguous_or_disabled_existing_object_is_not_setup(self):
        for rows in ([INKSCAPE['elements'][1], {**INKSCAPE['elements'][1], 'element_index': 13}],
                     [{**INKSCAPE['elements'][1], 'enabled': False}]):
            mcp, run = self.prepare(observed({**INKSCAPE, 'elements': rows}))
            with self.assertRaisesRegex(GroundingUnavailable, 'ambiguous or disabled'):
                run()
            self.assertEqual(self.names(mcp), ['get_window_state', 'list_windows'])

    def test_click_result_must_be_background_and_never_retried(self):
        for response in ({'structuredContent': {'delivery': {'mode': 'foreground'}}},
                         {'isError': True, 'structuredContent': {}}):
            mcp, run = self.prepare([*observed(INKSCAPE_NO_OBJECTS), response])
            with self.assertRaises(AssertionError):
                run()
            self.assertEqual(self.names(mcp).count('click'), 1)
        mcp, run = self.prepare([*observed(INKSCAPE_NO_OBJECTS), TimeoutError('unknown')])
        with patch('production_app_smoke.read', side_effect=OSError('unavailable')), \
                self.assertRaises(TimeoutError):
            run()
        self.assertEqual(self.names(mcp).count('click'), 1)
        self.assertEqual(mcp.tool.call_count, 3)

    def test_unverified_result_after_click_fails_closed(self):
        duplicate = {**INKSCAPE, 'elements': INKSCAPE['elements'] + [
            {**INKSCAPE['elements'][1], 'element_index': 13}]}
        for after in (INKSCAPE_NO_OBJECTS, duplicate, {**INKSCAPE, 'truncated': True},
                      {**INKSCAPE, 'elements': [{**INKSCAPE['elements'][1], 'enabled': False}]}):
            mcp, run = self.prepare([*observed(INKSCAPE_NO_OBJECTS), SETUP_CLICK, *observed(after)])
            with self.subTest(after=after), self.assertRaises(GroundingUnavailable):
                run()
            self.assertEqual(self.names(mcp).count('click'), 1)
            self.assertNotIn('hotkey', self.names(mcp))

    def test_inkscape_run_sets_up_before_raw_input_and_marks_it_not_plugin_proof(self):
        document = create_documents(self.directory, 'inkscape-only')['inkscape']
        mcp = Mock()
        mcp.directory = self.directory
        mcp.tool.side_effect = [{'structuredContent': {}},
                                *observed(INKSCAPE_NO_OBJECTS), SETUP_CLICK, *observed(INKSCAPE),
                                *observed(INKSCAPE), TimeoutError('unknown hotkey')]
        with patch('production_app_smoke.Path.iterdir', return_value=iter([])), \
                patch('production_app_smoke.discover', return_value=(TARGET, {})) as discover, \
                patch('production_app_smoke.read', return_value='{"pid": 999}'), \
                self.assertRaises(TimeoutError):
            run_app(mcp, 'inkscape', document, self.directory)
        self.assertEqual(self.names(mcp), ['launch_app', 'get_window_state', 'list_windows', 'click',
                                           'get_window_state', 'list_windows',
                                           'get_window_state', 'list_windows', 'hotkey'])
        tag = inkscape_app_id_tag(self.directory)
        launch = mcp.tool.call_args_list[0].args[1]
        self.assertEqual(launch['additional_arguments'], [f'--app-id-tag={tag}', str(document)])
        self.assertEqual(discover.call_args.args[4], [f'--app-id-tag={tag}'])
        self.assertEqual(mcp.tool.call_args_list[8].args[1],
                         {**TARGET, 'keys': ['ctrl', 'a'], 'delivery_mode': 'background'})

    def test_calc_run_has_no_setup_click(self):
        document = create_documents(self.directory)['calc']
        mcp = Mock()
        mcp.directory = self.directory
        mcp.tool.side_effect = [{'structuredContent': {}}, *observed(CALC), TimeoutError('unknown')]
        with patch('production_app_smoke.Path.iterdir', return_value=iter([])), \
                patch('production_app_smoke.discover', return_value=(TARGET, {})), \
                patch('production_app_smoke.read', return_value='{"pid": 999}'), \
                self.assertRaises(TimeoutError):
            run_app(mcp, 'calc', document, self.directory)
        self.assertNotIn('click', self.names(mcp))

    def test_observation_helper_requests_explicit_budget(self):
        mcp = Mock()
        mcp.tool.side_effect = observed(INKSCAPE)
        observe(mcp, TARGET, 'cua-smoke-inkscape.svg')
        self.assertEqual(mcp.tool.call_args_list[0].args,
                         ('get_window_state', {**TARGET, 'timeout_ms': 15000, 'full_output': True}))


UNPROVEN = {'degraded': True,
            'degraded_reason': 'accessibility_window_identity_unproven: tree is application-scoped'}


class ObservationRetryTests(unittest.TestCase):
    FILENAME = 'cua-smoke-calc.ods'

    def names(self, mcp):
        return [call.args[0] for call in mcp.tool.call_args_list]

    def test_transient_identity_refusal_succeeds_on_second_observation(self):
        with tempfile.TemporaryDirectory() as temp:
            mcp = Mock()
            mcp.directory = Path(temp)
            mcp.tool.side_effect = [*observed({**CALC, **UNPROVEN}), *observed(CALC)]
            self.assertEqual(observe(mcp, TARGET, self.FILENAME), CALC)
            self.assertEqual(self.names(mcp), ['get_window_state', 'list_windows'] * 2)
            record = json.loads((Path(temp) / 'observation-retries.json').read_text())
            self.assertEqual(record['max_attempts'], OBSERVATION_ATTEMPTS)
            self.assertEqual([row['attempt'] for row in record['attempts']], [1])
            self.assertTrue(record['attempts'][0]['degraded_reason'].startswith(
                'accessibility_window_identity_unproven'))

    def test_persistent_identity_refusal_stops_after_fixed_bound(self):
        mcp = Mock()
        mcp.tool.side_effect = [*observed({**CALC, **UNPROVEN})] * 5
        with self.assertRaisesRegex(GroundingUnavailable, 'still unproven'):
            observe(mcp, TARGET, self.FILENAME)
        self.assertEqual(OBSERVATION_ATTEMPTS, 2)
        self.assertEqual(self.names(mcp).count('get_window_state'), 2)

    def test_non_retryable_states_fail_immediately(self):
        for change in ({'truncated': True, **UNPROVEN}, {'elements_complete': False, **UNPROVEN},
                       {'timeout_ms': 1000, **UNPROVEN}, {'degraded': True, 'degraded_reason': 'other'},
                       {'degraded': True}, {'truncated': True}):
            mcp = Mock()
            mcp.tool.side_effect = observed({**CALC, **change})
            with self.subTest(change=change), self.assertRaisesRegex(GroundingUnavailable, 'not proven complete'):
                observe(mcp, TARGET, self.FILENAME)
            self.assertEqual(self.names(mcp), ['get_window_state'])

    def test_wrong_title_fails_immediately(self):
        mcp = Mock()
        mcp.tool.side_effect = observed({**CALC, **UNPROVEN, 'window_title': 'other.ods'})
        with self.assertRaisesRegex(AssertionError, 'not the synthetic document'):
            observe(mcp, TARGET, self.FILENAME)
        self.assertEqual(self.names(mcp), ['get_window_state'])

    def test_wrong_windows_fail_on_every_attempt_without_retrying(self):
        extra = {'structuredContent': {'windows': [TARGET, {'pid': 123, 'window_id': 789}]}}
        wrong = {'structuredContent': {'windows': [{'pid': 123, 'window_id': 999}]}}
        for windows in (extra, wrong):
            mcp = Mock()
            mcp.tool.side_effect = [{'structuredContent': {**CALC, **UNPROVEN}}, windows]
            with self.subTest(windows=windows), self.assertRaisesRegex(GroundingUnavailable, 'extra app window'):
                observe(mcp, TARGET, self.FILENAME)
            self.assertEqual(mcp.tool.call_count, 2)
        # The second attempt is checked too, even when the first window list was fine.
        mcp = Mock()
        mcp.tool.side_effect = [*observed({**CALC, **UNPROVEN}),
                                {'structuredContent': CALC}, extra]
        with self.assertRaisesRegex(GroundingUnavailable, 'extra app window'):
            observe(mcp, TARGET, self.FILENAME)

    def test_retry_never_replays_input(self):
        mcp = Mock()
        mcp.tool.side_effect = [*observed(CALC), GOOD_DELIVERY,
                                *observed({**CALC, **UNPROVEN}), *observed(CALC)]
        input_step(mcp, TARGET, self.FILENAME, 'calc', 'insert', 'type_text', {'text': 'abc'})
        self.assertEqual(self.names(mcp), ['get_window_state', 'list_windows', 'type_text',
                                           'get_window_state', 'list_windows',
                                           'get_window_state', 'list_windows'])
        self.assertEqual(self.names(mcp).count('type_text'), 1)


class RawInputReportTests(unittest.TestCase):
    def test_input_step_marks_attempt_before_dispatch(self):
        mcp = Mock()
        seen = []
        responses = [*observed(CALC)]

        def tool(name, arguments):
            if name == 'type_text':
                seen.append(mcp.raw_input_attempted)
                raise TimeoutError('unknown')
            return responses.pop(0)
        mcp.tool.side_effect = tool
        mcp.raw_input_attempted = False
        with patch('production_app_smoke.read', side_effect=OSError('unavailable')), self.assertRaises(TimeoutError):
            input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'type_text', {'text': 'abc'})
        self.assertEqual(seen, [True])

    def test_grounding_loss_after_raw_attempt_is_failed_not_delivered(self):
        mcp = Mock()
        mcp.raw_input_attempted = True
        outcome = grounding_outcome(mcp, GroundingUnavailable('lost'))
        self.assertEqual(outcome, {'result': 'failed', 'blocker': 'lost', 'raw_input_attempted': True})
        self.assertNotIn('actions_delivered', outcome)

    def test_grounding_loss_without_raw_attempt_stays_inspection_only(self):
        for mcp in (Mock(), None):  # A bare Mock attribute is truthy but not True.
            with self.subTest(mcp=mcp):
                self.assertEqual(grounding_outcome(mcp, GroundingUnavailable('lost')),
                                 {'result': 'inspection_only', 'blocker': 'lost'})

    def test_post_input_grounding_failure_is_reported_failed(self):
        mcp = Mock()
        mcp.tool.side_effect = [*observed(CALC), GOOD_DELIVERY, *observed({**CALC, 'truncated': True})]
        with self.assertRaises(GroundingUnavailable) as caught:
            input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'type_text', {'text': 'abc'})
        self.assertEqual(grounding_outcome(mcp, caught.exception)['result'], 'failed')


if __name__ == '__main__':
    unittest.main()
