"""Check the fixed fixture command's actual shell argv without signing or launching code."""
import os
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[3]
ENTRY = ROOT / 'scripts/ci/macos/run-embedded-sdk-window.sh'


class EmbeddedWindowFixtureRunnerTests(unittest.TestCase):
    def test_certificate_prefix_is_attached_to_optional_codesign_argument(self):
        commands = [line for line in ENTRY.read_text().splitlines()
                    if line.startswith('codesign -d --extract-certificates')]
        self.assertEqual(len(commands), 1)
        for directory, binary in [('/tmp/fixture', '/tmp/native-fixture'),
                                  ('/tmp/fixture with spaces', '/tmp/native fixture')]:
            with self.subTest(directory=directory):
                # Stub only this codesign invocation and capture the shell-expanded argv.
                shell = 'codesign() { printf "%s\\0" "$@"; }\n' + commands[0]
                result = subprocess.run(['bash', '-c', shell], check=True, capture_output=True,
                                        env=dict(os.environ, FIXTURE_DIR=directory, binary=binary))
                self.assertEqual(result.stdout.decode().split('\0')[:-1],
                                 ['-d', '--extract-certificates=' + directory + '/signing-cert', binary])


if __name__ == '__main__':
    unittest.main()
