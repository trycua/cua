"""No native applications or input: fixtures and fail-closed orchestration only."""
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

from production_app_smoke import (
    GroundingUnavailable, check_delivery, create_documents, ground, input_step,
    mapped_plugin, package_owner, verify_calc, verify_inkscape,
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
        self.assertEqual(mapped_plugin(row, plugin), [row])
        for invalid in ('', row + ' (deleted)', row.replace(str(stat.st_ino), '0'),
                        row.replace(str(plugin), '/elsewhere/plugin.so')):
            with self.assertRaises(AssertionError):
                mapped_plugin(invalid, plugin)

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
