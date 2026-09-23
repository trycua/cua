#!/usr/bin/env python3
"""LoRA fine-tune `Qwen/Qwen3.5-4B` into a `cua-s1-4b` adapter -- v2 recipe.

This is a separate script from `train_4b.py`, which stays the recipe that
`cua-s1-4b-0.1` was trained with and is documented against. The two recipes
differ in four places:

1. PER-ELEMENT LOSS. `cua_bench_s1.eval.scoring.score_task` scores a task by
   taking, FOR EACH ELEMENT SEPARATELY, the argmax over just that element's
   own candidate actions, and marks the task correct only if every element's
   argmax matches gold. The loss here is therefore the mean of per-element
   cross-entropies over each element's own option logits -- exactly the
   quantity the scorer thresholds.

   This matters because most elements on a GUI screen are uncontested: in a
   16-element task, ~15 elements typically offer `skip` as their only
   candidate action, and an element with one option is correct by
   construction. Scoping the softmax to one element makes those contribute
   identically zero gradient (a softmax over a single logit is 1.0 regardless
   of parameters), so all of the signal lands on the two or three decisions
   that actually determine the score. An element with more than one gold
   action still gets uniform target mass over them, scoped to that element.

2. PER-EPOCH SHUFFLING, off a seeded RNG so runs stay reproducible. A
   training file is typically grouped by source, and stepping through it in
   file order at an effective batch size of 1 makes the end of every epoch a
   single source's data.

3. GRADIENT ACCUMULATION over `--batch-size` examples, with gradient-norm
   clipping, plus a linear warmup into a cosine decay rather than a flat
   learning rate.

4. VALIDATION-SELECTED CHECKPOINTING. Real task accuracy on `--val` is
   measured after every epoch using the eval-time readout (per-element
   argmax, task correct only if every element is), and the best epoch's
   adapter is what lands on disk. `--val` must be a validation split, never
   the held-out test split; the script refuses an obvious test path.

`--modality multimodal` trains the same objective against screenshots. It
adds the vision projector to the LoRA target modules, gives those parameters
a lower learning rate (a small module bridging two pretrained towers, where a
full learning rate damages the existing visual-to-token alignment rather than
refining it), and lightly augments each screenshot. Multimodal examples are
built lazily -- only the image path is held, and the processor runs per step
-- so resident memory stays at one image's tensors rather than the whole
split's decoded `pixel_values`.

Usage:
    python libs/cua-s1/training/train_4b_v2.py --train runs/train.jsonl \
        --val runs/val.jsonl --out runs/cua4b_v2_lora --modality text
"""

from __future__ import annotations

import argparse
import json
import math
import random
import sys
from collections import defaultdict
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

# Vision-projector/merger layer names for `Qwen/Qwen3.5-4B`'s vision tower:
# `Qwen3_5VisionPatchMerger`'s `linear_fc1`/`linear_fc2`. These names only
# occur on the merger, so a suffix match against them is unambiguous as LoRA
# `target_modules`.
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


def element_groups(task: CuaTask, assignment) -> list[tuple[list[int], list[int]]]:
    """The per-element option grouping the loss and the readout both need.

    One `(option_indices, gold_positions)` pair per CONTESTED element, where
    `option_indices` index into the assignment's option list and
    `gold_positions` index into `option_indices`.

    Elements with a single candidate action are dropped: a softmax over one
    logit is 1.0 regardless of parameters, so they are provably zero-gradient,
    and the scorer marks them correct unconditionally. An element whose gold
    action is not among its own options is dropped too -- that element is
    malformed (the same condition `eval.adapter.OracleAdapter` raises on), but
    the task's other elements are still valid supervision.
    """
    by_element: dict[str, list[int]] = defaultdict(list)
    for i, option in enumerate(assignment.options):
        by_element[option.element_id].append(i)

    groups = []
    for element_id, idxs in by_element.items():
        if len(idxs) < 2:
            continue
        gold_action = task.expected.get(element_id)
        gold_positions = [
            j for j, i in enumerate(idxs) if assignment.options[i].action == gold_action
        ]
        if not gold_positions:
            continue
        groups.append((idxs, gold_positions))
    return groups


def _letter_token_ids(assignment, tokenizer, task_id: str) -> list[int]:
    letter_ids = []
    for letter in assignment.letters:
        toks = tokenizer.encode(letter, add_special_tokens=False)
        if len(toks) != 1:
            raise ValueError(
                f"task {task_id}: letter {letter!r} is not a single token for this tokenizer"
            )
        letter_ids.append(toks[0])
    return letter_ids


def plan_example(task: CuaTask, tokenizer, modality: str = "text", data_root: Path | None = None):
    """Everything about one training example that does not require decoding an
    image: the option-letter token ids, the per-element grouping, and either
    the tokenized text prompt or the screenshot path to load per step.

    Returns `None` when the task has no contested element to supervise, or
    (multimodal) when its screenshot is missing from disk.
    """
    options = _task_options(task)
    assignment = assign_letters(options)
    groups = element_groups(task, assignment)
    if not groups:
        return None

    letter_ids = _letter_token_ids(assignment, tokenizer, task.id)
    plan = {
        "task": task,
        "assignment": assignment,
        "letter_token_ids": letter_ids,
        "groups": groups,
        "task_id": task.id,
    }

    if modality == "multimodal":
        image_path = Path(task.screenshot) if task.screenshot else None
        if image_path is None:
            return None
        if not image_path.is_absolute() and data_root is not None:
            image_path = data_root / task.screenshot
        if not image_path.exists():
            return None
        plan["image_path"] = image_path
        return plan

    messages = build_prompt(
        assignment,
        app=task.app,
        task_family=task.family,
        ax_tree=task.ax_tree,
        modality="text",
        goal=task.goal,
    )
    text_prompt = tokenizer.apply_chat_template(
        messages, tokenize=False, add_generation_prompt=True
    )
    plan["input_ids"] = tokenizer(text_prompt, return_tensors="pt").input_ids[0]
    return plan


def _augment_image(image, rng: random.Random):
    """Light augmentation: a small crop resized back to the original size,
    plus mild brightness/contrast. No flips or rotations -- a mirrored UI is
    not a valid UI -- and no hue jitter, since screenshots have no natural hue
    variation. GUI screenshots vary so little pixel-to-pixel that without this
    the vision-projector LoRA weights can overfit to exact pixel positions
    instead of learning visual-to-token grounding.
    """
    from torchvision.transforms import functional as TF

    w, h = image.size
    scale = rng.uniform(0.90, 1.0)
    cw, ch = int(w * scale), int(h * scale)
    if cw < w or ch < h:
        left = rng.randint(0, w - cw)
        top = rng.randint(0, h - ch)
        image = TF.resized_crop(image, top, left, ch, cw, [h, w])
    image = TF.adjust_brightness(image, rng.uniform(0.95, 1.05))
    image = TF.adjust_contrast(image, rng.uniform(0.95, 1.05))
    return image


def materialize(plan, processor, augment: bool, rng: random.Random) -> dict:
    """The model inputs for one multimodal example. Called per step, never
    cached: caching would hold every example's decoded `pixel_values`
    resident at once.

    `messages` is passed through unflattened, matching `FourBModel.forward`:
    `build_prompt`'s `{"type": "image", ...}` content block is what makes the
    chat template emit the image placeholder tokens that `processor(...)`
    scatters the image features into. Every key the processor produced is
    kept, since this model's M-RoPE path needs `mm_token_type_ids` alongside
    `image_grid_thw`.
    """
    from PIL import Image

    task = plan["task"]
    image = Image.open(plan["image_path"]).convert("RGB")
    if augment:
        image = _augment_image(image, rng)
    messages = build_prompt(
        plan["assignment"],
        app=task.app,
        task_family=task.family,
        screenshot=str(plan["image_path"]),
        modality="multimodal",
        goal=task.goal,
    )
    chat_text = processor.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)
    return dict(processor(text=[chat_text], images=[image], return_tensors="pt"))


def group_log_probs(model, plan, inputs, torch, F) -> list:
    """Per-element log-softmax over each element's own option logits, read off
    the final sequence position -- the same readout used at eval time."""
    fwd = {}
    for k, v in inputs.items():
        v = v.to(model.device)
        if k == "pixel_values":
            v = v.to(dtype=model.dtype)
        fwd[k] = v
    out = model(**fwd)
    final_logits = out.logits[0, -1, :]
    option_logits = final_logits[
        torch.tensor(plan["letter_token_ids"], device=final_logits.device)
    ].float()
    return [
        F.log_softmax(option_logits[torch.tensor(idxs, device=option_logits.device)], dim=-1)
        for idxs, _ in plan["groups"]
    ]


def example_loss(model, plan, inputs, torch, F):
    """Mean over contested elements of each element's cross-entropy, with
    uniform target mass over that element's gold action(s)."""
    log_probs = group_log_probs(model, plan, inputs, torch, F)
    loss = None
    for lp, (_, gold_positions) in zip(log_probs, plan["groups"], strict=False):
        term = -lp[torch.tensor(gold_positions, device=lp.device)].mean()
        loss = term if loss is None else loss + term
    return loss / len(plan["groups"])


def _plan_inputs(plan, processor, augment, rng) -> dict:
    if "input_ids" in plan:
        return {"input_ids": plan["input_ids"].unsqueeze(0)}
    return materialize(plan, processor, augment=augment, rng=rng)


def evaluate_val(model, plans, torch, F, processor=None) -> float:
    """Real task accuracy over `plans` using the eval-time readout: each
    element's argmax must match one of that element's gold actions, and the
    task counts only if every element does. No augmentation."""
    if not plans:
        return 0.0
    model.eval()
    rng = random.Random(0)
    correct = 0
    with torch.no_grad():
        for plan in plans:
            inputs = _plan_inputs(plan, processor, augment=False, rng=rng)
            log_probs = group_log_probs(model, plan, inputs, torch, F)
            correct += int(
                all(
                    int(lp.argmax().item()) in set(gold_positions)
                    for lp, (_, gold_positions) in zip(log_probs, plan["groups"], strict=False)
                )
            )
    model.train()
    return correct / len(plans)


def lr_schedule(
    step: int, *, base_lr: float, min_lr: float, warmup: int, total_steps: int
) -> float:
    """Linear warmup into a cosine decay from `base_lr` down to `min_lr`."""
    if step < warmup:
        return base_lr * (step + 1) / warmup
    progress = (step - warmup) / max(1, total_steps - warmup)
    return min_lr + (base_lr - min_lr) * 0.5 * (1 + math.cos(math.pi * progress))


def check_val_split(val_path: str) -> None:
    """Refuse a `--val` that is obviously the held-out test split. Selecting a
    checkpoint on the test split is selection pressure on it, which silently
    turns a held-out number into a fitted one."""
    if Path(val_path).name.startswith("test"):
        raise SystemExit(
            f"--val={val_path!r} looks like a held-out test split. Epoch selection must never "
            f"see it; point --val at a validation split."
        )


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

    check_val_split(args.val)
    multimodal = args.modality == "multimodal"
    data_root = Path(args.data_root) if args.data_root else Path(args.train).parent

    tokenizer = AutoTokenizer.from_pretrained(args.base_model)
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token
    processor = AutoProcessor.from_pretrained(args.base_model) if multimodal else None

    def plans_for(path: str) -> list:
        return [
            p
            for t in load_jsonl(path)
            if (p := plan_example(t, tokenizer, modality=args.modality, data_root=data_root))
            is not None
        ]

    train_plans = plans_for(args.train)
    val_plans = plans_for(args.val)
    if not train_plans:
        raise SystemExit(
            f"no trainable examples found in {args.train} (no task had a contested element)"
        )
    contested = sum(len(p["groups"]) for p in train_plans) / len(train_plans)
    print(
        f"[train_4b_v2] {len(train_plans)} train / {len(val_plans)} val examples; "
        f"{contested:.2f} contested elements per task"
    )

    if multimodal:
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

    model = get_peft_model(
        model,
        LoraConfig(
            r=args.lora_r,
            lora_alpha=args.lora_alpha,
            lora_dropout=args.lora_dropout,
            target_modules=target_modules,
            task_type="CAUSAL_LM",
        ),
    )
    model.print_trainable_parameters()
    model.train()

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
            f"[train_4b_v2] differential LR: {len(llm_params)} LLM-LoRA params @ lr={args.lr}, "
            f"{len(vision_params)} vision-projector-LoRA params @ lr={vision_lr}"
        )
    optim = torch.optim.AdamW(param_groups, weight_decay=args.weight_decay)
    trainable = llm_params + vision_params
    base_lrs = [g["lr"] for g in optim.param_groups]

    steps_per_epoch = math.ceil(len(train_plans) / args.batch_size)
    total_steps = steps_per_epoch * args.epochs
    warmup = max(1, int(total_steps * args.warmup_frac))

    rng = random.Random(args.seed)
    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    history, best_acc, global_step = [], -1.0, 0

    for epoch in range(args.epochs):
        order = list(range(len(train_plans)))
        rng.shuffle(order)
        total_loss, n_seen = 0.0, 0
        optim.zero_grad()
        for k, idx in enumerate(order):
            plan = train_plans[idx]
            inputs = _plan_inputs(plan, processor, augment=(multimodal and args.augment), rng=rng)
            loss = example_loss(model, plan, inputs, torch, F)
            (loss / args.batch_size).backward()
            total_loss += float(loss.item())
            n_seen += 1
            del inputs, loss
            if (k + 1) % args.batch_size == 0 or (k + 1) == len(order):
                for group, base in zip(optim.param_groups, base_lrs, strict=False):
                    group["lr"] = lr_schedule(
                        global_step,
                        base_lr=base,
                        min_lr=args.min_lr,
                        warmup=warmup,
                        total_steps=total_steps,
                    )
                torch.nn.utils.clip_grad_norm_(trainable, args.max_grad_norm)
                optim.step()
                optim.zero_grad()
                global_step += 1
                if multimodal:
                    torch.cuda.empty_cache()

        mean_loss = total_loss / max(1, n_seen)
        val_acc = evaluate_val(model, val_plans, torch, F, processor=processor)
        history.append({"epoch": epoch + 1, "mean_loss": mean_loss, "val_accuracy": val_acc})
        print(
            f"[train_4b_v2] epoch {epoch + 1}/{args.epochs} loss={mean_loss:.4f} val_acc={val_acc:.4f}"
        )
        if val_acc > best_acc:
            best_acc = val_acc
            # Standard PEFT on-disk layout (adapter_config.json +
            # adapter_model.safetensors) -- what `FourBModel(lora_adapter_path=...)`
            # already expects to load via `peft.PeftModel.from_pretrained`.
            model.save_pretrained(str(out_dir))
            tokenizer.save_pretrained(str(out_dir))
            print(f"[train_4b_v2]   new best val_acc={val_acc:.4f} -> saved to {out_dir}")

    (out_dir / "train_config.json").write_text(
        json.dumps(
            {**vars(args), "best_val_accuracy": best_acc, "history": history}, default=str, indent=2
        ),
        encoding="utf-8",
    )
    print(f"[train_4b_v2] done. best val_acc={best_acc:.4f}, adapter at {out_dir}")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--train", required=True, help="path to a CuaTask jsonl training split")
    p.add_argument(
        "--val",
        required=True,
        help="validation split used to select the saved epoch -- never the test split",
    )
    p.add_argument("--out", required=True, help="output directory for the LoRA adapter")
    p.add_argument(
        "--data-root",
        default=None,
        help="root that relative screenshot paths resolve against; defaults to --train's directory",
    )
    p.add_argument("--base-model", default=DEFAULT_BASE_MODEL)
    p.add_argument("--modality", choices=["text", "multimodal"], default="text")
    p.add_argument("--epochs", type=int, default=4)
    p.add_argument("--lr", type=float, default=1e-4)
    p.add_argument("--min-lr", type=float, default=0.0)
    p.add_argument(
        "--vision-lr",
        type=float,
        default=None,
        help="LR for vision-projector LoRA params (multimodal only); default lr/4",
    )
    p.add_argument("--weight-decay", type=float, default=0.0)
    p.add_argument("--warmup-frac", type=float, default=0.05)
    p.add_argument("--batch-size", type=int, default=8, help="gradient-accumulation batch size")
    p.add_argument("--max-grad-norm", type=float, default=1.0)
    p.add_argument("--lora-r", type=int, default=16)
    p.add_argument("--lora-alpha", type=int, default=32)
    p.add_argument("--lora-dropout", type=float, default=0.05)
    p.add_argument(
        "--augment",
        action="store_true",
        default=True,
        help="light screenshot augmentation (multimodal only)",
    )
    p.add_argument("--no-augment", dest="augment", action="store_false")
    p.add_argument("--seed", type=int, default=0)
    return p


def main() -> None:
    train(build_parser().parse_args())


if __name__ == "__main__":
    main()
