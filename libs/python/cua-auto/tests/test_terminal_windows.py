"""Windows-specific PTY tests for cua_auto.terminal.

Uses pywinpty under the hood via the Terminal._create_windows path.
All tests are skipped on non-Windows platforms.
"""

from __future__ import annotations

import sys
import time

import pytest

pytestmark = pytest.mark.skipif(
    sys.platform != "win32",
    reason="Windows PTY tests — requires pywinpty",
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _run_command(cmd: str, timeout: float = 10.0) -> tuple[bytes, int]:
    """Spawn *cmd* via PowerShell in a PTY, collect output, return (output, exit_code)."""
    from cua_auto.terminal import Terminal

    chunks: list[bytes] = []
    t = Terminal()
    # Wrap in powershell -Command so we can use PS syntax
    full_cmd = f'powershell -NoProfile -NonInteractive -Command "{cmd}"'
    session = t.create(command=full_cmd, cols=80, rows=24, on_data=chunks.append)
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
        output, code = _run_command("Write-Output hello")
        assert b"hello" in output
        assert code == 0

    def test_echo_with_spaces(self):
        output, code = _run_command("Write-Output 'hello world'")
        assert b"hello world" in output
        assert code == 0

    def test_echo_numbers(self):
        output, code = _run_command("Write-Output 42")
        assert b"42" in output
        assert code == 0

    def test_echo_multiple_words(self):
        output, code = _run_command("Write-Output 'foo bar baz'")
        assert b"foo" in output
        assert b"bar" in output
        assert b"baz" in output
        assert code == 0


# ---------------------------------------------------------------------------
# Exit codes
# ---------------------------------------------------------------------------


class TestExitCodes:
    def test_exit_zero(self):
        _, code = _run_command("exit 0")
        assert code == 0

    def test_exit_nonzero(self):
        # pywinpty/ConPTY does not propagate PowerShell's exit code reliably;
        # cmd.exe /c exit N does propagate correctly.
        from cua_auto.terminal import Terminal

        t = Terminal()
        session = t.create(
            command="cmd /c exit 1", cols=80, rows=24, on_data=lambda _: None
        )
        code = t.wait(session.pid, timeout=5.0)
        assert code == 1

    def test_true_command(self):
        _, code = _run_command("$true | Out-Null")
        assert code == 0

    def test_false_exits_nonzero(self):
        # pywinpty/ConPTY does not propagate PowerShell's exit code reliably;
        # cmd.exe /c exit N does propagate correctly.
        from cua_auto.terminal import Terminal

        t = Terminal()
        session = t.create(
            command="cmd /c exit 42", cols=80, rows=24, on_data=lambda _: None
        )
        code = t.wait(session.pid, timeout=5.0)
        assert code == 42


# ---------------------------------------------------------------------------
# Interactive stdin (send_stdin)
# ---------------------------------------------------------------------------


class TestSendStdin:
    def test_send_echo_via_stdin(self):
        from cua_auto.terminal import Terminal

        chunks: list[bytes] = []
        t = Terminal()
        session = t.create(
            command="powershell -NoProfile -NonInteractive",
            cols=80,
            rows=24,
            on_data=chunks.append,
        )

        time.sleep(0.5)
        t.send_stdin(session.pid, b"Write-Output hello_from_stdin\r\n")
        t.send_stdin(session.pid, b"exit\r\n")
        t.wait(session.pid, timeout=10.0)

        output = b"".join(chunks)
        assert b"hello_from_stdin" in output

    def test_send_multiple_commands(self):
        from cua_auto.terminal import Terminal

        chunks: list[bytes] = []
        t = Terminal()
        session = t.create(
            command="powershell -NoProfile -NonInteractive",
            cols=80,
            rows=24,
            on_data=chunks.append,
        )

        time.sleep(0.5)
        t.send_stdin(session.pid, b"Write-Output first\r\n")
        t.send_stdin(session.pid, b"Write-Output second\r\n")
        t.send_stdin(session.pid, b"exit\r\n")
        t.wait(session.pid, timeout=10.0)

        output = b"".join(chunks)
        assert b"first" in output
        assert b"second" in output


# ---------------------------------------------------------------------------
# Kill
# ---------------------------------------------------------------------------


class TestKill:
    def test_kill_running_session(self):
        from cua_auto.terminal import Terminal

        t = Terminal()
        session = t.create(
            command="powershell -NoProfile -NonInteractive -Command Start-Sleep 60",
            cols=80,
            rows=24,
            on_data=lambda _: None,
        )

        time.sleep(0.3)
        killed = t.kill(session.pid)
        assert killed is True

        code = t.wait(session.pid, timeout=5.0)
        assert code is not None

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
        session = t.create(
            command="powershell -NoProfile -NonInteractive",
            cols=80,
            rows=24,
            on_data=lambda _: None,
        )

        time.sleep(0.3)
        t.resize(session.pid, 120, 40)
        t.resize(session.pid, 80, 24)

        t.send_stdin(session.pid, b"exit\r\n")
        t.wait(session.pid, timeout=5.0)

    def test_resize_unknown_pid_is_noop(self):
        from cua_auto.terminal import Terminal

        t = Terminal()
        t.resize(9999999, 80, 24)  # should not raise


# ---------------------------------------------------------------------------
# Connect (callback replacement)
# ---------------------------------------------------------------------------


class TestConnect:
    def test_connect_replaces_callback(self):
        from cua_auto.terminal import Terminal

        first_chunks: list[bytes] = []
        second_chunks: list[bytes] = []

        t = Terminal()
        session = t.create(
            command="powershell -NoProfile -NonInteractive",
            cols=80,
            rows=24,
            on_data=first_chunks.append,
        )

        time.sleep(0.5)
        t.send_stdin(session.pid, b"Write-Output before_connect\r\n")
        time.sleep(0.3)

        t.connect(session.pid, second_chunks.append)

        t.send_stdin(session.pid, b"Write-Output after_connect\r\n")
        t.send_stdin(session.pid, b"exit\r\n")
        t.wait(session.pid, timeout=10.0)

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
            command='powershell -NoProfile -NonInteractive -Command "Write-Output singleton_works"',
            cols=80,
            rows=24,
            on_data=chunks.append,
        )
        terminal.wait(session.pid, timeout=10.0)
        assert b"singleton_works" in b"".join(chunks)
