"""Bounded fault injection for an explicitly selected disposable compositor.

This intentionally freezes the desktop. It is not an uninterrupted-user test.
An independent PID-fd watchdog resumes the exact process even if the caller dies.
Never run against a user's working desktop.
"""
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import time


def stall_past_expiry(pid, expires_unix_ms):
    remaining = expires_unix_ms / 1000 - time.time()
    if not 0 < remaining < 2.9:
        raise ValueError('expiry must be within the bounded compositor stall')
    expected = Path('/usr/bin/Hyprland').resolve(strict=True)
    if Path(f'/proc/{pid}/exe').resolve(strict=True) != expected:
        raise ValueError('fault target is not the installed Hyprland executable')
    if Path(f'/proc/{pid}').stat().st_uid != os.getuid():
        raise ValueError('fault target is not owned by the test user')
    fd = os.pidfd_open(pid)
    watchdog = None
    try:
        watchdog = subprocess.Popen([
            sys.executable, '-c',
            'import signal,sys,time; print("ARMED",flush=True); time.sleep(4); '
            'signal.pidfd_send_signal(int(sys.argv[1]),signal.SIGCONT)', str(fd)],
            pass_fds=(fd,), stdout=subprocess.PIPE, text=True)
        if not select.select([watchdog.stdout], [], [], 2)[0] or watchdog.stdout.readline().strip() != 'ARMED':
            raise RuntimeError('compositor resume watchdog did not arm')
        started = time.monotonic_ns()
        signal.pidfd_send_signal(fd, signal.SIGSTOP)
        try:
            # Use one monotonic deadline; clock adjustments cannot extend the stall.
            deadline = time.monotonic() + max(0, expires_unix_ms / 1000 - time.time()) + 0.05
            time.sleep(max(0, deadline - time.monotonic()))
        finally:
            resumed = time.monotonic_ns()
            signal.pidfd_send_signal(fd, signal.SIGCONT)
        return {'resume_ns': resumed, 'stall_ms': (resumed - started) / 1_000_000}
    finally:
        # Also covers exceptions after SIGSTOP and before entering its try block.
        signal.pidfd_send_signal(fd, signal.SIGCONT)
        if watchdog is not None:
            if watchdog.poll() is None:
                watchdog.terminate()
            watchdog.wait(timeout=5)
            watchdog.stdout.close()
        os.close(fd)
