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



class AppBundleHandoffTests(unittest.TestCase):
    @staticmethod
    def helper(name):
        text = ENTRY.read_text()
        start = text.index(name + '() {')
        return text[start:text.index('\n}\n', start) + 3]

    def test_launch_uses_launchservices_exact_app_and_fresh_process(self):
        shell = 'run_bounded_command() { printf "%s\\0" "$@"; }\n' + self.helper('launch_fixture_app')
        shell += '\nlaunch_fixture_app 15 preflight-0 --status-only --evidence "$FIXTURE_DIR/result.json"'
        env = dict(os.environ, APP='/tmp/Cua Fixture.app', FIXTURE_DIR='/tmp/evidence with spaces', source_sha='a' * 40)
        result = subprocess.run(['bash', '-c', shell], env=env, check=True, capture_output=True)
        self.assertEqual(result.stdout.decode().split('\0')[:-1], [
            '/usr/bin/open', '-n', '-W', '-a', env['APP'], '--env', 'CUA_E2E_SOURCE_SHA=' + 'a' * 40,
            '--stdout', env['FIXTURE_DIR'] + '/preflight-0.stdout', '--stderr', env['FIXTURE_DIR'] + '/preflight-0.stderr',
            '--args', '--status-only', '--evidence', env['FIXTURE_DIR'] + '/result.json'])

    def simulate_handoff(self, *, authorized=False, trust_at=2, timeout=False, failure=''):
        import json
        import tempfile
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            shell = '\n'.join(self.helper(name) for name in ['read_preflight_trust', 'record_handoff', 'ensure_fixture_trust'])
            shell += r'''
probe_count=0
launch_fixture_app() {
  probe_count=$((probe_count + 1))
  [[ "$SIM_FAILURE" != launch ]] || return 1
  [[ "$SIM_FAILURE" != missing-report ]] || return 0
  python3 - "$5" "$probe_count" "$APP" "$binary" "$source_sha" "$TRUST_AT" "$SIM_FAILURE" <<'PYREPORT'
import json, pathlib, sys
report, count, app, binary, sha, trust_at, failure = sys.argv[1:]
value = {'schema': 'cua-driver/embedded-sdk-window@1', 'source_sha': sha, 'status': 'preflight',
         'process': {'pid': int(count), 'bundle_path': app,
                     'bundle_id': 'com.trycua.fixture.embedded-sdk-window', 'executable': binary,
                     'appkit_main_thread': True, 'ax_trusted': int(count) >= int(trust_at)}}
if failure == 'wrong-identity': value['process']['bundle_id'] = 'com.apple.Terminal'
pathlib.Path(report).write_text(json.dumps(value))
PYREPORT
}
sleep() { if [[ "$SIM_TIMEOUT" == 1 ]]; then SECONDS=$((SECONDS + 301)); fi; }
ensure_fixture_trust
'''
            env = dict(os.environ, FIXTURE_DIR=str(root), APP='/tmp/Fixed Fixture.app',
                       binary='/tmp/Fixed Fixture.app/Contents/MacOS/embedded_menu_restore', source_sha='a' * 40,
                       ACCESSIBILITY_HANDOFF='1' if authorized else '0', TRUST_AT=str(trust_at),
                       SIM_TIMEOUT='1' if timeout else '0', SIM_FAILURE=failure)
            result = subprocess.run(['bash', '-c', shell], env=env, capture_output=True, timeout=5)
            handoff = root / 'handoff.json'
            return result.returncode, json.loads(handoff.read_text()) if handoff.exists() else None

    def test_missing_trust_fails_without_explicit_handoff(self):
        code, handoff = self.simulate_handoff()
        self.assertEqual(code, 2)
        self.assertEqual(handoff, {'requested': False, 'needed': True, 'completed': False,
                                  'initial_pid': 1, 'trusted_pid': None})

    def test_authorized_handoff_relaunches_before_resuming(self):
        code, handoff = self.simulate_handoff(authorized=True)
        self.assertEqual(code, 0)
        self.assertEqual(handoff, {'requested': True, 'needed': True, 'completed': True,
                                  'initial_pid': 1, 'trusted_pid': 2})

    def test_existing_trust_needs_no_handoff(self):
        code, handoff = self.simulate_handoff(trust_at=1)
        self.assertEqual(code, 0)
        self.assertFalse(handoff['needed'])
        self.assertFalse(handoff['requested'])

    def test_handoff_timeout_remains_failure(self):
        code, handoff = self.simulate_handoff(authorized=True, trust_at=100, timeout=True)
        self.assertEqual(code, 2)
        self.assertFalse(handoff['completed'])
        self.assertIsNone(handoff['trusted_pid'])

    def test_launch_failure_missing_report_and_wrong_identity_cannot_resume(self):
        for failure in ('launch', 'missing-report', 'wrong-identity'):
            with self.subTest(failure=failure):
                code, handoff = self.simulate_handoff(authorized=True, trust_at=1, failure=failure)
                self.assertEqual(code, 2)
                self.assertIsNone(handoff)

    def test_bundle_layout_preserves_payload_and_refuses_overwrite(self):
        import hashlib
        import plistlib
        import tempfile
        text = ENTRY.read_text()
        script = text.split("<<'PYBUNDLE'\n", 1)[1].split('\nPYBUNDLE', 1)[0]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / 'fake-native-payload'
            binary.write_bytes(b'non-executable unit test data')
            app = root / 'Test Fixture.app'
            command = ['python3', '-c', script, str(binary), str(app)]
            subprocess.run(command, check=True)
            copied = app / 'Contents/MacOS/embedded_menu_restore'
            self.assertEqual(hashlib.sha256(binary.read_bytes()).digest(), hashlib.sha256(copied.read_bytes()).digest())
            info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
            self.assertEqual(info['CFBundleIdentifier'], 'com.trycua.fixture.embedded-sdk-window')
            self.assertEqual(info['CFBundleExecutable'], 'embedded_menu_restore')
            self.assertEqual(info['CFBundlePackageType'], 'APPL')
            self.assertNotEqual(subprocess.run(command, capture_output=True).returncode, 0)


if __name__ == '__main__':
    unittest.main()
