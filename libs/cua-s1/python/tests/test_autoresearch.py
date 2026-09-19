from __future__ import annotations

import sys
from pathlib import Path

import pytest

TRAINING_ROOT = Path(__file__).resolve().parents[2] / "training"
sys.path.insert(0, str(TRAINING_ROOT))

from autoresearch import build_manifest, select_best  # noqa: E402
from train import configure_determinism  # noqa: E402


def _record(name: str, top1: float, nll: float, test_top1: float = 0.99) -> dict:
    return {
        "name": name,
        "best_validation": {"top1": top1, "nll": nll},
        "test": {"top1": test_top1},
    }


def test_select_best_uses_validation_metrics_and_deterministic_tiebreaks():
    records = [
        _record("higher-test-only", 0.8, 0.3, test_top1=1.0),
        _record("lower-nll", 0.9, 0.2, test_top1=0.1),
        _record("higher-nll", 0.9, 0.4),
        _record("z-name-tiebreak", 0.9, 0.2),
    ]

    assert select_best(records)["name"] == "z-name-tiebreak"


def test_build_manifest_records_selection_and_replay_settings():
    records = [_record("first", 0.7, 0.5), _record("winner", 0.8, 0.4)]

    assert build_manifest(records, base_seed=17, deterministic=True) == {
        "base_seed": 17,
        "deterministic": True,
        "best": "winner",
        "best_checkpoint": "best",
        "experiments": records,
    }


def test_deterministic_configuration_does_not_allow_warning_only(monkeypatch):
    calls = []
    monkeypatch.setattr(
        "train.torch.use_deterministic_algorithms",
        lambda enabled, **kwargs: calls.append((enabled, kwargs)),
    )

    configure_determinism(seed=5, deterministic=True)

    assert calls == [(True, {})]


def test_select_best_rejects_empty_records():
    with pytest.raises(ValueError, match="empty result set"):
        select_best([])
