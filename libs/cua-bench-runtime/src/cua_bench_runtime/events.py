"""Append-only, hash-chained trial events."""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, Self

from cua_bench_runtime.canon import canonical_json, sha256_bytes
from cua_bench_runtime.clock import Clock
from cua_bench_runtime.errors import ValidationFailure

GENESIS = "0" * 64


class EventLog:
    def __init__(self, path: Path, clock: Clock) -> None:
        self.path = path
        self.clock = clock
        self.seq = 0
        self.previous = GENESIS
        self._handle = path.open("xb")

    def append(self, event_type: str, data: dict[str, Any], *, sync: bool = False) -> str:
        body = {
            "seq": self.seq,
            "ts_utc": self.clock.utc(),
            "elapsed_ms": self.clock.elapsed_ms(),
            "type": event_type,
            "prev": self.previous,
            "data": data,
        }
        event_hash = sha256_bytes(canonical_json(body), prefix=False)
        event = {**body, "hash": event_hash}
        self._handle.write(canonical_json(event) + b"\n")
        self._handle.flush()
        if sync:
            os.fsync(self._handle.fileno())
        self.previous = event_hash
        self.seq += 1
        return event_hash

    def close(self) -> None:
        if not self._handle.closed:
            self._handle.flush()
            os.fsync(self._handle.fileno())
            self._handle.close()

    @property
    def closed(self) -> bool:
        return self._handle.closed

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()


def verify_event_log(path: Path) -> list[dict[str, Any]]:
    raw = path.read_bytes()
    if raw and not raw.endswith(b"\n"):
        raise ValidationFailure("event log has a truncated final record")

    events: list[dict[str, Any]] = []
    previous = GENESIS
    for expected_seq, line in enumerate(raw.splitlines()):
        try:
            event = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ValidationFailure(f"event {expected_seq} is invalid JSON: {error}") from error
        if canonical_json(event) != line:
            raise ValidationFailure(f"event {expected_seq} is not canonical JSON")
        if event.get("seq") != expected_seq:
            raise ValidationFailure(f"event sequence mismatch at {expected_seq}")
        if event.get("prev") != previous:
            raise ValidationFailure(f"event chain mismatch at {expected_seq}")
        body = {key: value for key, value in event.items() if key != "hash"}
        actual = sha256_bytes(canonical_json(body), prefix=False)
        if event.get("hash") != actual:
            raise ValidationFailure(f"event hash mismatch at {expected_seq}")
        previous = actual
        events.append(event)
    return events
