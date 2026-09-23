from __future__ import annotations

import pytest

torch = pytest.importorskip("torch")

from cua_s1.model import (  # noqa: E402
    ByteCollator,
    ChoiceExample,
    load_checkpoint,
    make_system,
    save_checkpoint,
    validate_example,
)

TINY_CONFIG = {
    "encoder": "tiny",
    "width": 8,
    "rank": 4,
    "context_tokens": 32,
    "option_tokens": 16,
}


def test_choice_example_validation_rejects_invalid_labels():
    example = validate_example({"context": "field", "options": ["fill value", "skip"], "label": 1})
    assert example == ChoiceExample("field", ("fill value", "skip"), 1)

    with pytest.raises(ValueError, match="valid option index"):
        validate_example({"context": "field", "options": ["a", "b"], "label": 2})
    with pytest.raises(ValueError, match="at least two"):
        validate_example({"context": "field", "options": ["skip"], "label": 0})


def test_byte_collator_masks_variable_option_counts():
    batch = ByteCollator(context_tokens=8, option_tokens=6)(
        [
            ChoiceExample("abc", ("one", "two"), 0),
            ChoiceExample("xy", ("one", "two", "three"), 2),
        ]
    )

    assert batch["context_ids"].shape == (2, 3)
    assert batch["option_ids"].shape == (2, 3, 5)
    assert batch["option_mask"].tolist() == [[True, True, False], [True, True, True]]
    assert batch["labels"].tolist() == [0, 2]


def test_tiny_model_scores_options_without_downloading_weights():
    model, collator = make_system(TINY_CONFIG, "cpu")
    batch = collator(
        [
            ChoiceExample("email field", ("fill email", "skip"), 0),
            ChoiceExample("button", ("click", "skip", "check"), 1),
        ]
    )

    logits = model(batch)

    assert logits.shape == (2, 3)
    assert torch.isfinite(logits[0, :2]).all()
    assert torch.isneginf(logits[0, 2]) or logits[0, 2] < -1e20
    assert torch.isfinite(logits[1]).all()


def test_make_system_rejects_unknown_or_invalid_configs():
    with pytest.raises(ValueError, match="encoder"):
        make_system({**TINY_CONFIG, "encoder": "unknown"}, "cpu")
    with pytest.raises(ValueError, match="'tiny' or 'tinyx'"):
        make_system({**TINY_CONFIG, "encoder": "hf", "hf_model": "remote/model"}, "cpu")
    with pytest.raises(ValueError, match="positive integer"):
        make_system({**TINY_CONFIG, "width": 0}, "cpu")


def test_model_checkpoint_round_trip_preserves_logits(tmp_path):
    pytest.importorskip("safetensors")
    torch.manual_seed(7)
    model, collator = make_system(TINY_CONFIG, "cpu")
    model.eval()
    examples = [ChoiceExample("city", ("fill city", "skip", "click"), 0)]
    expected = model(collator(examples)).detach()

    save_checkpoint(tmp_path / "checkpoint", model, TINY_CONFIG, {"synthetic": True})
    restored, restored_collator, restored_config = load_checkpoint(tmp_path / "checkpoint", "cpu")
    actual = restored(restored_collator(examples)).detach()

    assert restored_config == TINY_CONFIG
    assert torch.equal(actual, expected)
