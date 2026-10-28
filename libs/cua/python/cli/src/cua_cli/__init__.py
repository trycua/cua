"""The Rust `cua` CLI for the `cua` Python package (run through its `cua`
console command)."""

from __future__ import annotations

import os
from pathlib import Path


def binary() -> Path:
    """Path of the bundled `cua` executable."""
    return Path(__file__).resolve().parent / "bin" / ("cua.exe" if os.name == "nt" else "cua")
