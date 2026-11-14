"""CLI boundaries only; live fixture journals own desktop assertions."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
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

    def test_consumed_ledger_blocks_fresh_runner_process_before_driver(self):
        # Exercise the real runner's exclusive-create barrier, not a live bus or input.
        bootstrap = '''
import runpy, sys, types
from unittest.mock import patch
sys.path.insert(0, sys.argv[1])
runner, directory = sys.argv[2:]
sys.argv = [runner, "--directory", directory, "--seq", "9", "--case", "A"]
class Bus:
    def get_object(self, *args):
        return object()
dbus = types.ModuleType("dbus")
dbus.SessionBus = lambda: Bus()
dbus.Interface = lambda *args: object()
mainloop = types.ModuleType("dbus.mainloop")
glib = types.ModuleType("dbus.mainloop.glib")
glib.DBusGMainLoop = lambda **kwargs: None
activation = types.ModuleType("activation_probe")
activation.observe = lambda *args: (_ for _ in ()).throw(AssertionError("activation called"))
sys.modules.update({"dbus": dbus, "dbus.mainloop": mainloop,
                    "dbus.mainloop.glib": glib, "activation_probe": activation})
with patch("subprocess.check_output", side_effect=AssertionError("driver identity called")), \\
     patch("subprocess.Popen", side_effect=AssertionError("driver started")):
    runpy.run_path(runner, run_name="__main__")
'''
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'fixture-owner.json').write_text(json.dumps({'owner': ':1.1', 'pid': 123}))
            ledger = root / 'operation-9.jsonl'
            original = '{"seq": 9, "status": "consumed_before_submission"}\n'
            ledger.write_text(original)
            for _ in range(2):
                result = subprocess.run(
                    [sys.executable, '-c', bootstrap, str(RUNNER.parent), str(RUNNER), directory],
                    capture_output=True, text=True,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('FileExistsError', result.stderr)
                self.assertNotIn('driver identity called', result.stderr)
                self.assertNotIn('driver started', result.stderr)
                self.assertEqual(ledger.read_text(), original)

if __name__ == '__main__':
    unittest.main()
