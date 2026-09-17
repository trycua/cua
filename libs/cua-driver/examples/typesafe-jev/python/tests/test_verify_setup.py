from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from verify_setup import fixture, verify


CHILD = """
import json, sys
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import Request, urlopen
url, value, outcome, log, code = sys.argv[1:]
with urlopen(Request(url + 'submit', data=urlencode({'value': value}).encode()), timeout=2):
    pass
Path(log).write_text(json.dumps({'event': 'outcome', 'outcome': outcome, 'token': 'proof'}) + '\\n')
raise SystemExit(int(code))
"""


class VerifySetupTests(unittest.TestCase):
    def test_fixture_closes_on_success(self):
        with fixture() as url:
            port = int(url.split(':')[-1].rstrip('/'))
            with socket.create_connection(('127.0.0.1', port), timeout=1):
                pass
        with socket.socket() as connection:
            self.assertNotEqual(connection.connect_ex(('127.0.0.1', port)), 0)

    def test_fixture_closes_on_failure(self):
        with self.assertRaisesRegex(RuntimeError, 'deliberate'):
            with fixture() as url:
                port = int(url.split(':')[-1].rstrip('/'))
                raise RuntimeError('deliberate')
        with socket.socket() as connection:
            self.assertNotEqual(connection.connect_ex(('127.0.0.1', port)), 0)

    def check_child(self, value='proof', outcome='verified', code='0'):
        with tempfile.TemporaryDirectory() as directory, fixture() as url:
            log = Path(directory) / 'run.jsonl'
            command = [sys.executable, '-c', CHILD, url, value, outcome, str(log), code]
            return verify(command, url, 'proof', log)

    def test_requires_matching_independent_state(self):
        result = self.check_child()
        self.assertEqual(result['observed'], {'submitted': 'proof'})
        self.assertEqual(result['outcome'], 'verified')

    def test_rejects_wrong_submission_despite_verified_event(self):
        with self.assertRaisesRegex(RuntimeError, 'Independent'):
            self.check_child(value='wrong')

    def test_rejects_missing_verified_outcome_despite_correct_state(self):
        with self.assertRaisesRegex(RuntimeError, 'Runner'):
            self.check_child(outcome='unknown')

    def test_rejects_failed_runner_despite_correct_state(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.check_child(code='1')

    def test_unattended_live_requires_key_before_starting(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'proof'
            result = subprocess.run(
                [sys.executable, str(Path(__file__).resolve().parents[2] / 'verify_setup.py'),
                 '--live', '--output-dir', str(output)],
                input='', capture_output=True, text=True,
                env={key: value for key, value in os.environ.items() if key != 'TYPESAFE_API_KEY'},
                timeout=10,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('TYPESAFE_API_KEY', result.stderr)
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
