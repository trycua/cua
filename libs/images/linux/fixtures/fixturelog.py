"""Shared JSONL event log for the linux conformance fixtures.

Every fixture appends one JSON object per line to
``$CUA_FIXTURE_LOG_DIR/<name>.jsonl`` (default ``/tmp/cua-fixtures``). Each
record carries ``ts`` (unix seconds, float), ``mono`` (monotonic seconds),
``fixture`` and ``type``; the rest is event specific. Lines are flushed
immediately so a test can tail the file while it drives input.
"""

from __future__ import annotations

import json
import os
import time
from typing import Any


class FixtureLog:
    def __init__(self, name: str) -> None:
        self.name = name
        log_dir = os.environ.get("CUA_FIXTURE_LOG_DIR", "/tmp/cua-fixtures")
        os.makedirs(log_dir, exist_ok=True)
        self.path = os.path.join(log_dir, f"{name}.jsonl")
        self._fh = open(self.path, "a", encoding="utf-8", buffering=1)

    def emit(self, type_: str, **fields: Any) -> None:
        record = {
            "ts": round(time.time(), 6),
            "mono": round(time.monotonic(), 6),
            "fixture": self.name,
            "type": type_,
        }
        record.update(fields)
        self._fh.write(json.dumps(record, sort_keys=True) + "\n")
        self._fh.flush()
