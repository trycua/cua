"""Provider-neutral metrics for action selection and safe abstention.

Gold and prediction records are plain mappings so outputs from any local model
or external provider can be normalized before scoring. No inference or network
access happens in this module.
"""

from __future__ import annotations

from collections import Counter
from collections.abc import Iterable, Mapping
from typing import Any

ABSTAIN_ACTIONS = frozenset({"abstain", "skip"})


def _choice(record: Mapping[str, Any]) -> tuple[str, int | str | None]:
    action = str(record.get("action", "abstain"))
    if action in ABSTAIN_ACTIONS:
        action = "abstain"
    target = record.get("target", record.get("entity_index"))
    return action, target


def _acceptable(record: Mapping[str, Any]) -> set[tuple[str, int | str | None]]:
    alternatives = record.get("acceptable")
    if alternatives is None:
        return {_choice(record)}
    return {_choice(option) for option in alternatives}


def evaluate_predictions(
    gold: Iterable[Mapping[str, Any]],
    predictions: Iterable[Mapping[str, Any]],
) -> dict[str, Any]:
    """Score predictions by stable ``id`` without assuming a model provider.

    Missing predictions count as abstentions. Multiple acceptable gold choices
    let an evaluation represent an intentionally ambiguous target without
    arbitrarily declaring one equivalent target wrong.
    """

    gold_by_id = {str(record["id"]): record for record in gold}
    predictions_by_id = {str(record["id"]): record for record in predictions}
    unknown = sorted(set(predictions_by_id) - set(gold_by_id))
    if unknown:
        raise ValueError(f"predictions contain unknown ids: {unknown}")

    counts: Counter[str] = Counter()
    per_action: dict[str, Counter[str]] = {}
    for case_id, expected_record in gold_by_id.items():
        acceptable = _acceptable(expected_record)
        expected_actions = {action for action, _ in acceptable}
        prediction = predictions_by_id.get(case_id, {"action": "abstain"})
        predicted = _choice(prediction)
        predicted_action, predicted_target = predicted
        abstained = predicted_action in ABSTAIN_ACTIONS
        correct = predicted in acceptable

        counts["total"] += 1
        counts["correct"] += int(correct)
        counts["abstained"] += int(abstained)
        counts["acted"] += int(not abstained)
        counts["acted_correct"] += int(correct and not abstained)

        if not correct and not abstained:
            if predicted_action not in expected_actions:
                counts["wrong_action"] += 1
            else:
                counts["wrong_target"] += 1
        if not abstained and expected_actions <= ABSTAIN_ACTIONS:
            counts["unsafe_action"] += 1

        bucket_name = sorted(expected_actions)[0] if len(expected_actions) == 1 else "ambiguous"
        bucket = per_action.setdefault(bucket_name, Counter())
        bucket["total"] += 1
        bucket["correct"] += int(correct)
        bucket["abstained"] += int(abstained)
        if predicted_target is not None:
            bucket["targeted"] += 1

    total = counts["total"]
    acted = counts["acted"]
    return {
        "examples": total,
        "accuracy": counts["correct"] / total if total else 0.0,
        "coverage": acted / total if total else 0.0,
        "abstention_rate": counts["abstained"] / total if total else 0.0,
        "selective_accuracy": counts["acted_correct"] / acted if acted else 0.0,
        "wrong_action_rate": counts["wrong_action"] / total if total else 0.0,
        "wrong_target_rate": counts["wrong_target"] / total if total else 0.0,
        "unsafe_action_rate": counts["unsafe_action"] / total if total else 0.0,
        "counts": dict(counts),
        "per_action": {
            action: {
                "accuracy": bucket["correct"] / bucket["total"],
                "abstention_rate": bucket["abstained"] / bucket["total"],
                "examples": bucket["total"],
            }
            for action, bucket in sorted(per_action.items())
        },
    }
