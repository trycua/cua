from __future__ import annotations

import hashlib
import json

import pytest

from cua_s1.synth import SynthesisConfig, episode_rows, write_splits


def test_episode_generation_is_seed_deterministic():
    assert episode_rows(2026) == episode_rows(2026)
    assert episode_rows(2026) != episode_rows(2027)


def test_generated_rows_are_self_consistent_choice_examples():
    rows = episode_rows(41)

    assert rows
    signatures = {row["meta"]["form_signature"] for row in rows}
    assert len(signatures) == 1
    assert {row["meta"]["seed"] for row in rows} == {41}
    for row in rows:
        assert row["context"]
        assert len(row["options"]) >= 3
        assert 0 <= row["label"] < len(row["options"])
        assert row["meta"]["action"] in {"fill", "check", "click", "skip"}


def test_split_files_are_reproducible_and_form_disjoint(tmp_path):
    first = tmp_path / "first"
    second = tmp_path / "second"

    assert write_splits(first, episodes=120, seed=1234) == write_splits(
        second, episodes=120, seed=1234
    )

    signatures_by_split = {}
    for split in ("train", "validation", "test"):
        first_bytes = (first / f"{split}.jsonl").read_bytes()
        assert first_bytes == (second / f"{split}.jsonl").read_bytes()
        rows = [json.loads(line) for line in first_bytes.splitlines()]
        signatures_by_split[split] = {row["meta"]["form_signature"] for row in rows}

    assert signatures_by_split["train"].isdisjoint(signatures_by_split["validation"])
    assert signatures_by_split["train"].isdisjoint(signatures_by_split["test"])
    assert signatures_by_split["validation"].isdisjoint(signatures_by_split["test"])

    first_manifest = json.loads((first / "manifest.json").read_text(encoding="utf-8"))
    second_manifest = json.loads((second / "manifest.json").read_text(encoding="utf-8"))
    assert first_manifest == second_manifest
    assert first_manifest["format"] == "cua-s1-choice-jsonl"
    assert first_manifest["format_version"] == 1
    assert first_manifest["generator"]["format"] == "cua_s1.synth"
    assert first_manifest["generator"]["config"] == {
        "filled_field_probability": 0.6,
        "hard_negative_probability": 0.35,
        "max_distractor_entities": 3,
        "max_extra_entities": 5,
        "missing_entity_probability": 0.12,
        "partial_form_probability": 0.3,
        "stale_value_probability": 0.05,
    }
    assert first_manifest["ratios"] == {"train": 0.8, "validation": 0.1, "test": 0.1}
    assert first_manifest["seed"] == 1234
    assert first_manifest["episodes"] == 120
    for split in ("train", "validation", "test"):
        split_bytes = (first / f"{split}.jsonl").read_bytes()
        assert first_manifest["splits"][split]["sha256"] == hashlib.sha256(split_bytes).hexdigest()


@pytest.mark.parametrize(
    "config",
    [
        SynthesisConfig(missing_entity_probability=-0.01),
        SynthesisConfig(partial_form_probability=1.01),
        SynthesisConfig(max_extra_entities=-1),
    ],
)
def test_invalid_synthesis_limits_are_rejected(config):
    with pytest.raises(ValueError):
        episode_rows(1, config)
