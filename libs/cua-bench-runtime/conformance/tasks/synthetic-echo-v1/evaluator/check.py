"""Independent evaluator for the synthetic lifecycle task."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--agent-exit-code", type=int, required=True)
    args = parser.parse_args()

    source = json.loads((args.workspace / "input.json").read_text(encoding="utf-8"))
    expected = source["parameters"]["nonce"][::-1]
    output_path = args.artifacts / "output.json"
    passed = False
    detail: dict[str, object] = {"agent_exit_code": args.agent_exit_code}
    if output_path.is_file():
        try:
            output = json.loads(output_path.read_text(encoding="utf-8"))
            passed = output.get("value") == expected and args.agent_exit_code == 0
            detail["output_parseable"] = True
        except json.JSONDecodeError:
            detail["output_parseable"] = False
    else:
        detail["output_parseable"] = False
    args.result.write_text(
        json.dumps(
            {"passed": passed, "score": 1.0 if passed else 0.0, "detail": detail},
            sort_keys=True,
        ),
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
