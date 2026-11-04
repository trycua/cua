"""Per-task log files for tasks that run concurrently in one process.

Task code, agents and the episode all ``print``. While a batch runs,
``sys.stdout``/``sys.stderr`` are replaced by routers that send each write to
the log file of the task whose asyncio context made it (a ``ContextVar``;
asyncio copies the context into every task). Writes outside any task go to
the real console.
"""

from __future__ import annotations

import io
import sys
from contextlib import contextmanager
from contextvars import ContextVar
from pathlib import Path
from typing import Iterator, Optional, TextIO

_current: ContextVar[Optional[TextIO]] = ContextVar("cua_bench_task_log", default=None)


class _Router(io.TextIOBase):
    def __init__(self, fallback: TextIO) -> None:
        self._fallback = fallback

    def _target(self) -> TextIO:
        return _current.get() or self._fallback

    def write(self, s: str) -> int:  # type: ignore[override]
        self._target().write(s)
        return len(s)

    def flush(self) -> None:
        self._target().flush()

    def isatty(self) -> bool:
        target = self._target()
        return target is self._fallback and self._fallback.isatty()

    @property
    def encoding(self) -> str:  # type: ignore[override]
        return getattr(self._fallback, "encoding", "utf-8")

    def fileno(self) -> int:
        return self._fallback.fileno()


@contextmanager
def routed_stdio() -> Iterator[TextIO]:
    """Install the routers; yields the real console stdout."""
    real_out, real_err = sys.stdout, sys.stderr
    if isinstance(real_out, _Router):  # nested: already routed
        yield real_out._fallback
        return
    sys.stdout, sys.stderr = _Router(real_out), _Router(real_err)
    try:
        yield real_out
    finally:
        sys.stdout, sys.stderr = real_out, real_err


@contextmanager
def task_log(path: Path) -> Iterator[TextIO]:
    """Send this context's output to ``path`` (appending, line buffered)."""
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = open(path, "a", encoding="utf-8", buffering=1)
    token = _current.set(handle)
    try:
        yield handle
    finally:
        _current.reset(token)
        handle.close()
