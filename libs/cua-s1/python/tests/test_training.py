from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest
from torch.utils.data import DataLoader

TRAINING_ROOT = Path(__file__).resolve().parents[2] / "training"
sys.path.insert(0, str(TRAINING_ROOT))

from train import MetaDataset  # noqa: E402


def _write_jsonl(path: Path, rows: list[dict | None]) -> None:
    path.write_text(
        "\n".join("" if row is None else json.dumps(row) for row in rows),
        encoding="utf-8",
    )


def test_meta_dataset_retains_blob_and_parses_rows_on_demand(tmp_path: Path):
    path = tmp_path / "choices.jsonl"
    rows = [
        {"context": "first" * 100, "options": ["click", "skip"], "label": 0},
        None,
        {
            "context": "middle" * 100,
            "options": ["custom value", "skip", "click"],
            "label": 0,
            "meta": {"action": "custom"},
        },
        {"context": "last" * 100, "options": ["scroll", "skip"], "label": 1},
    ]
    _write_jsonl(path, rows)

    dataset = MetaDataset(path)

    assert len(dataset) == 3
    assert not hasattr(dataset, "examples")
    assert dataset.blob == path.read_bytes()
    assert dataset.actions == ["click", "custom", "skip"]
    assert dataset[0].context == rows[0]["context"]
    assert dataset[1].context == rows[2]["context"]
    assert dataset[-1].context == rows[3]["context"]


def test_meta_dataset_supports_shuffled_loader_access(tmp_path: Path):
    path = tmp_path / "choices.jsonl"
    _write_jsonl(
        path,
        [
            {"context": f"row-{index}", "options": ["click", "skip"], "label": index % 2}
            for index in range(12)
        ],
    )
    dataset = MetaDataset(path)

    contexts = [
        batch[0].context
        for batch in DataLoader(dataset, batch_size=1, shuffle=True, collate_fn=lambda rows: rows)
    ]

    assert set(contexts) == {f"row-{index}" for index in range(12)}


def test_meta_dataset_reports_original_line_number(tmp_path: Path):
    path = tmp_path / "invalid.jsonl"
    _write_jsonl(
        path,
        [
            {"context": "valid", "options": ["click", "skip"], "label": 0},
            None,
            {"context": "invalid", "options": ["only one"], "label": 0},
        ],
    )

    with pytest.raises(ValueError, match=r"invalid\.jsonl:3$"):
        MetaDataset(path)
