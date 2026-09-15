"""Minimal signal capture for graceful trial cleanup."""

from __future__ import annotations

import signal
from types import FrameType
from typing import Any, Self

from cua_bench_runtime.errors import HardAbort, TrialInterrupted


class InterruptFlag:
    def __init__(self) -> None:
        self.count = 0
        self._previous: dict[int, Any] = {}

    def _handle(self, _signum: int, _frame: FrameType | None) -> None:
        self.count += 1

    def __enter__(self) -> Self:
        names = ["SIGINT", "SIGTERM"]
        if hasattr(signal, "SIGBREAK"):
            names.append("SIGBREAK")
        for name in names:
            signum = getattr(signal, name)
            self._previous[signum] = signal.getsignal(signum)
            signal.signal(signum, self._handle)
        return self

    def __exit__(self, *_: object) -> None:
        for signum, handler in self._previous.items():
            signal.signal(signum, handler)

    def raise_if_requested(self) -> None:
        if self.count > 1:
            raise HardAbort("received a second interrupt")
        if self.count == 1:
            raise TrialInterrupted("trial interrupted")
