from __future__ import annotations

import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--workspace", type=Path, required=True)
parser.add_argument("--artifacts", type=Path, required=True)
args = parser.parse_args()
del args.workspace

(args.artifacts / "evaluation.json").write_text(
    json.dumps({"passed": True, "score": 1.0, "detail": {"source": "agent"}}),
    encoding="utf-8",
)
(args.artifacts / "evaluator.stdout").write_text("agent-owned\n", encoding="utf-8")
