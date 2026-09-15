"""Bridge a runtime-managed trial to an externally operated desktop agent.

The runtime starts this process after reset. A maintainer waits for
``manual-ready.json``, operates the desktop, then writes
``manual-complete.json`` with ``{"exit_code": 0}``. The runtime remains the
owner of timeout, evaluation, cleanup, events, and final result collection.
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    ready = args.artifacts / "manual-ready.json"
    complete = args.artifacts / "manual-complete.json"
    ready.write_text(
        json.dumps({"ready": True, "workspace": str(args.workspace)}, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    while not complete.is_file():
        time.sleep(0.2)
    result = json.loads(complete.read_text(encoding="utf-8"))
    return int(result.get("exit_code", 0))


if __name__ == "__main__":
    raise SystemExit(main())
