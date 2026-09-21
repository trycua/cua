from __future__ import annotations

import pytest

from cua_s1.four_b import (
    Option,
    OptionProbability,
    assign_letters,
    build_prompt,
)


def _option(element_id: str, action: str = "click") -> Option:
    return Option(element_id=element_id, role="Button", label=element_id, action=action)


def test_assign_letters_gives_each_option_a_unique_single_token_letter():
    options = [_option("submit"), _option("cancel"), _option("email", action="fill")]

    assignment = assign_letters(options)

    assert assignment.letters == ["A", "B", "C"]
    assert len(set(assignment.letters)) == len(options)
    for letter in assignment.letters:
        assert len(letter) == 1
    assert assignment.option_for_letter("B") is options[1]
    assert assignment.option_for_letter("b") is options[1]  # case-insensitive lookup


def test_assign_letters_rejects_empty_or_oversized_option_lists():
    with pytest.raises(ValueError, match="no options"):
        assign_letters([])
    with pytest.raises(ValueError, match="26-letter budget"):
        assign_letters([_option(f"el{i}") for i in range(27)])


def test_build_prompt_text_mode_embeds_ax_tree_and_lettered_options():
    options = [_option("submit"), _option("email", action="fill")]
    assignment = assign_letters(options)

    messages = build_prompt(
        assignment,
        app="TestApp",
        task_family="forms",
        ax_tree="<tree/>",
        modality="text",
    )

    assert messages[0]["role"] == "system"
    assert "single letter" in messages[0]["content"]
    user = messages[1]
    assert user["role"] == "user"
    assert "<tree/>" in user["content"]
    assert "A. Button \"submit\" -> click" in user["content"]
    assert "B. Button \"email\" -> fill" in user["content"]


def test_build_prompt_multimodal_mode_requires_screenshot_and_emits_image_block():
    options = [_option("submit")]
    assignment = assign_letters(options)

    with pytest.raises(ValueError, match="requires screenshot"):
        build_prompt(assignment, app="TestApp", task_family="forms", modality="multimodal")

    messages = build_prompt(
        assignment,
        app="TestApp",
        task_family="forms",
        screenshot="/tmp/shot.png",
        modality="multimodal",
    )
    content = messages[1]["content"]
    assert isinstance(content, list)
    assert content[0] == {"type": "image", "image": "/tmp/shot.png"}
    assert content[1]["type"] == "text"


def test_build_prompt_text_mode_requires_ax_tree():
    options = [_option("submit")]
    assignment = assign_letters(options)
    with pytest.raises(ValueError, match="requires ax_tree"):
        build_prompt(assignment, app="TestApp", task_family="forms", modality="text")


def test_build_prompt_rejects_unknown_modality():
    options = [_option("submit")]
    assignment = assign_letters(options)
    with pytest.raises(ValueError, match="unknown modality"):
        build_prompt(assignment, app="TestApp", task_family="forms", ax_tree="x", modality="bogus")


torch = pytest.importorskip("torch")

from cua_s1.four_b import FourBModel  # noqa: E402


class _FakeOutput:
    def __init__(self, logits):
        self.logits = logits


class _FakeModel:
    """Stands in for the frozen base model's forward pass: returns
    deterministic logits so the answer-letter softmax readout can be tested
    without any real weights, network access, or GPU."""

    def __init__(self, favored_index: int, num_positions: int, vocab_size: int):
        self.favored_index = favored_index
        self.num_positions = num_positions
        self.vocab_size = vocab_size
        self.device = "cpu"

    def __call__(self, **inputs):
        logits = torch.zeros(1, self.num_positions, self.vocab_size)
        logits[0, -1, self.favored_index] = 20.0
        return _FakeOutput(logits)

    def eval(self):
        return self


class _FakeTokenizer:
    """Maps each letter to a distinct single token id, and any other string
    to a fixed multi-token stand-in, matching the shape the real tokenizer's
    `.encode` / `.apply_chat_template` calls have in `FourBModel.forward`."""

    def __init__(self, letter_ids: dict[str, int]):
        self._letter_ids = letter_ids

    def encode(self, text: str, add_special_tokens: bool = False):
        if text in self._letter_ids:
            return [self._letter_ids[text]]
        return [1, 2, 3]

    def apply_chat_template(self, messages, tokenize=False, add_generation_prompt=True):
        return "<chat-text>"

    def __call__(self, text, return_tensors="pt"):
        return _FakeBatch()


class _FakeBatch(dict):
    def to(self, device):
        return self


def test_forward_softmaxes_only_the_option_letter_logits():
    options = [_option("submit"), _option("cancel"), _option("email", action="fill")]
    letter_ids = {"A": 10, "B": 11, "C": 12}

    model = FourBModel(modality="text")
    model._tokenizer = _FakeTokenizer(letter_ids)
    # Favor letter B's token id far above every other logit, including the
    # other two option letters, so the softmax result should put ~all mass
    # on option index 1 regardless of the rest of the (irrelevant) vocab.
    model._model = _FakeModel(favored_index=11, num_positions=5, vocab_size=32)

    results = model.forward(options, app="TestApp", task_family="forms", ax_tree="<tree/>")

    assert [r.letter for r in results] == ["A", "B", "C"]
    assert all(isinstance(r, OptionProbability) for r in results)
    probs = [r.probability for r in results]
    assert probs[1] > 0.99
    assert abs(sum(probs) - 1.0) < 1e-6
    assert results[1].element_id == "cancel"
    assert results[1].action == "click"


def test_letter_token_ids_rejects_multi_token_letters():
    model = FourBModel(modality="text")
    model._tokenizer = _FakeTokenizer({"A": 10})  # "B" falls through to the 3-token stand-in
    assignment = assign_letters([_option("submit"), _option("cancel")])

    with pytest.raises(ValueError, match="single token"):
        model._letter_token_ids(assignment)
