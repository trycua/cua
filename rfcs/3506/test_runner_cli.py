"""CLI boundaries only; live fixture journals own desktop assertions."""
from pathlib import Path
import subprocess
import sys
import unittest

RUNNER = Path(__file__).with_name('run_input_case.py')

class RunnerCliTests(unittest.TestCase):
    def test_guest_binary_and_ungated_flags_are_available(self):
        result = subprocess.run([sys.executable, str(RUNNER), '--help'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('--binary', result.stdout)
        self.assertIn('--ungated', result.stdout)

    def test_ungated_takeover_is_rejected_before_bus_or_ledger(self):
        result = subprocess.run([sys.executable, str(RUNNER), '--directory', '/nonexistent-cua-proof',
                                 '--seq', '1', '--case', 'postcheck_takeover', '--ungated'],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('ungated requires A or B', result.stderr)

if __name__ == '__main__':
    unittest.main()
