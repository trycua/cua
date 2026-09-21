#!/usr/bin/env python3
"""LoRA fine-tune the frozen `Qwen/Qwen3.5-4B` base model into a `cua-s1-4b`
adapter.

`cua_s1.four_b` (see `libs/cua-s1/python/src/cua_s1/four_b.py`) implements
inference only: it loads the frozen base model, optionally layers a PEFT LoRA
adapter on top, builds a chat-template prompt describing a screen state plus a
closed, lettered list of candidate (element, action) options, and reads a
per-option probability off the final-position logits for those option-letter
tokens. This script trains the adapter that `FourBModel(lora_adapter_path=...)`
loads: it specializes the frozen model's answer-letter logit distribution to
reflect a real `cua_bench_s1` training split's gold actions, rather than
relying purely on zero-shot prompting.

Loss design: soft-label cross-entropy over the option-letter logits at the
final sequence position. The target distribution is NOT one-hot on a single
letter, because `CuaTask.expected` is a per-element map and a single task can
have more than one correct (element, action) pair open at once (e.g. two
still-empty form fields both need `fill` this turn). The target instead puts
uniform probability mass across all gold-option letters and zero elsewhere
("one-hot-set", a generalization of one-hot to more than one correct answer);
cross-entropy against a uniform target is equivalent to averaging the
per-letter NLL of each gold option, a simple and reasonable choice for
multi-gold tasks without needing a margin/ranking loss.

Usage:
    python libs/cua-s1/training/train_4b.py --train runs/cua4b_train.jsonl \
        --out runs/cua4b_lora --modality text --epochs 3
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# `cua-bench-s1` is a sibling package (see libs/cua-bench-s1/python) declared
# as a dependency of the `four-b-train` extra in libs/cua-s1/python/pyproject
# .toml (a uv path source). Fall back to a direct sys.path insert of its
# source tree so this script and its tests also work in an environment that
# only installed cua-s1's base/four-b dependencies.
try:
    from cua_bench_s1.task import CuaTask, load_jsonl
except ImportError:
    _BENCH_S1_SRC = Path(__file__).resolve().parents[2] / "cua-bench-s1" / "python" / "src"
    sys.path.insert(0, str(_BENCH_S1_SRC))
    from cua_bench_s1.task import CuaTask, load_jsonl  # noqa: E402

from cua_s1.four_b import DEFAULT_BASE_MODEL, Option, assign_letters, build_prompt

# Standard Qwen-family LLM attention + MLP projection layer names.
LLM_LORA_TARGET_MODULES = [
    "q_proj",
    "k_proj",
    "v_proj",
    "o_proj",
    "gate_proj",
    "up_proj",
    "down_proj",
]

# Vision-projector/merger layer names for `Qwen/Qwen3.5-4B`'s vision tower
# (transformers' `models/qwen3_5/modeling_qwen3_5.py`):
#   class Qwen3_5VisionPatchMerger(nn.Module):
#       self.linear_fc1 = nn.Linear(...)
#       self.linear_fc2 = nn.Linear(...)
# instantiated as `self.merger` inside `Qwen3_5VisionModel`, which itself is
# `model.visual` on `Qwen3_5ForConditionalGeneration`/`Qwen3_5Model`. These
# names only occur on the merger, so a suffix match against them is
# unambiguous as LoRA `target_modules`.
VISION_PROJECTOR_TARGET_MODULES = ["linear_fc1", "linear_fc2"]


def _task_options(task: CuaTask) -> list[Option]:
    """Convert a `CuaTask`'s `OptionSpec` list into `cua_s1.four_b.Option`s,
    preserving order (letter assignment is order-sensitive)."""
    return [
        Option(
            element_id=o.element_id,
            role=o.role,
            label=o.label,
            action=o.action,
            entity_id=o.entity_id,
        )
        for o in task.options
    ]


def gold_option_letters(task: CuaTask, assignment) -> list[str]:
    """The letters of every option whose (element_id, action) matches the
    task's gold `expected` map.

    `CuaTask.expected` maps `element_id -> gold action`; an option is gold if
    it is the action recorded for its element. More than one option can be
    gold in the same task (e.g. two still-empty fields both need `fill` this
    turn), which is why the training target below is a set, not a single
    label.
    """
    letters = []
    for letter, option in zip(assignment.letters, assignment.options, strict=False):
        if task.expected.get(option.element_id) == option.action:
            letters.append(letter)
    return letters


def _augment_image(image):
    """Light image augmentation for multimodal training screenshots.

    GUI screenshots have very little natural pixel variation (same fonts,
    same chrome, near-identical layouts across many tasks), so without
    augmentation the vision-projector LoRA weights can trivially overfit to
    exact pixel positions instead of learning robust visual-to-token
    grounding. Kept deliberately light (small crop, no rotation/flip -- a
    flipped UI is not a valid UI).
    """
    import random

    from torchvision.transforms import functional as TF

    w, h = image.size
    # random crop: keep 90-100% of the image, re-pad back to original size
    # so image_grid_thw stays consistent per-example.
    scale = random.uniform(0.90, 1.0)
    cw, ch = int(w * scale), int(h * scale)
    if cw < w or ch < h:
        left = random.randint(0, w - cw)
        top = random.randint(0, h - ch)
        image = TF.resized_crop(image, top, left, ch, cw, [h, w])
    # mild color jitter -- brightness/contrast only, real screenshots don't
    # have natural hue variation so leave hue/saturation alone.
    image = TF.adjust_brightness(image, random.uniform(0.95, 1.05))
    image = TF.adjust_contrast(image, random.uniform(0.95, 1.05))
    return image


def build_example(
    task: CuaTask, tokenizer, modality: str = "text", processor=None, augment: bool = False
):
    """Build one training example: input_ids (+ pixel values for multimodal)
    for the prompt (ending right before the answer position) plus a soft
    target distribution over the option-letter token ids at that position.

    Returns `None` if the task has no gold option this turn (e.g. an
    all-skip state) -- nothing to supervise.
    """
    options = _task_options(task)
    assignment = assign_letters(options)
    gold_letters = gold_option_letters(task, assignment)
    if not gold_letters:
        return None

    letter_ids = []
    for letter in assignment.letters:
        toks = tokenizer.encode(letter, add_special_tokens=False)
        if len(toks) != 1:
            raise ValueError(
                f"task {task.id}: letter {letter!r} is not a single token for this tokenizer"
            )
        letter_ids.append(toks[0])

    messages = build_prompt(
        assignment,
        app=task.app,
        task_family=task.family,
        ax_tree=task.ax_tree,
        screenshot=task.screenshot,
        modality=modality,
    )

    target = [0.0] * len(letter_ids)
    for letter in gold_letters:
        target[assignment.letters.index(letter)] = 1.0 / len(gold_letters)

    if modality == "multimodal":
        if processor is None:
            raise ValueError("modality=multimodal requires a processor (image/text preprocessor)")
        from PIL import Image

        image = Image.open(task.screenshot).convert("RGB")
        if augment:
            image = _augment_image(image)

        # Pass `messages` through unflattened, matching `FourBModel.forward`:
        # `build_prompt`'s `{"type": "image", ...}` content block is what
        # makes the chat template emit the image placeholder tokens that
        # `processor(...)` then scatters the image features into. Flattening
        # to text-only here would silently drop those placeholders and leave
        # pixel values unused.
        chat_text = processor.apply_chat_template(
            messages, tokenize=False, add_generation_prompt=True
        )
        inputs = processor(text=[chat_text], images=[image], return_tensors="pt")
        mm_inputs = {k: v for k, v in inputs.items() if k != "input_ids"}
        return {
            "input_ids": inputs["input_ids"][0],
            "mm_inputs": mm_inputs,
            "letter_token_ids": letter_ids,
            "target": target,
            "task_id": task.id,
        }

    text_prompt = tokenizer.apply_chat_template(
        messages, tokenize=False, add_generation_prompt=True
    )
    input_ids = tokenizer(text_prompt, return_tensors="pt").input_ids[0]

    return {
        "input_ids": input_ids,
        "letter_token_ids": letter_ids,
        "target": target,
        "task_id": task.id,
    }


def train(args: argparse.Namespace) -> None:
    import torch
    from peft import LoraConfig, get_peft_model
    from torch.nn import functional as F
    from transformers import (
        AutoModelForCausalLM,
        AutoModelForImageTextToText,
        AutoProcessor,
        AutoTokenizer,
    )

    tokenizer = AutoTokenizer.from_pretrained(args.base_model)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    processor = None
    if args.modality == "multimodal":
        processor = AutoProcessor.from_pretrained(args.base_model)

    tasks = load_jsonl(args.train)
    examples = [
        e
        for t in tasks
        if (
            e := build_example(
                t,
                tokenizer,
                modality=args.modality,
                processor=processor,
                augment=(args.modality == "multimodal"),
            )
        )
        is not None
    ]
    if not examples:
        raise SystemExit(
            f"no trainable examples found in {args.train} (all tasks had no gold option, "
            f"or all were multimodal without --modality multimodal support)"
        )
    print(f"[train_4b] {len(examples)}/{len(tasks)} tasks yielded a trainable example")

    if args.modality == "multimodal":
        # This checkpoint's vision tower is only wired up under
        # AutoModelForImageTextToText -- AutoModelForCausalLM resolves to the
        # text-only model class, which drops every `model.visual.*` weight.
        model = AutoModelForImageTextToText.from_pretrained(
            args.base_model, torch_dtype=torch.bfloat16, device_map="cuda"
        )
        target_modules = LLM_LORA_TARGET_MODULES + VISION_PROJECTOR_TARGET_MODULES
    else:
        model = AutoModelForCausalLM.from_pretrained(
            args.base_model, torch_dtype=torch.bfloat16, device_map="cuda"
        )
        target_modules = LLM_LORA_TARGET_MODULES

    lora_config = LoraConfig(
        r=args.lora_r,
        lora_alpha=args.lora_alpha,
        lora_dropout=0.05,
        target_modules=target_modules,
        task_type="CAUSAL_LM",
    )
    model = get_peft_model(model, lora_config)
    model.print_trainable_parameters()
    model.train()

    # Two-parameter-group optimizer with a differential learning rate: the
    # vision-projector LoRA params get a lower LR than the LLM-side LoRA
    # params, since the projector is a much smaller, more fragile module
    # bridging two pretrained towers -- a full LR there risks destroying the
    # pretrained visual-to-token alignment rather than refining it.
    vision_lr = args.vision_lr if args.vision_lr is not None else args.lr / 4.0
    vision_params, llm_params = [], []
    for name, p in model.named_parameters():
        if not p.requires_grad:
            continue
        if any(vm in name for vm in VISION_PROJECTOR_TARGET_MODULES):
            vision_params.append(p)
        else:
            llm_params.append(p)
    param_groups = [{"params": llm_params, "lr": args.lr}]
    if vision_params:
        param_groups.append({"params": vision_params, "lr": vision_lr})
        print(
            f"[train_4b] differential LR: {len(llm_params)} LLM-LoRA params @ lr={args.lr}, "
            f"{len(vision_params)} vision-projector-LoRA params @ lr={vision_lr}"
        )
    optim = torch.optim.AdamW(param_groups)

    for epoch in range(args.epochs):
        total_loss = 0.0
        for ex in examples:
            input_ids = ex["input_ids"].unsqueeze(0).to(model.device)
            fwd_kwargs = {"input_ids": input_ids}
            for k, v in ex.get("mm_inputs", {}).items():
                v = v.to(model.device)
                if k == "pixel_values":
                    v = v.to(dtype=model.dtype)
                fwd_kwargs[k] = v
            out = model(**fwd_kwargs)
            final_logits = out.logits[0, -1, :]
            option_logits = final_logits[
                torch.tensor(ex["letter_token_ids"], device=final_logits.device)
            ]
            log_probs = F.log_softmax(option_logits.float(), dim=-1)
            target = torch.tensor(ex["target"], device=log_probs.device, dtype=log_probs.dtype)
            loss = -(target * log_probs).sum()  # soft-label cross-entropy

            optim.zero_grad()
            loss.backward()
            optim.step()
            total_loss += loss.item()
        print(
            f"[train_4b] epoch {epoch + 1}/{args.epochs} mean loss = {total_loss / len(examples):.4f}"
        )

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    # Standard PEFT on-disk layout (adapter_config.json + adapter_model
    # .safetensors) -- this is what `FourBModel(lora_adapter_path=...)`
    # already expects to load via `peft.PeftModel.from_pretrained`.
    model.save_pretrained(str(out_dir))
    tokenizer.save_pretrained(str(out_dir))
    (out_dir / "train_config.json").write_text(
        json.dumps(vars(args), default=str, indent=2), encoding="utf-8"
    )
    print(f"[train_4b] saved LoRA adapter to {out_dir}")


def main() -> None:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--train", required=True, help="path to a CuaTask jsonl training split")
    p.add_argument("--out", required=True, help="output directory for the LoRA adapter")
    p.add_argument("--base-model", default=DEFAULT_BASE_MODEL)
    p.add_argument("--modality", choices=["text", "multimodal"], default="text")
    p.add_argument("--epochs", type=int, default=3)
    p.add_argument("--lr", type=float, default=1e-4)
    p.add_argument(
        "--vision-lr",
        type=float,
        default=None,
        help="LR for vision-projector LoRA params (multimodal only); default lr/4",
    )
    p.add_argument("--lora-r", type=int, default=16)
    p.add_argument("--lora-alpha", type=int, default=32)
    args = p.parse_args()

    train(args)


if __name__ == "__main__":
    main()
