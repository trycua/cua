"""Direct Driver MCP runtime contracts shared by every production proof."""
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import proofs_path  # noqa: F401  Puts ../proofs on sys.path.
from production_mcp import DirectMCP, assert_distinct_runtimes, profile_environment, stop_process


class RuntimeTests(unittest.TestCase):
    def test_profiles_keep_managed_policy_and_remove_inherited_approval(self):
        inherited = {'CUA_DRIVER_PERMISSION_MODE': 'unrestricted',
                     'CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS': '1',
                     'CUA_DRIVER_SESSION_POLICY_FILE': 'old-manifest',
                     'CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED': '1',
                     'CUA_DRIVER_MANAGED_POLICY_FILE': 'managed.yaml'}
        env = profile_environment({'mode': 'standard'}, inherited)
        self.assertEqual(env, {'CUA_DRIVER_PERMISSION_MODE': 'standard',
                               'CUA_DRIVER_MANAGED_POLICY_FILE': 'managed.yaml'})

    def test_profile_matrix_uses_normal_environment_contract(self):
        with tempfile.NamedTemporaryFile() as manifest:
            for mode in ('standard', 'bounded', 'unrestricted'):
                profile = {'mode': mode, 'manifest': manifest.name, 'approve_manifest': True,
                           'acknowledge_unrestricted': True}
                env = profile_environment(profile, {})
                self.assertEqual(env['CUA_DRIVER_PERMISSION_MODE'], mode)
                self.assertEqual(env['CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED'], '1')
            for mode in ('standard', 'unrestricted'):
                profile_environment({'mode': mode, 'acknowledge_unrestricted': True}, {})
        for profile in ({'mode': 'bounded'}, {'mode': 'unrestricted'},
                        {'mode': 'standard', 'manifest': 'unreviewed.yaml'}):
            with self.assertRaises(AssertionError):
                profile_environment(profile, {})

    def test_distinct_labels_are_not_process_ownership(self):
        def client(pid):
            return SimpleNamespace(process=Mock(pid=pid, poll=Mock(return_value=None)))
        self.assertEqual(assert_distinct_runtimes([client(1), client(2)]), [1, 2])
        with self.assertRaises(AssertionError):
            assert_distinct_runtimes([client(1), client(1)])

    def test_owned_process_is_reaped_after_terminate_timeout(self):
        process = Mock()
        process.poll.return_value = None
        process.wait.side_effect = [subprocess.TimeoutExpired('child', 5),
                                    subprocess.TimeoutExpired('child', 5), 0]
        stop_process(process)
        process.stdin.close.assert_called_once()
        process.terminate.assert_called_once()
        process.kill.assert_called_once()
        self.assertEqual(process.wait.call_count, 3)

    def test_initialization_failure_closes_owned_process(self):
        process = Mock()
        process.poll.return_value = 0
        with tempfile.TemporaryDirectory() as directory, \
                patch('production_mcp.subprocess.Popen', return_value=process) as spawn, \
                patch('driver_input_live.MCP.rpc', side_effect=TimeoutError('unknown')):
            with self.assertRaises(TimeoutError):
                DirectMCP(Path('/synthetic/driver'), Path(directory), {'mode': 'standard'})
            self.assertEqual(spawn.call_args.args[0], ['/synthetic/driver', 'mcp', '--direct'])
            process.wait.assert_called_once()

    def test_timeout_poison_prevents_replay(self):
        mcp = DirectMCP.__new__(DirectMCP)
        mcp.failed = mcp.closed = False
        with patch('driver_input_live.MCP.rpc', side_effect=TimeoutError('unknown')) as rpc:
            with self.assertRaises(TimeoutError):
                mcp.rpc('tools/call', {})
            with self.assertRaisesRegex(RuntimeError, 'do not replay'):
                mcp.rpc('tools/call', {})
            rpc.assert_called_once()

    def test_unrestricted_direct_launch_uses_environment_not_serve_only_flags(self):
        process = Mock()
        process.poll.return_value = 0
        with tempfile.TemporaryDirectory() as directory, \
                patch('production_mcp.subprocess.Popen', return_value=process) as spawn, \
                patch('driver_input_live.MCP.rpc', return_value={}):
            mcp = DirectMCP(Path('/synthetic/driver'), Path(directory),
                            {'mode': 'unrestricted', 'acknowledge_unrestricted': True})
            mcp.close()
            self.assertEqual(spawn.call_args.args[0], [
                '/synthetic/driver', 'mcp', '--direct'])
            self.assertEqual(spawn.call_args.kwargs['env']['CUA_DRIVER_PERMISSION_MODE'], 'unrestricted')
            self.assertEqual(spawn.call_args.kwargs['env']['CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS'], '1')


if __name__ == '__main__':
    unittest.main()
