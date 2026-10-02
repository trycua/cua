"""Fail-closed evidence and cleanup boundaries of the opt-in native check."""
import importlib.util
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / 'libs/cua-driver/tests/runners/macos-lume/product_window_regression.py'
spec = importlib.util.spec_from_file_location('product_window_regression', SOURCE)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ProductWindowEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.args = SimpleNamespace(artifacts=Path(self.temp.name) / 'new',
                                    socket=Path('/owned/driver.sock'), pid=42, source_sha='a' * 40)
        self.check = module.Check(self.args)

    def test_nonce_oracle_uses_exact_document_value_not_metadata(self):
        state = {'pid': 123, 'window_id': 456, 'label': 'nonce',
                 'elements': [{'role': 'AXTextArea', 'label': 'nonce'}]}
        self.assertEqual(module.document_value(state, 123, 456), '')
        state['elements'][0]['value'] = 'nonce'
        self.assertEqual(module.document_value(state, 123, 456), 'nonce')
        with self.assertRaises(RuntimeError):
            module.document_value(state, 123, 789)

    def test_existing_evidence_is_never_reused(self):
        with self.assertRaises(FileExistsError):
            module.Check(self.args)

    def test_refusal_requires_error_exit_and_exact_reason(self):
        for code, text in [(0, 'own authorization process'), (1, 'unrelated error')]:
            with self.subTest(code=code, text=text), patch.object(module.subprocess, 'run',
                 return_value=subprocess.CompletedProcess([], code, text, '')):
                with self.assertRaises(RuntimeError):
                    self.check.call('bring_to_front', {}, refusal='own authorization process')

    def test_source_mismatch_stops_before_native_window_operations(self):
        with patch.object(module.subprocess, 'run'), patch.object(self.check, 'call',
             return_value={'source_sha': 'b' * 40}) as call:
            with self.assertRaises(RuntimeError):
                self.check.run()
        self.assertEqual(call.call_args_list, [unittest.mock.call('get_config', {})])

    def test_cleanup_refuses_reused_pid_with_different_executable(self):
        self.check.owned[123] = '/System/Applications/TextEdit.app/Contents/MacOS/TextEdit'
        with patch.object(module, 'alive', return_value=True), \
             patch.object(module.subprocess, 'check_output', return_value='/unrelated/app\n'), \
             patch.object(module.os, 'kill') as kill:
            with self.assertRaises(RuntimeError):
                self.check.terminate(123)
            kill.assert_not_called()

    def test_interrupted_partial_run_cannot_report_pass_after_cleanup(self):
        self.check.owned[123] = '/owned/app'
        def interrupted():
            self.check.cases.append({'case': 'own-move'})
            raise KeyboardInterrupt()
        with patch.object(self.check, 'run', side_effect=interrupted), \
             patch.object(self.check, 'terminate') as terminate, \
             patch.object(module, 'alive', return_value=True):
            self.assertEqual(self.check.execute(), 1)
        terminate.assert_called_once_with(123)
        self.assertEqual(self.check.report['status'], 'failed')
        self.assertEqual(self.check.report['cases'], [{'case': 'own-move'}])
        self.assertTrue(any('KeyboardInterrupt' in e for e in self.check.report['errors']))

    def test_dead_daemon_at_final_boundary_cannot_report_pass(self):
        with patch.object(self.check, 'run'), patch.object(module, 'alive', return_value=False):
            self.assertEqual(self.check.execute(), 1)
        self.assertEqual(self.check.report['status'], 'failed')
        self.assertFalse(self.check.report['daemon_alive'])
        self.assertIn('supplied daemon exited', self.check.report['errors'])

    def test_cleanup_failure_cannot_report_pass(self):
        self.check.owned[123] = '/owned/app'
        with patch.object(self.check, 'run'), \
             patch.object(self.check, 'terminate', side_effect=RuntimeError('still alive')), \
             patch.object(module, 'alive', return_value=True):
            self.assertEqual(self.check.execute(), 1)
        self.assertEqual(self.check.report['status'], 'failed')
        self.assertIn('cleanup: still alive', self.check.report['errors'])

    def test_native_timeout_preserves_failure_and_attempts_cleanup(self):
        self.check.owned[123] = '/owned/app'
        with patch.object(self.check, 'run', side_effect=subprocess.TimeoutExpired('driver', 30)), \
             patch.object(self.check, 'terminate') as terminate, \
             patch.object(module, 'alive', return_value=True):
            self.assertEqual(self.check.execute(), 1)
        terminate.assert_called_once_with(123)
        self.assertEqual(self.check.report['status'], 'failed')


if __name__ == '__main__':
    unittest.main()
