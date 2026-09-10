from __future__ import annotations

import argparse
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--workspace", required=True)
parser.add_argument("--artifacts", type=Path, required=True)
args = parser.parse_args()
(args.artifacts / "output.json").write_text("{bad", encoding="utf-8")
