"""Exercise the closed-candidate CLI against a synthetic request."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", choices=("mock", "jev", "s1"), required=True)
    parser.add_argument("--expected-id", default="submit-form")
    args = parser.parse_args()

    root = Path(__file__).resolve().parent
    request = json.loads((root / "fixtures/jev-choice-request-v1.json").read_text(encoding="utf-8"))
    result = subprocess.run(
        [sys.executable, str(root / "python/choose_decision.py"), "--model", args.model],
        input=json.dumps(request),
        text=True,
        capture_output=True,
        check=True,
    )
    response = json.loads(result.stdout)
    expected_keys = {
        "schema",
        "kind",
        "capture_id",
        "selected_id",
        "model",
        "confidence",
        "probabilities",
        "reason",
    }
    if set(response) != expected_keys or response["schema"] != "cua.decision_choice_v1":
        raise RuntimeError("decision model returned an unsupported response")
    if response["capture_id"] != request["capture_id"]:
        raise RuntimeError("decision model returned a different capture")
    if (response["kind"], response["selected_id"]) != ("selected", args.expected_id):
        raise RuntimeError("decision model did not select the expected candidate")
    candidate_ids = {candidate["id"] for candidate in request["candidates"]}
    if set(response["probabilities"]) != candidate_ids:
        raise RuntimeError("decision model returned a different candidate set")
    print(
        json.dumps(
            {
                "model": response["model"],
                "kind": response["kind"],
                "selected_id": response["selected_id"],
            }
        )
    )


if __name__ == "__main__":
    main()
