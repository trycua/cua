"""Cua-S1-4B: a LoRA fine-tune on the frozen, open-weight `Qwen/Qwen3.5-4B`
model for computer-use GUI decisions.

Unlike the `tiny`/`tinyx` scorers in `cua_s1.model` (small, from-scratch
byte-transformer encoders trained end to end), `cua-s1-4b` is a LoRA adapter
layered on top of an existing, openly licensed 4B-parameter chat/vision
language model. The base model's weights are never modified or redistributed
by this package; only the (much smaller) LoRA adapter is a Cua-S1 artifact.

Decoding contract implemented here:

  1. Build a chat-template prompt that describes the current screen state and
     a fixed, closed list of candidate (element, action) options, each given
     a single answer letter.
  2. Verify that every option letter encodes to exactly one tokenizer token
     id for the loaded base model -- this is a hard precondition, not a best
     effort, because the readout below only makes sense if "the token for
     letter X" is unambiguous.
  3. Run one forward pass over the whole prompt (no per-option branching or
     KV-cache reuse across options).
  4. Read the logits at the final sequence position, slice out just the
     option-letter token ids, and softmax those into per-option
     probabilities.

This module never imports `torch`/`transformers`/`peft` at import time, so
the prompt-building and letter-assignment logic (`assign_letters`,
`build_prompt`) can be unit-tested without a GPU, without those optional
dependencies installed, and without downloading any model weights. Those
libraries are only imported inside `FourBModel.load`.
"""

from __future__ import annotations

import os
import string
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

DEFAULT_BASE_MODEL = "Qwen/Qwen3.5-4B"

LETTERS = string.ascii_uppercase  # "A".."Z" -- enough letters for any realistic option set

# Transformers 5 materializes checkpoint tensors on a thread pool unless this
# variable is true. On Apple silicon, concurrent dtype casts onto `mps` (for
# example bf16 checkpoint shards loaded as float16) can segfault or hang
# during `from_pretrained`, so `FourBModel.load` turns it on for `mps` loads.
ASYNC_LOAD_ENV = "HF_DEACTIVATE_ASYNC_LOAD"


def needs_sync_weight_loading(device: str) -> bool:
    """Return whether weights for `device` must be materialized sequentially.

    Only Apple's `mps` backend (`"mps"` or `"mps:<index>"`) needs it; CUDA and
    CPU keep Transformers' default threaded loader.
    """
    return str(device).strip().lower().split(":", 1)[0] == "mps"


@contextmanager
def sync_weight_loading(enabled: bool) -> Iterator[None]:
    """Set `HF_DEACTIVATE_ASYNC_LOAD=1` for the duration of a model load.

    Does nothing when `enabled` is false or when the caller already set the
    variable explicitly (any value), so an explicit environment choice always
    wins. The previous environment is restored afterwards.
    """
    if not enabled or ASYNC_LOAD_ENV in os.environ:
        yield
        return
    os.environ[ASYNC_LOAD_ENV] = "1"
    try:
        yield
    finally:
        os.environ.pop(ASYNC_LOAD_ENV, None)


@dataclass(frozen=True)
class Option:
    """One candidate (element, action) decision for a single screen state.

    `entity_id` is only meaningful for `action == "fill"`, where it names
    which extracted value would be entered if this option were chosen.
    """

    element_id: str
    role: str
    label: str
    action: str
    entity_id: str | None = None


@dataclass(frozen=True)
class LetterAssignment:
    """The mapping from answer letters to `Option`s for one specific screen
    state's option list.

    A model call's option list is not a fixed global vocabulary: it is
    whatever closed set of candidate decisions the caller is choosing among
    for that one turn, in whatever order the caller built it. Letters are
    therefore assigned fresh, in the given option order, and must be
    re-derived per call -- never cached or reused across calls, since two
    calls can use the same letter for unrelated options.
    """

    letters: list[str]
    options: list[Option]

    def option_for_letter(self, letter: str) -> Option:
        idx = self.letters.index(letter.strip().upper())
        return self.options[idx]


def assign_letters(options: list[Option]) -> LetterAssignment:
    """Assign one letter per option, in the given option order.

    A pure function of `options`'s existing order: it does not re-sort or
    canonicalize anything, so repeated calls with the same list are stable.
    """
    n = len(options)
    if n == 0:
        raise ValueError("no options to assign letters to")
    if n > len(LETTERS):
        raise ValueError(f"{n} options exceeds the {len(LETTERS)}-letter budget")
    return LetterAssignment(letters=list(LETTERS[:n]), options=list(options))


def _describe_option(letter: str, option: Option) -> str:
    action_desc = option.action
    if option.action == "fill" and option.entity_id:
        action_desc += f" (with entity '{option.entity_id}')"
    return f'{letter}. {option.role} "{option.label}" -> {action_desc}'


SYSTEM_PROMPT = (
    "You are a one-pass computer-use decision model. You are shown the "
    "current state of a screen and a fixed, closed list of candidate "
    "(element, action) options, each given a single letter. Choose exactly "
    "one option: the single best next action to take. Answer with ONLY that "
    "option's letter -- no words, no punctuation, no explanation."
)


def build_prompt(
    assignment: LetterAssignment,
    *,
    app: str,
    task_family: str,
    ax_tree: str | None = None,
    screenshot: str | Path | None = None,
    modality: str = "text",
    goal: str | None = None,
) -> list[dict]:
    """Build chat-template messages for one screen state and option list.

    `goal` is the user's stated objective for the episode, when the caller has
    one that the state itself does not already show. It is printed above the
    state description in both modalities. Callers whose state already carries
    the goal (a synthetic page that renders it, an accessibility tree that
    includes it) leave it unset, so the goal is never stated twice.

    `modality="text"` describes the state with `ax_tree` (an accessibility
    tree). `modality="multimodal"` instead references an attached
    screenshot -- `Qwen/Qwen3.5-4B` is natively vision-language
    ("image-text-to-text"), so this uses the base model's own image
    preprocessing rather than a separate vision model. In multimodal mode
    this function only emits the text scaffold plus a placeholder image
    content block; the actual image bytes are attached by the caller
    (`FourBModel.forward`) via the processor's multimodal content format.

    Returns chat messages (`list[{"role", "content"}]`) ready for
    `tokenizer.apply_chat_template(...)` or `processor.apply_chat_template(...)`.
    """
    if modality not in ("text", "multimodal"):
        raise ValueError(f"unknown modality: {modality!r}")
    if modality == "text" and not ax_tree:
        raise ValueError("modality='text' requires ax_tree")
    if modality == "multimodal" and not screenshot:
        raise ValueError("modality='multimodal' requires screenshot")

    option_lines = "\n".join(
        _describe_option(letter, option)
        for letter, option in zip(assignment.letters, assignment.options, strict=False)
    )
    state_desc = (
        (f"Goal: {goal}\n\n" if goal else "")
        + f"App: {app}\nTask family: {task_family}\n\n"
        + (
            f"Accessibility tree:\n{ax_tree}\n\n"
            if modality == "text"
            else "The current screenshot is attached.\n\n"
        )
        + f"Options:\n{option_lines}\n\nAnswer with a single letter."
    )

    user_content: str | list[dict]
    if modality == "multimodal":
        user_content = [
            {"type": "image", "image": str(screenshot)},
            {"type": "text", "text": state_desc},
        ]
    else:
        user_content = state_desc

    return [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": user_content},
    ]


@dataclass
class OptionProbability:
    element_id: str
    role: str
    label: str
    action: str
    entity_id: str | None
    letter: str
    probability: float


class FourBModel:
    """Loads the frozen `Qwen/Qwen3.5-4B` base model, optionally with a
    `cua-s1-4b` LoRA adapter, and exposes
    `forward(options, ...) -> list[OptionProbability]`.

    Loading is lazy (on first `forward()` call, or an explicit `.load()`)
    so importing this module, and unit-testing the prompt/letter-mapping
    logic above, never requires `torch`/`transformers`/`peft` to be
    installed or the base model's multi-gigabyte weights to be downloaded.

    This checkpoint family is a LoRA fine-tune on top of a frozen,
    third-party open-weight base model, not a model trained from scratch by
    this project. `lora_adapter_path` is expected to be a directory in the
    standard PEFT on-disk layout (`adapter_config.json` plus one or more
    `adapter_model.safetensors` files), which is a different shape from
    `cua_s1.checkpoint`'s single-file `model.safetensors` + `config.json`
    convention used by the from-scratch `tiny`/`tinyx` scorers in
    `cua_s1.model`. That convention does not fit a LoRA adapter (which is a
    named collection of low-rank deltas keyed by base-model module path, not
    a flat state dict for a model this package defines), so this module does
    not force it into `cua_s1.checkpoint`'s shape; it loads the adapter
    directly via `peft.PeftModel.from_pretrained`, which already validates
    that the adapter's on-disk weights are safetensors (PEFT does not save
    or load pickled adapter weights).
    """

    def __init__(
        self,
        base_model: str = DEFAULT_BASE_MODEL,
        lora_adapter_path: str | Path | None = None,
        device: str = "cuda",
        dtype: str = "bfloat16",
        modality: str = "text",
    ) -> None:
        self.base_model = base_model
        self.lora_adapter_path = lora_adapter_path
        self.device = device
        self.dtype = dtype
        self.modality = modality
        self._model = None
        self._tokenizer = None
        self._processor = None  # multimodal image/text processor, if the base model ships one

    def load(self) -> None:
        """Load the tokenizer/processor, the frozen base model, and (if
        given) the `cua-s1-4b` LoRA adapter.

        Deferred import of `torch`/`transformers`/`peft` so this module can
        be imported, and its prompt-building and letter-assignment logic
        unit-tested, without those optional dependencies installed.
        """
        import torch
        from transformers import (
            AutoModelForCausalLM,
            AutoModelForImageTextToText,
            AutoProcessor,
            AutoTokenizer,
        )

        torch_dtype = getattr(torch, self.dtype)
        self._tokenizer = AutoTokenizer.from_pretrained(self.base_model)
        try:
            self._processor = AutoProcessor.from_pretrained(self.base_model)
        except (OSError, ValueError):
            # Some checkpoints ship no AutoProcessor at all (text-only base
            # models). Narrowed to the two exception types transformers
            # actually raises for "no processor config found" / "unrecognized
            # processor" -- a download/auth failure or a torch/CUDA error
            # must not be silently treated as "this base model has no
            # processor".
            self._processor = None

        if self.modality == "multimodal" and self._processor is None:
            raise RuntimeError(
                f"modality='multimodal' was requested for {self.base_model!r} "
                "but no AutoProcessor loaded for it -- refusing to silently "
                "fall back to a text-only model class, which would load the "
                "model without its vision tower."
            )
        # The multimodal model class must be selected from the *requested*
        # modality, not from whether a processor happened to load: some
        # base models expose a processor unconditionally (it is the same
        # vision-capable checkpoint either way), so keying the class choice
        # on processor availability rather than the caller's stated
        # modality can silently select a model class whose module layout
        # does not match a LoRA adapter trained against the other class,
        # causing `peft` to attach ~none of the adapter's weights (`peft`
        # only warns on missing adapter keys; it does not raise).
        model_cls = (
            AutoModelForImageTextToText if self.modality == "multimodal" else AutoModelForCausalLM
        )
        # Sequential materialization on `mps` avoids a crash or hang in
        # Transformers' threaded loader; see `ASYNC_LOAD_ENV`.
        with sync_weight_loading(needs_sync_weight_loading(self.device)):
            model = model_cls.from_pretrained(
                self.base_model, torch_dtype=torch_dtype, device_map=self.device
            )
            if self.lora_adapter_path:
                from peft import PeftModel

                model = PeftModel.from_pretrained(model, str(self._resolve_adapter_path()))
        model.eval()
        self._model = model

    def _resolve_adapter_path(self) -> Path:
        """Resolve `lora_adapter_path` to the on-disk adapter directory to
        actually load for `self.modality`.

        `cua-s1-4b`'s text and multimodal LoRA adapters are two independently
        trained checkpoints with different `target_modules` (the text
        adapter is a flat causal-LM LoRA; the multimodal one is jointly
        trained with the vision projector's `linear_fc1`/`linear_fc2`
        modules, and its key paths only match `AutoModelForImageTextToText`'s
        nested `.language_model.` layer structure). They are not
        interchangeable and are not merged into one adapter, so the on-disk
        layout mirrors `cua-s1-nano-0.1`: a `text/` and a `multimodal/`
        subdirectory, each a standalone PEFT adapter dir (`adapter_config.json`
        + `adapter_model.safetensors`), under one adapter root.

        If `lora_adapter_path` already points directly at a standalone
        adapter dir (no `text/`/`multimodal/` subdirs -- e.g. a bare
        checkpoint dir used in training/eval scripts), it is used as-is, so
        this stays backward compatible with callers that already pass a
        fully-resolved single-modality adapter path.
        """
        root = Path(self.lora_adapter_path)
        per_modality = root / self.modality
        if per_modality.is_dir() and (per_modality / "adapter_config.json").exists():
            return per_modality
        return root

    def _ensure_loaded(self) -> None:
        if self._model is None:
            self.load()

    def _letter_token_ids(self, assignment: LetterAssignment) -> list[int]:
        """Verify each option letter maps to exactly one token id and
        return those ids in option order."""
        ids = []
        for letter in assignment.letters:
            tokens = self._tokenizer.encode(letter, add_special_tokens=False)
            if len(tokens) != 1:
                raise ValueError(
                    f"letter {letter!r} does not map to a single token for "
                    f"{self.base_model} (got {len(tokens)} tokens) -- the "
                    "answer-letter readout requires exactly one token per "
                    "option letter"
                )
            ids.append(tokens[0])
        return ids

    def forward(
        self,
        options: list[Option],
        *,
        app: str,
        task_family: str,
        ax_tree: str | None = None,
        screenshot: str | Path | None = None,
        modality: str | None = None,
        goal: str | None = None,
    ) -> list[OptionProbability]:
        """Score every option in one forward pass, returning a probability
        per option: softmax over just the option-letter token logits at the
        final sequence position.

        `goal` is passed straight through to `build_prompt`; see its docstring
        for when a caller should supply one."""
        self._ensure_loaded()
        import torch

        modality = modality or self.modality
        assignment = assign_letters(options)
        letter_ids = self._letter_token_ids(assignment)
        messages = build_prompt(
            assignment,
            app=app,
            task_family=task_family,
            ax_tree=ax_tree,
            screenshot=screenshot,
            modality=modality,
            goal=goal,
        )

        if modality == "multimodal" and self._processor is not None:
            from PIL import Image

            image = Image.open(screenshot).convert("RGB")
            # Pass `messages` through unflattened so the processor's own
            # chat template inserts the real image placeholder tokens; the
            # image is filled in by `images=[image]` below. Flattening
            # `messages` down to plain text first would strip the
            # `{"type": "image", ...}` content block, so the chat template
            # would never emit the placeholder tokens the expanded image
            # features need to be scattered into.
            chat_text = self._processor.apply_chat_template(
                messages, tokenize=False, add_generation_prompt=True
            )
            inputs = self._processor(text=[chat_text], images=[image], return_tensors="pt").to(
                self._model.device
            )
        else:
            chat_text = self._tokenizer.apply_chat_template(
                messages, tokenize=False, add_generation_prompt=True
            )
            inputs = self._tokenizer(chat_text, return_tensors="pt").to(self._model.device)

        with torch.no_grad():
            out = self._model(**inputs)
        final_logits = out.logits[0, -1, :]  # (vocab,) at the final position -- single forward pass
        option_logits = final_logits[torch.tensor(letter_ids, device=final_logits.device)]
        probs = torch.softmax(option_logits.float(), dim=-1).tolist()

        results = []
        for letter, option, probability in zip(
            assignment.letters, assignment.options, probs, strict=False
        ):
            results.append(
                OptionProbability(
                    element_id=option.element_id,
                    role=option.role,
                    label=option.label,
                    action=option.action,
                    entity_id=option.entity_id,
                    letter=letter,
                    probability=probability,
                )
            )
        return results


# TODO: a LoRA training recipe for cua-s1-4b (fine-tuning the adapter used
# above against the frozen Qwen/Qwen3.5-4B base model) belongs under
# libs/cua-s1/training/ as a follow-up; this module is inference-only.
