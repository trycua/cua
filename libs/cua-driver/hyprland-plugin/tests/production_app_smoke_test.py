"""No native applications or input: fixtures and fail-closed orchestration only."""
import copy
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

from production_app_smoke import (
    LIMITS, GroundingUnavailable, check_delivery, create_documents, ground, input_step,
    kernel_file_identity, launch_arguments, mapped_plugin, package_owner,
    require_background_target, require_enabled_plugin, run_app, verify_calc, verify_inkscape,
)


TARGET = {'pid': 123, 'window_id': 456}
GOOD_DELIVERY = {'structuredContent': {'route': 'synthetic_events',
                                      'effect': 'unverifiable',
                                      'delivery': {'mode': 'background'}}}
CALC = {'window_title': 'cua-smoke-calc.ods - LibreOffice Calc',
        'elements': [{'role': 'text', 'label': 'Name Box', 'value': 'A1'}]}
WINDOWS = {'structuredContent': {'windows': [TARGET]}}


def changed_ods(original, text='a', empty_first=False):
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

    def test_alpm_owner_required_even_when_version_matches(self):
        executable = self.directory / 'app'
        executable.write_bytes(b'synthetic executable')
        executable.chmod(0o700)
        with patch('production_app_smoke.read', return_value='unrelated-package'):
            with self.assertRaisesRegex(AssertionError, 'noncanonical package owner'):
                package_owner(executable, 'libreoffice-fresh')

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

    def test_inkscape_launch_uses_supported_positional_document(self):
        self.assertEqual(launch_arguments('inkscape', Path('/docs/smoke.svg'), Path('/evidence')),
                         {'launch_path': '/usr/bin/inkscape',
                          'additional_arguments': ['/docs/smoke.svg']})

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

    def test_route_family_does_not_claim_plugin_transport_attribution(self):
        self.assertIs(LIMITS['plugin_transport_attribution'], False)

    def test_only_acknowledged_synthetic_background_delivery_accepted(self):
        check_delivery(GOOD_DELIVERY)
        for replacement in ({'route': 'atspi'}, {'effect': 'partial'},
                            {'delivery': {'mode': 'unknown'}}, {'delivery': {'mode': 'foreground'}}):
            with self.assertRaises(AssertionError):
                check_delivery({'structuredContent': {**GOOD_DELIVERY['structuredContent'], **replacement}})
        with self.assertRaises(AssertionError):
            check_delivery({**GOOD_DELIVERY, 'isError': True})

    def test_grounding_rejects_dialog_missing_selection_and_missing_canvas(self):
        ground(CALC, 'calc', 'insert')
        for state in ({'elements': []},
                      {'elements': [{'role': 'dialog', 'label': 'Recover documents'}]},
                      {'elements': [{'role': 'text', 'label': 'Name Box', 'value': 'B1'}]}):
            with self.assertRaises(GroundingUnavailable):
                ground(state, 'calc', 'insert')
        with self.assertRaises(GroundingUnavailable):
            ground(CALC, 'inkscape', 'select')
        ground({'elements': [{'role': 'drawing area'}]}, 'inkscape', 'select')
        with self.assertRaises(GroundingUnavailable):
            ground({'elements': [{'role': 'status bar', 'value': 'No objects selected'}]}, 'inkscape', 'move')
        ground({'elements': [{'role': 'status bar', 'value': '1 object selected'}]}, 'inkscape', 'move')

    def test_snapshot_action_snapshot_and_no_replay(self):
        mcp = Mock()
        mcp.tool.side_effect = [{'structuredContent': CALC}, WINDOWS, GOOD_DELIVERY,
                                {'structuredContent': CALC}, WINDOWS]
        input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'press_key', {'key': 'a'})
        calls = mcp.tool.call_args_list
        self.assertEqual([call.args[0] for call in calls],
                         ['get_window_state', 'list_windows', 'press_key', 'get_window_state', 'list_windows'])
        self.assertEqual(calls[2].args[1], {**TARGET, 'key': 'a', 'delivery_mode': 'background'})

    def test_missing_grounding_sends_no_input(self):
        mcp = Mock()
        mcp.tool.side_effect = [{'structuredContent': {**CALC, 'elements': []}}, WINDOWS]
        with self.assertRaises(GroundingUnavailable):
            input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'press_key', {'key': 'a'})
        self.assertEqual(mcp.tool.call_count, 2)

    def test_partial_delivery_stays_failure_even_if_dialog_appears(self):
        mcp = Mock()
        partial = {'structuredContent': {**GOOD_DELIVERY['structuredContent'], 'effect': 'partial'}}
        dialog = {**CALC, 'elements': [{'role': 'dialog'}]}
        mcp.tool.side_effect = [{'structuredContent': CALC}, WINDOWS, partial,
                                {'structuredContent': dialog}, WINDOWS]
        with self.assertRaises(AssertionError):
            input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'press_key', {'key': 'a'})
        self.assertEqual(mcp.tool.call_count, 5)

    def test_unknown_transport_outcome_is_not_replayed(self):
        mcp = Mock()
        mcp.tool.side_effect = [{'structuredContent': CALC}, WINDOWS, TimeoutError('unknown')]
        with patch('production_app_smoke.read', side_effect=OSError('unavailable')):
            with self.assertRaises(TimeoutError):
                input_step(mcp, TARGET, 'cua-smoke-calc.ods', 'calc', 'insert', 'press_key', {'key': 'a'})
        self.assertEqual(mcp.tool.call_count, 3)


if __name__ == '__main__':
    unittest.main()
