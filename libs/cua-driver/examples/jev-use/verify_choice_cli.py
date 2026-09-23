from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--live", action="store_true")
    args = parser.parse_args()

    root = Path(__file__).resolve().parent
    request = json.loads(
        (root / "fixtures/jev-choice-request-v1.json").read_text(encoding="utf-8")
    )
    command = [sys.executable, str((root / "python/choose_action.py").resolve())]
    if not args.live:
        command.append("--mock")
    result = subprocess.run(
        command,
        input=json.dumps(request),
        text=True,
        capture_output=True,
        check=True,
    )
    response = json.loads(result.stdout)
    if set(response) != {"schema", "selected_id", "model", "confidence", "probabilities"}:
        raise RuntimeError("chooser returned fields outside the response contract")
    if response["schema"] != "cua.jev_choice_v1":
        raise RuntimeError("chooser returned an unsupported schema")
    candidate_ids = {candidate["id"] for candidate in request["candidates"]}
    if response["selected_id"] not in candidate_ids:
        raise RuntimeError("chooser returned an ID outside the request")
    if not set(response["probabilities"]).issubset(candidate_ids):
        raise RuntimeError("chooser returned probabilities outside the request")
    sys.stdout.write(result.stdout)


if __name__ == "__main__":
    main()
