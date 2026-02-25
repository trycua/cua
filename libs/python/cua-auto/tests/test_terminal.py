"""Unit tests for cua_auto.terminal — cross-platform PTY engine.

Covers Unix PTY paths (echo, spaces, stdin interaction, exit codes, kill, resize).
Windows-only paths are skipped when not running on win32 (and vice-versa).
"""

from __future__ import annotations

import sys
import time

import pytest

# All tests in this module require a real PTY, so skip on platforms where we
# can't spawn one without the optional dependency.
pytestmark = pytest.mark.skipif(
    sys.platform == "win32",
    reason="Unix PTY tests — win32 uses pywinpty (tested separately)",
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _run_command(cmd: str, timeout: float = 3.0) -> tuple[bytes, int]:
    """Spawn *cmd* in a PTY, collect all output, return (output, exit_code)."""
    from cua_auto.terminal import Terminal

    chunks: list[bytes] = []
    t = Terminal()
    session = t.create(command=cmd, cols=80, rows=24, on_data=chunks.append)
    exit_code = t.wait(session.pid, timeout=timeout)
    return b"".join(chunks), exit_code or 0


# ---------------------------------------------------------------------------
# Import / singleton
# ---------------------------------------------------------------------------


class TestTerminalImport:
    def test_module_importable(self):
        import cua_auto.terminal  # noqa: F401

    def test_singleton_exists(self):
        from cua_auto.terminal import Terminal, terminal

        assert isinstance(terminal, Terminal)

    def test_pty_session_dataclass(self):
        from cua_auto.terminal import PtySession

        s = PtySession(pid=1234, cols=80, rows=24)
        assert s.pid == 1234
        assert s.cols == 80
        assert s.rows == 24


# ---------------------------------------------------------------------------
# Basic echo
# ---------------------------------------------------------------------------


class TestEchoBasic:
    def test_echo_hello(self):
        output, code = _run_command("echo hello")
        assert b"hello" in output
        assert code == 0

    def test_echo_with_spaces(self):
        output, code = _run_command("echo hello world")
        assert b"hello world" in output
        assert code == 0

    def test_echo_leading_trailing_spaces(self):
        output, code = _run_command("echo '  spaced  '")
        assert b"spaced" in output
        assert code == 0

    def test_echo_multiple_words(self):
        output, code = _run_command("echo foo bar baz")
        assert b"foo" in output
        assert b"bar" in output
        assert b"baz" in output
        assert code == 0

    def test_echo_empty_string(self):
        # echo with an empty-ish arg should still exit 0
        output, code = _run_command("echo ''")
        assert code == 0

    def test_echo_numbers(self):
        output, code = _run_command("echo 42")
        assert b"42" in output
        assert code == 0

    def test_echo_special_chars(self):
        output, code = _run_command("echo 'hello-world_test'")
        assert b"hello-world_test" in output
        assert code == 0


# ---------------------------------------------------------------------------
# Exit codes
# ---------------------------------------------------------------------------


class TestExitCodes:
    def test_exit_zero(self):
        _, code = _run_command("exit 0")
        assert code == 0

    def test_exit_nonzero(self):
        _, code = _run_command("exit 1")
        assert code == 1

    def test_exit_42(self):
        _, code = _run_command("exit 42")
        assert code == 42

    def test_true_command(self):
        _, code = _run_command("true")
        assert code == 0

    def test_false_command(self):
        _, code = _run_command("false")
        assert code != 0


# ---------------------------------------------------------------------------
# Interactive stdin (send_stdin)
# ---------------------------------------------------------------------------


class TestSendStdin:
    def test_send_echo_via_stdin(self):
        from cua_auto.terminal import Terminal

        chunks: list[bytes] = []
        t = Terminal()
        session = t.create(command="bash", cols=80, rows=24, on_data=chunks.append)

        # Give bash a moment to start
        time.sleep(0.2)
        t.send_stdin(session.pid, b"echo hello_from_stdin\n")
        t.send_stdin(session.pid, b"exit\n")
        t.wait(session.pid, timeout=3.0)

        output = b"".join(chunks)
        assert b"hello_from_stdin" in output

    def test_send_echo_with_spaces_via_stdin(self):
        from cua_auto.terminal import Terminal

        chunks: list[bytes] = []
        t = Terminal()
        session = t.create(command="bash", cols=80, rows=24, on_data=chunks.append)

        time.sleep(0.2)
        t.send_stdin(session.pid, b"echo 'hello world from stdin'\n")
        t.send_stdin(session.pid, b"exit\n")
        t.wait(session.pid, timeout=3.0)

        output = b"".join(chunks)
        assert b"hello world from stdin" in output

    def test_send_multiple_commands(self):
        from cua_auto.terminal import Terminal

        chunks: list[bytes] = []
        t = Terminal()
        session = t.create(command="bash", cols=80, rows=24, on_data=chunks.append)

        time.sleep(0.2)
        t.send_stdin(session.pid, b"echo first\n")
        t.send_stdin(session.pid, b"echo second\n")
        t.send_stdin(session.pid, b"exit\n")
        t.wait(session.pid, timeout=3.0)

        output = b"".join(chunks)
        assert b"first" in output
        assert b"second" in output

    def test_exit_code_via_stdin(self):
        from cua_auto.terminal import Terminal

        t = Terminal()
        session = t.create(command="bash", cols=80, rows=24, on_data=lambda _: None)

        time.sleep(0.2)
        t.send_stdin(session.pid, b"exit 7\n")
        code = t.wait(session.pid, timeout=3.0)
        assert code == 7


# ---------------------------------------------------------------------------
# Kill
# ---------------------------------------------------------------------------


class TestKill:
    def test_kill_running_session(self):
        from cua_auto.terminal import Terminal

        t = Terminal()
        # sleep for a long time — we'll kill it
        session = t.create(command="sleep 60", cols=80, rows=24, on_data=lambda _: None)

        killed = t.kill(session.pid)
        assert killed is True

        # After kill, wait should return quickly
        code = t.wait(session.pid, timeout=2.0)
        assert code is not None  # process has exited

    def test_kill_unknown_pid_returns_false(self):
        from cua_auto.terminal import Terminal

        t = Terminal()
        assert t.kill(9999999) is False

    def test_wait_unknown_pid_returns_none(self):
        from cua_auto.terminal import Terminal

        t = Terminal()
        assert t.wait(9999999, timeout=0.1) is None


# ---------------------------------------------------------------------------
# Resize
# ---------------------------------------------------------------------------


class TestResize:
    def test_resize_does_not_raise(self):
        from cua_auto.terminal import Terminal

        t = Terminal()
        session = t.create(command="bash", cols=80, rows=24, on_data=lambda _: None)

        # Resize should not raise
        t.resize(session.pid, 120, 40)
        t.resize(session.pid, 80, 24)

        t.send_stdin(session.pid, b"exit\n")
        t.wait(session.pid, timeout=2.0)

    def test_resize_unknown_pid_is_noop(self):
        from cua_auto.terminal import Terminal

        t = Terminal()
        # Should not raise even for unknown pids
        t.resize(9999999, 80, 24)


# ---------------------------------------------------------------------------
# Connect (callback replacement)
# ---------------------------------------------------------------------------


class TestConnect:
    def test_connect_replaces_callback(self):
        from cua_auto.terminal import Terminal

        first_chunks: list[bytes] = []
        second_chunks: list[bytes] = []

        t = Terminal()
        session = t.create(command="bash", cols=80, rows=24, on_data=first_chunks.append)

        time.sleep(0.2)
        t.send_stdin(session.pid, b"echo before_connect\n")
        time.sleep(0.1)

        # Re-attach with a new callback
        t.connect(session.pid, second_chunks.append)

        t.send_stdin(session.pid, b"echo after_connect\n")
        t.send_stdin(session.pid, b"exit\n")
        t.wait(session.pid, timeout=3.0)

        # "after_connect" should appear in the second callback's data
        assert b"after_connect" in b"".join(second_chunks)

    def test_connect_unknown_pid_raises(self):
        from cua_auto.terminal import Terminal

        t = Terminal()
        with pytest.raises(KeyError):
            t.connect(9999999, lambda _: None)


# ---------------------------------------------------------------------------
# Singleton convenience
# ---------------------------------------------------------------------------


class TestSingletonTerminal:
    def test_singleton_echo(self):
        from cua_auto.terminal import terminal

        chunks: list[bytes] = []
        session = terminal.create(
            command="echo singleton_works",
            cols=80,
            rows=24,
            on_data=chunks.append,
        )
        terminal.wait(session.pid, timeout=3.0)
        assert b"singleton_works" in b"".join(chunks)
