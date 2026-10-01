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
    parser.add_argument(
        "--expected-id",
        help="expected selection (default: submit-form, or ax:button:increment for --fixture native)",
    )
    parser.add_argument(
        "--fixture",
        choices=("positive", "negative", "native"),
        default="positive",
        help="native uses the cua.jev_choice_request_v2 fixture",
    )
    parser.add_argument(
        "--screenshot",
        type=Path,
        help="run the multimodal S1 adapter with this image, bound to the fixture capture",
    )
    args = parser.parse_args()
    if args.screenshot is not None and args.model != "s1":
        parser.error("--screenshot requires --model s1")

    root = Path(__file__).resolve().parent
    fixture_name = {
        "positive": "jev-choice-request-v1.json",
        "negative": "jev-choice-negative-v1.json",
        "native": "jev-choice-request-v2.json",
    }[args.fixture]
    if args.expected_id is None:
        args.expected_id = "ax:button:increment" if args.fixture == "native" else "submit-form"
    request = json.loads((root / "fixtures" / fixture_name).read_text(encoding="utf-8"))
    command = [sys.executable, str(root / "python/choose_decision.py"), "--model", args.model]
    if args.screenshot is not None:
        command += [
            "--s1-modality",
            "multimodal",
            "--screenshot",
            str(args.screenshot),
            "--screenshot-capture-id",
            request["capture_id"],
        ]
    result = subprocess.run(
        command,
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
    expected_kind = args.expected_id if args.expected_id in {"reobserve", "abstain"} else "selected"
    if (response["kind"], response["selected_id"]) != (expected_kind, args.expected_id):
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
