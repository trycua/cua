"""Injectable runtime clock."""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from datetime import UTC, datetime


@dataclass
class Clock:
    started: float = field(default_factory=time.monotonic)

    def elapsed_ms(self) -> int:
        return int((time.monotonic() - self.started) * 1000)

    def utc(self) -> str:
        return datetime.now(UTC).isoformat().replace("+00:00", "Z")
