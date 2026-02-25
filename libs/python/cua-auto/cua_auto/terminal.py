"""Cross-platform PTY (pseudo-terminal) manager.

Uses the ``pty`` stdlib on Unix/macOS and ``pywinpty`` on Windows.

Usage::

    from cua_auto.terminal import terminal

    output = []
    session = terminal.create("bash", on_data=lambda d: output.append(d))
    terminal.send_stdin(session.pid, b"echo hello\\n")
    terminal.send_stdin(session.pid, b"exit\\n")
    terminal.wait(session.pid)
    print(b"".join(output))
"""

from __future__ import annotations

import os
import sys
import threading
from dataclasses import dataclass, field
from typing import Callable, Dict, List, Optional


@dataclass
class PtySession:
    """Lightweight handle returned from :meth:`Terminal.create`."""

    pid: int
    cols: int
    rows: int


class _PtyProcess:
    """Internal state holder for one PTY session."""

    def __init__(self) -> None:
        self.process = None  # subprocess.Popen (Unix) or None (Windows)
        self.master_fd: Optional[int] = None  # Unix master side of the PTY
        self.winpty_pty = None  # winpty.PtyProcess (Windows)
        self.on_data: List[Callable[[bytes], None]] = []
        self.reader_thread: Optional[threading.Thread] = None
        self._exit_event: threading.Event = threading.Event()
        self.exit_code: Optional[int] = None


class Terminal:
    """Cross-platform PTY manager.

    Each session is keyed by its process PID.  All public methods are
    thread-safe.
    """

    def __init__(self) -> None:
        self._sessions: Dict[int, _PtyProcess] = {}
        self._lock = threading.Lock()

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def create(
        self,
        command: Optional[str] = None,
        cols: int = 80,
        rows: int = 24,
        on_data: Optional[Callable[[bytes], None]] = None,
        cwd: Optional[str] = None,
        envs: Optional[Dict[str, str]] = None,
    ) -> PtySession:
        """Spawn a new PTY session.

        Args:
            command: Shell command to run.  Defaults to ``bash`` on Unix and
                ``powershell`` on Windows.
            cols: Initial terminal width (columns).
            rows: Initial terminal height (rows).
            on_data: Callback invoked from the reader thread with raw bytes
                whenever the process writes output.
            cwd: Working directory for the spawned process.
            envs: Additional environment variables (merged into ``os.environ``).

        Returns:
            :class:`PtySession` with the PID, cols, and rows.
        """
        if sys.platform == "win32":
            return self._create_windows(command, cols, rows, on_data, cwd, envs)
        return self._create_unix(command, cols, rows, on_data, cwd, envs)

    def send_stdin(self, pid: int, data: bytes) -> None:
        """Write *data* to the stdin of session *pid*."""
        with self._lock:
            holder = self._sessions.get(pid)
        if holder is None:
            raise KeyError(f"No PTY session with pid {pid}")
        if holder.master_fd is not None:
            os.write(holder.master_fd, data)
        elif holder.winpty_pty is not None:
            holder.winpty_pty.write(data.decode("utf-8", errors="replace"))

    def resize(self, pid: int, cols: int, rows: int) -> None:
        """Resize the terminal window for session *pid*."""
        with self._lock:
            holder = self._sessions.get(pid)
        if holder is None:
            return
        if holder.master_fd is not None:
            import fcntl
            import struct
            import termios

            fcntl.ioctl(
                holder.master_fd,
                termios.TIOCSWINSZ,
                struct.pack("HHHH", rows, cols, 0, 0),
            )
        elif holder.winpty_pty is not None:
            holder.winpty_pty.setwinsize(rows, cols)

    def kill(self, pid: int) -> bool:
        """Kill the process for session *pid*.

        Returns:
            ``True`` if the signal was delivered successfully.
        """
        with self._lock:
            holder = self._sessions.get(pid)
        if holder is None:
            return False
        if holder.process is not None:
            try:
                holder.process.kill()
                return True
            except Exception:
                return False
        if holder.winpty_pty is not None:
            try:
                holder.winpty_pty.terminate()
                return True
            except Exception:
                return False
        return False

    def wait(self, pid: int, timeout: Optional[float] = None) -> Optional[int]:
        """Block until session *pid* exits and return its exit code.

        Args:
            pid: Session PID.
            timeout: Maximum seconds to wait.  ``None`` means wait forever.

        Returns:
            Exit code, or ``None`` if the timeout expired or pid is unknown.
        """
        with self._lock:
            holder = self._sessions.get(pid)
        if holder is None:
            return None
        holder._exit_event.wait(timeout=timeout)
        return holder.exit_code

    def connect(
        self,
        pid: int,
        on_data: Callable[[bytes], None],
    ) -> PtySession:
        """Attach a new *on_data* callback to an existing session.

        The previous callbacks are replaced.  Useful for reconnecting an SSE
        or WebSocket consumer to an already-running session.

        Returns:
            :class:`PtySession` for the existing session (cols/rows default to
            80×24 since we don't track them after creation).
        """
        with self._lock:
            holder = self._sessions.get(pid)
        if holder is None:
            raise KeyError(f"No PTY session with pid {pid}")
        with self._lock:
            holder.on_data = [on_data]
        return PtySession(pid=pid, cols=80, rows=24)

    # ------------------------------------------------------------------
    # Platform-specific internals
    # ------------------------------------------------------------------

    def _create_unix(
        self,
        command: Optional[str],
        cols: int,
        rows: int,
        on_data: Optional[Callable[[bytes], None]],
        cwd: Optional[str],
        envs: Optional[Dict[str, str]],
    ) -> PtySession:
        import fcntl
        import struct
        import subprocess
        import termios
        import pty as _pty

        cmd_str = command or "bash"
        if isinstance(cmd_str, str):
            cmd = ["/bin/sh", "-c", cmd_str]
        else:
            cmd = cmd_str

        master_fd, slave_fd = _pty.openpty()

        # Set initial terminal size
        fcntl.ioctl(slave_fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

        env = os.environ.copy()
        if envs:
            env.update(envs)
        env.setdefault("TERM", "xterm-256color")

        proc = subprocess.Popen(
            cmd,
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
            close_fds=True,
            cwd=cwd,
            env=env,
            start_new_session=True,
        )
        os.close(slave_fd)

        holder = _PtyProcess()
        holder.process = proc
        holder.master_fd = master_fd
        if on_data is not None:
            holder.on_data.append(on_data)

        pid = proc.pid
        with self._lock:
            self._sessions[pid] = holder

        def _reader() -> None:
            try:
                while True:
                    try:
                        data = os.read(master_fd, 4096)
                    except OSError:
                        break
                    if not data:
                        break
                    with self._lock:
                        callbacks = list(holder.on_data)
                    for cb in callbacks:
                        try:
                            cb(data)
                        except Exception:
                            pass
            finally:
                try:
                    os.close(master_fd)
                except OSError:
                    pass
                with self._lock:
                    holder.master_fd = None
                holder.exit_code = proc.wait()
                holder._exit_event.set()

        holder.reader_thread = threading.Thread(target=_reader, daemon=True, name=f"pty-reader-{pid}")
        holder.reader_thread.start()

        return PtySession(pid=pid, cols=cols, rows=rows)

    def _create_windows(
        self,
        command: Optional[str],
        cols: int,
        rows: int,
        on_data: Optional[Callable[[bytes], None]],
        cwd: Optional[str],
        envs: Optional[Dict[str, str]],
    ) -> PtySession:
        try:
            import winpty  # type: ignore[import]
        except ImportError as exc:
            raise ImportError(
                "pywinpty is required for PTY on Windows. "
                "Install with: pip install pywinpty"
            ) from exc

        cmd = command or "powershell"

        env = None
        if envs:
            env = os.environ.copy()
            env.update(envs)

        pty_proc = winpty.PtyProcess.spawn(
            cmd,
            cwd=cwd,
            env=env,
            dimensions=(rows, cols),
        )

        holder = _PtyProcess()
        holder.winpty_pty = pty_proc
        if on_data is not None:
            holder.on_data.append(on_data)

        pid: int = pty_proc.pid
        with self._lock:
            self._sessions[pid] = holder

        def _reader() -> None:
            try:
                while pty_proc.isalive():
                    try:
                        data = pty_proc.read(4096)
                        if data:
                            raw: bytes = (
                                data.encode("utf-8", errors="replace")
                                if isinstance(data, str)
                                else data
                            )
                            with self._lock:
                                callbacks = list(holder.on_data)
                            for cb in callbacks:
                                try:
                                    cb(raw)
                                except Exception:
                                    pass
                    except Exception:
                        break
            finally:
                try:
                    holder.exit_code = pty_proc.wait()
                except Exception:
                    holder.exit_code = -1
                holder._exit_event.set()

        holder.reader_thread = threading.Thread(target=_reader, daemon=True, name=f"pty-reader-{pid}")
        holder.reader_thread.start()

        return PtySession(pid=pid, cols=cols, rows=rows)


# Module-level singleton for convenience
terminal = Terminal()
