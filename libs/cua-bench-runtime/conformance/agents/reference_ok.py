from __future__ import annotations

import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--workspace", type=Path, required=True)
parser.add_argument("--artifacts", type=Path, required=True)
args = parser.parse_args()
source = json.loads((args.workspace / "input.json").read_text(encoding="utf-8"))
(args.artifacts / "output.json").write_text(
    json.dumps({"value": source["parameters"]["nonce"][::-1]}, sort_keys=True),
    encoding="utf-8",
)
