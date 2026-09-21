from __future__ import annotations

import pytest

torch = pytest.importorskip("torch")

from cua_s1.nano import (  # noqa: E402
    NanoByteCollator,
    NanoElement,
    NanoScorer,
    load_nano_checkpoint,
    make_nano_system,
    parameter_count,
    save_nano_checkpoint,
)

NANO_CONFIG = {
    "width": 8,
    "rank": 4,
    "context_tokens": 32,
    "option_tokens": 16,
}


def test_make_nano_system_builds_matching_model_and_collator():
    model, collator = make_nano_system(NANO_CONFIG, "cpu")
    assert isinstance(model, NanoScorer)
    assert isinstance(collator, NanoByteCollator)
    assert model.width == 8
    assert model.rank == 4
    assert parameter_count(model) > 0


def test_make_nano_system_rejects_invalid_configs():
    with pytest.raises(ValueError, match="positive integer"):
        make_nano_system({**NANO_CONFIG, "width": 0}, "cpu")
    with pytest.raises(ValueError, match="vision_backbone"):
        make_nano_system({**NANO_CONFIG, "vision_backbone": "unknown"}, "cpu")


def test_text_modality_scores_cover_exactly_the_offered_options():
    model, collator = make_nano_system(NANO_CONFIG, "cpu")
    model.eval()
    elements = [
        NanoElement("email-field", "an email input labeled Work email", ("fill email", "skip")),
        NanoElement("submit-button", "a button labeled Submit", ("click", "skip", "check")),
    ]

    scores = model.score_elements(elements, collator)

    assert set(scores) == {"email-field", "submit-button"}
    for element in elements:
        distribution = scores[element.element_id]
        assert set(distribution) == set(range(len(element.options)))
        assert distribution
        assert pytest.approx(sum(distribution.values()), abs=1e-4) == 1.0
        assert all(p >= 0.0 for p in distribution.values())


def test_forward_masks_absent_options_and_produces_finite_logits():
    model, collator = make_nano_system(NANO_CONFIG, "cpu")
    batch = collator(
        [
            NanoElement("a", "context one", ("one", "two")),
            NanoElement("b", "context two", ("one", "two", "three")),
        ]
    )

    logits = model(batch)

    assert logits.shape == (2, 3)
    assert torch.isfinite(logits[0, :2]).all()
    assert torch.isneginf(logits[0, 2]) or logits[0, 2] < -1e20
    assert torch.isfinite(logits[1]).all()


def test_multimodal_forward_uses_precomputed_crop_features():
    model, _collator = make_nano_system(NANO_CONFIG, "cpu")
    model.eval()
    batch_size, crop_tokens, option_count, option_tokens = 2, 5, 3, NANO_CONFIG["option_tokens"]
    batch = {
        "modality": "multimodal",
        "crop_features": torch.randn(batch_size, crop_tokens, model.vision_dim),
        "crop_mask": torch.ones(batch_size, crop_tokens, dtype=torch.bool),
        "option_ids": torch.randint(0, 257, (batch_size, option_count, option_tokens)),
        "option_token_mask": torch.ones(batch_size, option_count, option_tokens, dtype=torch.bool),
        "option_mask": torch.ones(batch_size, option_count, dtype=torch.bool),
    }

    logits = model(batch)

    assert logits.shape == (batch_size, option_count)
    assert torch.isfinite(logits).all()


def test_forward_rejects_unknown_modality():
    model, collator = make_nano_system(NANO_CONFIG, "cpu")
    batch = collator([NanoElement("a", "context", ("one", "two"))])
    batch["modality"] = "audio"

    with pytest.raises(ValueError, match="unknown modality"):
        model(batch)


def test_nano_checkpoint_round_trip_preserves_logits(tmp_path):
    pytest.importorskip("safetensors")
    torch.manual_seed(11)
    model, collator = make_nano_system(NANO_CONFIG, "cpu")
    model.eval()
    elements = [NanoElement("a", "an address field", ("fill address", "skip", "click"))]
    expected = model(collator(elements)).detach()

    save_nano_checkpoint(tmp_path / "checkpoint", model, NANO_CONFIG, {"synthetic": True})
    restored, restored_collator, restored_config = load_nano_checkpoint(tmp_path / "checkpoint", "cpu")
    actual = restored(restored_collator(elements)).detach()

    assert restored_config == NANO_CONFIG
    assert torch.equal(actual, expected)
