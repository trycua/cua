"""Canonical JSON and byte digests."""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any

import rfc8785


def canonical_json(value: Any) -> bytes:
    return rfc8785.dumps(value)


def sha256_bytes(value: bytes, *, prefix: bool = True) -> str:
    digest = hashlib.sha256(value).hexdigest()
    return f"sha256:{digest}" if prefix else digest


def digest_json(value: Any) -> str:
    return sha256_bytes(canonical_json(value))


def digest_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())
