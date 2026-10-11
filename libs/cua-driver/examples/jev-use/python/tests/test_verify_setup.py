from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from verify_setup import BASE, acted_path, fixture, runner_command, verify


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
    def test_python_runner_can_start_without_a_desktop(self):
        result = subprocess.run(
            runner_command('python', 'mock') + ['--help'],
            cwd=BASE, capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('--fixture-url', result.stdout)

    @unittest.skipUnless(shutil.which('node') and (BASE / 'node_modules/tsx').is_dir(),
                         'requires installed TypeScript dependencies and Node')
    def test_typescript_runner_reaches_argument_validation(self):
        result = subprocess.run(
            runner_command('typescript', 'mock') + ['--max-steps', '0'],
            cwd=BASE, capture_output=True, text=True, timeout=30,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('max-steps must be a positive integer', result.stderr)

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


    def write_log(self, directory, events):
        log = Path(directory) / 'run.jsonl'
        log.write_text(''.join(json.dumps(event) + '\n' for event in events))
        return log

    def submit_step(self, tool, status):
        visual = {'status': status}
        if status == 'ok':
            visual.update(capture_id='capture-1', region_count=3)
        return {'event': 'step', 'step': 2, 'candidate': 'submit-form', 'tool': tool, 'visual': visual}

    def test_acted_path_reports_dom_and_visual_submissions(self):
        type_step = {'event': 'step', 'step': 1, 'candidate': 'type-verification-value',
                     'tool': 'browser_type', 'visual': {'status': 'not_installed', 'error_code': 'not_installed'}}
        self.assertEqual(
            acted_path([type_step, self.submit_step('browser_click', 'not_installed')]),
            {'submit_tool': 'browser_click', 'acted_path': 'page_structure',
             'submit_delivery_mode': None,
             'visual_statuses': ['not_installed', 'not_installed'], 'escalations': []},
        )
        self.assertEqual(
            acted_path([type_step, self.submit_step('click', 'ok')])['acted_path'], 'visual'
        )
        self.assertEqual(acted_path([type_step])['acted_path'], None)

    def replay(self, events, *, submit=True, code='0', **options):
        """Run a child that replays a JSONL log and optionally submits the token."""
        child = (
            "import sys\n"
            "from pathlib import Path\n"
            "from urllib.parse import urlencode\n"
            "from urllib.request import Request, urlopen\n"
            "url, source, log, submit, code = sys.argv[1:]\n"
            "if submit == '1':\n"
            "    urlopen(Request(url + 'submit', data=urlencode({'value': 'proof'}).encode()), timeout=2).close()\n"
            "Path(log).write_text(Path(source).read_text())\n"
            "raise SystemExit(int(code))\n"
        )
        with tempfile.TemporaryDirectory() as directory, \
                fixture(visual=options.get('visual_fixture', False)) as url:
            source = self.write_log(directory, events)
            log = Path(directory) / 'out.jsonl'
            command = [sys.executable, '-c', child, url, str(source), str(log), '1' if submit else '0', code]
            return verify(command, url, 'proof', log, **options)

    def verified(self, tool, status):
        return [self.submit_step(tool, status), {'event': 'outcome', 'outcome': 'verified', 'token': 'proof'}]

    def test_reports_page_structure_path(self):
        result = self.replay(self.verified('browser_click', 'not_installed'))
        self.assertEqual(result['acted_path'], 'page_structure')
        self.assertEqual(result['submit_tool'], 'browser_click')

    def test_required_visual_path_fails_when_dom_click_acted(self):
        with self.assertRaisesRegex(RuntimeError, 'Visual path was required.*browser_click'):
            self.replay(self.verified('browser_click', 'ok'), require_visual=True)

    def test_required_visual_path_accepts_capture_bound_click(self):
        result = self.replay(self.verified('click', 'ok'), require_visual=True, visual_fixture=True)
        self.assertEqual(result['acted_path'], 'visual')
        self.assertEqual(result['visual_statuses'], ['ok'])

    def test_expected_visual_status_must_be_logged_on_every_step(self):
        with self.assertRaisesRegex(RuntimeError, "visual status 'not_installed'"):
            self.replay(self.verified('browser_click', 'error'), expect_visual_status='not_installed')

    def test_visual_fixture_fallback_is_observable_and_never_claims_success(self):
        events = [
            {'event': 'step', 'step': 1, 'candidate': 'type-verification-value', 'tool': 'browser_type',
             'visual': {'status': 'not_installed', 'error_code': 'not_installed'}},
            {'event': 'step', 'step': 2, 'candidate': 'reobserve', 'tool': None,
             'visual': {'status': 'not_installed', 'error_code': 'not_installed'}},
            {'event': 'outcome', 'outcome': 'budget_exhausted', 'token': 'proof'},
        ]
        result = self.replay(events, submit=False, code='1', visual_fixture=True,
                             expect_visual_status='not_installed')
        self.assertEqual(result['outcome'], 'budget_exhausted')
        self.assertIsNone(result['acted_path'])
        self.assertEqual(result['observed'], {'submitted': None})
        with self.assertRaisesRegex(RuntimeError, 'unexpected submission'):
            self.replay(events, submit=True, code='1', visual_fixture=True,
                        expect_visual_status='not_installed')


    def test_skipped_steps_are_not_visual_attempts(self):
        skipped = {'status': 'skipped', 'reason': 'page_structure_candidate'}
        type_step = {'event': 'step', 'step': 1, 'candidate': 'type-verification-value',
                     'tool': 'browser_type', 'visual': skipped}
        events = [type_step, self.submit_step('click', 'ok'),
                  {'event': 'outcome', 'outcome': 'verified', 'token': 'proof'}]
        result = self.replay(events, expect_visual_status='ok', require_visual=True, visual_fixture=True)
        self.assertEqual(result['visual_statuses'], ['skipped', 'ok'])
        only_skipped = [
            {**type_step, 'step': 1},
            {'event': 'step', 'step': 2, 'candidate': 'submit-form', 'tool': 'browser_click', 'visual': skipped},
            {'event': 'outcome', 'outcome': 'verified', 'token': 'proof'},
        ]
        with self.assertRaisesRegex(RuntimeError, "visual status 'not_installed'"):
            self.replay(only_skipped, expect_visual_status='not_installed')

    def test_reports_foreground_escalation_after_background_refusal(self):
        refused = {**self.submit_step('click', 'ok'), 'delivery_mode': 'background',
                   'action_error': 'background_unavailable',
                   'escalation': {'from': 'background', 'to': 'foreground', 'reason': 'background_unavailable'}}
        foreground = {**self.submit_step('click', 'ok'), 'step': 3,
                      'candidate': 'submit-form-foreground', 'delivery_mode': 'foreground'}
        events = [refused, foreground, {'event': 'outcome', 'outcome': 'verified', 'token': 'proof'}]
        result = self.replay(events, require_visual=True, visual_fixture=True)
        self.assertEqual(result['acted_path'], 'visual')
        self.assertEqual(result['submit_delivery_mode'], 'foreground')
        self.assertEqual(result['escalations'],
                         [{'from': 'background', 'to': 'foreground', 'reason': 'background_unavailable'}])
        self.assertEqual(acted_path([refused])['submit_tool'], None)


if __name__ == '__main__':
    unittest.main()
