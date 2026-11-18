from __future__ import annotations

import pytest
from evals.metrics import evaluate_predictions


def test_metrics_separate_abstention_wrong_action_and_wrong_target():
    gold = [
        {"id": "name", "action": "fill", "target": 0},
        {"id": "consent", "action": "check"},
        {"id": "promo", "action": "abstain"},
        {"id": "city", "action": "fill", "target": 3},
    ]
    predictions = [
        {"id": "name", "action": "fill", "target": 0},
        {"id": "consent", "action": "click"},
        {"id": "promo", "action": "fill", "target": 1},
        {"id": "city", "action": "fill", "target": 2},
    ]

    report = evaluate_predictions(gold, predictions)

    assert report["accuracy"] == pytest.approx(0.25)
    assert report["wrong_action_rate"] == pytest.approx(0.5)
    assert report["wrong_target_rate"] == pytest.approx(0.25)
    assert report["unsafe_action_rate"] == pytest.approx(0.25)
    assert report["coverage"] == 1.0


def test_metrics_accept_equivalent_targets_and_treat_missing_as_abstention():
    gold = [
        {
            "id": "phone",
            "acceptable": [
                {"action": "fill", "target": "mobile"},
                {"action": "fill", "target": "telephone"},
            ],
        },
        {"id": "unknown", "action": "abstain"},
    ]

    report = evaluate_predictions(gold, [{"id": "phone", "action": "fill", "target": "telephone"}])

    assert report["accuracy"] == 1.0
    assert report["abstention_rate"] == pytest.approx(0.5)
    assert report["selective_accuracy"] == 1.0


def test_skip_and_abstain_are_equivalent_safety_choices():
    report = evaluate_predictions(
        [{"id": "optional", "action": "skip"}],
        [{"id": "optional", "action": "abstain"}],
    )

    assert report["accuracy"] == 1.0
    assert report["abstention_rate"] == 1.0
    assert report["unsafe_action_rate"] == 0.0


def test_metrics_reject_predictions_for_unknown_cases():
    with pytest.raises(ValueError, match="unknown ids"):
        evaluate_predictions([], [{"id": "surprise", "action": "click"}])
