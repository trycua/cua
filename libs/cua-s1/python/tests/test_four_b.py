from __future__ import annotations

import pytest

from cua_s1.four_b import (
    ASYNC_LOAD_ENV,
    Option,
    OptionProbability,
    assign_letters,
    build_prompt,
    needs_sync_weight_loading,
    sync_weight_loading,
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
    assert 'A. Button "submit" -> click' in user["content"]
    assert 'B. Button "email" -> fill' in user["content"]


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


@pytest.mark.parametrize(
    ("device", "expected"),
    [
        ("mps", True),
        ("mps:0", True),
        (" MPS ", True),
        ("cuda", False),
        ("cuda:0", False),
        ("cpu", False),
        ("auto", False),
    ],
)
def test_needs_sync_weight_loading_only_for_mps(device, expected):
    assert needs_sync_weight_loading(device) is expected


def test_sync_weight_loading_sets_and_restores_the_async_load_switch(monkeypatch):
    import os

    monkeypatch.delenv(ASYNC_LOAD_ENV, raising=False)

    with sync_weight_loading(True):
        assert os.environ[ASYNC_LOAD_ENV] == "1"
    assert ASYNC_LOAD_ENV not in os.environ

    with sync_weight_loading(False):
        assert ASYNC_LOAD_ENV not in os.environ

    with pytest.raises(RuntimeError), sync_weight_loading(True):
        raise RuntimeError("load failed")
    assert ASYNC_LOAD_ENV not in os.environ


def test_sync_weight_loading_keeps_an_explicit_environment_choice(monkeypatch):
    import os

    monkeypatch.setenv(ASYNC_LOAD_ENV, "0")

    with sync_weight_loading(True):
        assert os.environ[ASYNC_LOAD_ENV] == "0"
    assert os.environ[ASYNC_LOAD_ENV] == "0"


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


def test_resolve_adapter_path_picks_modality_subdir_when_present(tmp_path):
    root = tmp_path / "cua-s1-4b-0.1"
    (root / "text").mkdir(parents=True)
    (root / "text" / "adapter_config.json").write_text("{}")
    (root / "multimodal").mkdir(parents=True)
    (root / "multimodal" / "adapter_config.json").write_text("{}")

    text_model = FourBModel(lora_adapter_path=root, modality="text")
    mm_model = FourBModel(lora_adapter_path=root, modality="multimodal")

    assert text_model._resolve_adapter_path() == root / "text"
    assert mm_model._resolve_adapter_path() == root / "multimodal"


def test_resolve_adapter_path_falls_back_to_root_without_subdirs(tmp_path):
    root = tmp_path / "semif_lora_multimodal"
    root.mkdir()
    (root / "adapter_config.json").write_text("{}")

    model = FourBModel(lora_adapter_path=root, modality="multimodal")

    assert model._resolve_adapter_path() == root


def test_letter_token_ids_rejects_multi_token_letters():
    model = FourBModel(modality="text")
    model._tokenizer = _FakeTokenizer({"A": 10})  # "B" falls through to the 3-token stand-in
    assignment = assign_letters([_option("submit"), _option("cancel")])

    with pytest.raises(ValueError, match="single token"):
        model._letter_token_ids(assignment)


@pytest.mark.parametrize(("device", "expected"), [("mps", "1"), ("cpu", None), ("cuda", None)])
def test_load_disables_threaded_weight_loading_only_on_mps(monkeypatch, device, expected):
    import os

    import sys
    import types

    monkeypatch.delenv(ASYNC_LOAD_ENV, raising=False)
    seen: dict[str, object] = {}

    class _FakeAuto:
        @staticmethod
        def from_pretrained(name, **kwargs):
            seen["async_env"] = os.environ.get(ASYNC_LOAD_ENV)
            seen["device_map"] = kwargs.get("device_map")
            return _FakeModel(favored_index=0, num_positions=1, vocab_size=1)

    class _NoProcessor:
        @staticmethod
        def from_pretrained(name, **kwargs):
            raise OSError("no processor")

    # A stand-in `transformers` module keeps this test offline and independent
    # of whether the `four-b` extra is installed.
    fake_transformers = types.ModuleType("transformers")
    fake_transformers.AutoTokenizer = _FakeAuto
    fake_transformers.AutoProcessor = _NoProcessor
    fake_transformers.AutoModelForCausalLM = _FakeAuto
    fake_transformers.AutoModelForImageTextToText = _FakeAuto
    monkeypatch.setitem(sys.modules, "transformers", fake_transformers)

    FourBModel(base_model="local-base", device=device, dtype="float16").load()

    assert seen == {"async_env": expected, "device_map": device}
    assert ASYNC_LOAD_ENV not in os.environ
