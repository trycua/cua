from contextlib import ExitStack
from pathlib import Path
import signal
import unittest
from unittest.mock import Mock, patch

from compositor_stall import stall_past_expiry


class CompositorStallTest(unittest.TestCase):
    def test_invalid_deadline_never_touches_a_process(self):
        with patch('compositor_stall.time.time', return_value=10), \
             patch('compositor_stall.os.pidfd_open', create=True) as opened:
            for expiry in (9000, 10000, 14000):
                with self.assertRaises(ValueError):
                    stall_past_expiry(7, expiry)
            opened.assert_not_called()

    def test_resume_runs_even_when_fault_wait_fails(self):
        calls = []
        watchdog = Mock()
        watchdog.stdout.readline.return_value = 'ARMED\n'
        watchdog.poll.return_value = None
        with ExitStack() as stack:
            stack.enter_context(patch('compositor_stall.time.time', return_value=10))
            stack.enter_context(patch('compositor_stall.time.sleep', side_effect=RuntimeError('fault wait')))
            stack.enter_context(patch('compositor_stall.Path.resolve', return_value=Path('/usr/bin/Hyprland')))
            stack.enter_context(patch('compositor_stall.Path.stat', return_value=Mock(st_uid=1000)))
            stack.enter_context(patch('compositor_stall.os.getuid', return_value=1000))
            opened = stack.enter_context(patch('compositor_stall.os.pidfd_open', return_value=9, create=True))
            closed = stack.enter_context(patch('compositor_stall.os.close'))
            stack.enter_context(patch('compositor_stall.signal.pidfd_send_signal',
                                      side_effect=lambda fd, sig: calls.append((fd, sig)), create=True))
            spawned = stack.enter_context(patch('compositor_stall.subprocess.Popen', return_value=watchdog))
            stack.enter_context(patch('compositor_stall.select.select', return_value=([watchdog.stdout], [], [])))
            with self.assertRaisesRegex(RuntimeError, 'fault wait'):
                stall_past_expiry(7, 11000)
            opened.assert_called_once_with(7)
            self.assertEqual(calls, [(9, signal.SIGSTOP), (9, signal.SIGCONT), (9, signal.SIGCONT)])
            self.assertEqual(spawned.call_args.kwargs['pass_fds'], (9,))
            watchdog.terminate.assert_called_once()
            watchdog.wait.assert_called_once_with(timeout=5)
            closed.assert_called_once_with(9)


if __name__ == '__main__':
    unittest.main()
