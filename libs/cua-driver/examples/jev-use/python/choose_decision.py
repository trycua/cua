"""Choose one bounded candidate with mock, TypeSafe Jev, or local S1."""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path
from typing import Any

from choose_action import MAX_INPUT_BYTES, validate_request
from decision_models import (
    DecisionRequest,
    ModelScores,
    S1DecisionModel,
    TypeSafeDecisionModel,
    choose,
)


class MockDecisionModel:
    name = "mock"

    def score(self, request: DecisionRequest) -> ModelScores:
        selected = next(
            (
                item["id"]
                for item in request.candidates
                if item["id"] not in {"reobserve", "abstain"}
            ),
            "reobserve",
        )
        return ModelScores(
            {item["id"]: float(item["id"] == selected) for item in request.candidates},
            self.name,
        )


def local_s1_model() -> S1DecisionModel:
    base = Path(os.environ["S1_BASE_MODEL_PATH"]).expanduser()
    adapter = Path(os.environ["S1_ADAPTER_PATH"]).expanduser()
    if not base.is_dir() or not adapter.is_dir():
        raise ValueError("S1_BASE_MODEL_PATH and S1_ADAPTER_PATH must be local directories")
    from cua_s1.four_b import FourBModel

    return S1DecisionModel(
        FourBModel(
            base_model=str(base),
            lora_adapter_path=adapter,
            device=os.environ.get("S1_DEVICE", "cpu"),
            dtype=os.environ.get("S1_DTYPE", "float16"),
            modality="text",
        )
    )


def choose_request(value: Any, model: Any) -> dict[str, Any]:
    request = DecisionRequest.from_validated(validate_request(value))
    return choose(model, request).to_wire()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", choices=("mock", "jev", "s1"), default="mock")
    args = parser.parse_args()
    raw = sys.stdin.buffer.read(MAX_INPUT_BYTES + 1)
    if len(raw) > MAX_INPUT_BYTES:
        raise SystemExit("request exceeds input limit")
    try:
        request = json.loads(raw)
        validate_request(request)
    except (UnicodeDecodeError, json.JSONDecodeError, ValueError, KeyError, TypeError) as error:
        raise SystemExit(f"invalid request: {error}") from None
    if args.model == "mock":
        model = MockDecisionModel()
    elif args.model == "s1":
        try:
            model = local_s1_model()
        except (KeyError, ValueError, ImportError) as error:
            raise SystemExit(f"S1 setup failed: {error}") from None
    else:
        try:
            from typesafe_sdk import TypeSafeClient

            with TypeSafeClient() as client:
                result = choose_request(request, TypeSafeDecisionModel(client))
        except Exception:
            raise SystemExit("provider setup failed") from None
        sys.stdout.write(json.dumps(result, separators=(",", ":")) + "\n")
        return
    result = choose_request(request, model)
    sys.stdout.write(json.dumps(result, separators=(",", ":")) + "\n")


if __name__ == "__main__":
    main()
