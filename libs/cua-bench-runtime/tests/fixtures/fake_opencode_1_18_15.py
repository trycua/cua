"""Minimal OpenCode 1.18.15 launch/config behavior for contract tests."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path


def main() -> int:
    if sys.argv[1:] == ["--version"]:
        print("1.18.15")
        return 0

    parser = argparse.ArgumentParser()
    parser.add_argument("--pure", action="store_true", required=True)
    parser.add_argument("command", choices=("run",))
    parser.add_argument("--model", required=True)
    parser.add_argument("--format", choices=("json",), required=True)
    parser.add_argument("--auto", action="store_true", required=True)
    parser.add_argument("message", nargs="*")
    args = parser.parse_args()

    config_path = Path(os.environ["XDG_CONFIG_HOME"]) / "opencode" / "opencode.json"
    config = json.loads(config_path.read_bytes())
    brief = sys.stdin.buffer.read()
    print(
        json.dumps(
            {
                "config_name": config_path.name,
                "auto": args.auto,
                "pure": args.pure,
                "mcp_command": config["mcp"]["cua"]["command"],
                "model": args.model,
                "positional_count": len(args.message),
                "stdin_bytes": len(brief),
                "stdin_sha256": hashlib.sha256(brief).hexdigest(),
                "version": "1.18.15",
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
