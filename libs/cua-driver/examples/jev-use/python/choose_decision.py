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
    S1_MODALITIES,
    DecisionRequest,
    ModelScores,
    S1DecisionModel,
    TypeSafeDecisionModel,
    choose,
    s1_model_identity,
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


def _resolve_s1_adapter(adapter: Path, modality: str) -> Path:
    """Return the PEFT adapter directory the chooser will load for `modality`."""
    for candidate in (adapter / modality, adapter):
        if (candidate / "adapter_config.json").is_file():
            return candidate
    raise ValueError(
        f"S1_ADAPTER_PATH has no Cua-S1-4B PEFT adapter for modality {modality!r} "
        f"(expected {modality}/adapter_config.json or adapter_config.json); "
        "cua-s1-nano and cua-s1-forms checkpoints are not supported by this chooser"
    )


def local_s1_model(
    *,
    modality: str | None = None,
    screenshot: Path | None = None,
    screenshot_capture_id: str | None = None,
) -> S1DecisionModel:
    base_value = os.environ["S1_BASE_MODEL_PATH"]
    adapter_value = os.environ["S1_ADAPTER_PATH"]
    if not base_value.strip() or not adapter_value.strip():
        raise ValueError("S1_BASE_MODEL_PATH and S1_ADAPTER_PATH must be local directories")
    base = Path(base_value).expanduser()
    adapter = Path(adapter_value).expanduser()
    if not base.is_dir() or not adapter.is_dir():
        raise ValueError("S1_BASE_MODEL_PATH and S1_ADAPTER_PATH must be local directories")
    modality = modality or os.environ.get("S1_MODALITY", "").strip() or "text"
    if modality not in S1_MODALITIES:
        raise ValueError(f"S1 modality must be one of {', '.join(S1_MODALITIES)}")
    if modality == "multimodal":
        if screenshot is None or not screenshot_capture_id:
            raise ValueError(
                "multimodal S1 requires --screenshot and --screenshot-capture-id "
                "from the same capture as the request"
            )
        screenshot = screenshot.expanduser()
        if not screenshot.is_file():
            raise ValueError("--screenshot must be an existing local image file")
    elif screenshot is not None or screenshot_capture_id is not None:
        raise ValueError("--screenshot and --screenshot-capture-id require multimodal S1")
    _resolve_s1_adapter(adapter, modality)
    name = s1_model_identity(
        adapter,
        modality,
        repo_id=os.environ.get("S1_ADAPTER_ID"),
        revision=os.environ.get("S1_ADAPTER_REVISION"),
    )
    from cua_s1.four_b import FourBModel

    return S1DecisionModel(
        FourBModel(
            base_model=str(base),
            lora_adapter_path=adapter,
            device=os.environ.get("S1_DEVICE", "cpu"),
            dtype=os.environ.get("S1_DTYPE", "float16"),
            modality=modality,
        ),
        modality=modality,
        screenshot_path=screenshot,
        screenshot_capture_id=screenshot_capture_id,
        name=name,
    )


def choose_request(value: Any, model: Any) -> dict[str, Any]:
    request = DecisionRequest.from_validated(validate_request(value))
    return choose(model, request).to_wire()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", choices=("mock", "jev", "s1"), default="mock")
    parser.add_argument(
        "--s1-modality",
        choices=S1_MODALITIES,
        help="S1 adapter to load (default: $S1_MODALITY, else text)",
    )
    parser.add_argument(
        "--screenshot",
        type=Path,
        help="multimodal S1 only: local image from the request's capture",
    )
    parser.add_argument(
        "--screenshot-capture-id",
        help="multimodal S1 only: capture ID of --screenshot; must equal the request capture_id",
    )
    args = parser.parse_args()
    if args.model != "s1" and (
        args.s1_modality or args.screenshot is not None or args.screenshot_capture_id is not None
    ):
        parser.error("--s1-modality, --screenshot, and --screenshot-capture-id require --model s1")
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
            model = local_s1_model(
                modality=args.s1_modality,
                screenshot=args.screenshot,
                screenshot_capture_id=args.screenshot_capture_id,
            )
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
