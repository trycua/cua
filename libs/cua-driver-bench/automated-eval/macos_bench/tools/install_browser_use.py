# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))

import browser_use_cli as bu

PINS = json.loads((HERE / "pins.json").read_text())["browser_use_cli"]


def install(wheel: Path, expected_sha: str = PINS["wheel_sha256"]) -> None:
    wheel = wheel.resolve()
    if wheel.name != f"browser_use-{PINS['version']}-py3-none-any.whl":
        raise ValueError("unexpected Browser Use wheel filename")
    if hashlib.sha256(wheel.read_bytes()).hexdigest() != expected_sha:
        raise ValueError("Browser Use wheel sha256 mismatch")
    subprocess.run(
        ["uv", "tool", "install", "--python", PINS["python"], str(wheel)],
        env={
            **os.environ,
            "UV_TOOL_DIR": str(bu.TOOL_DIR),
            "UV_TOOL_BIN_DIR": str(bu.TOOL_BIN_DIR),
        },
        check=True,
        timeout=120,
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("wheel", type=Path)
    install(parser.parse_args().wheel)
