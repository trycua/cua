"""Train (or fine-tune) `cua-s1-nano`: a from-scratch option-attention scorer
for computer-use GUI decisions, built on `cua_s1.nano`.

This script is meant to be run with both `cua-s1` and `cua-bench-s1` installed
side by side (the latter is a training/data dependency, not a runtime
dependency of `cua_s1.nano` itself -- see the `training` extra in
`libs/cua-s1/python/pyproject.toml`).

Two stages, mirroring the checkpoint's own two-stage recipe:

  1. Base training on a prepared bulk split (see `nano_data.py` for how to
     build one from a `cua_bench_s1.task.CuaTask` jsonl file):
       python train_nano.py base --data data/nano-mm \\
           --modality multimodal --output runs/nano-mm

  2. Optional fine-tune of a base checkpoint on a smaller split (same
     on-disk prepared format), warm-starting from an existing checkpoint at
     a lower learning rate:
       python train_nano.py finetune --data data/nano-mm-live \\
           --modality multimodal --checkpoint-in runs/nano-mm \\
           --checkpoint-out runs/nano-mm-live --epochs 10 --learning-rate 3e-4

Unlike the internal research harness this was ported from, this script has no
GPU-sharing lock: it assumes it owns whatever device it's given, which is the
right assumption for a public, single-job training script.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from collections import defaultdict
from pathlib import Path

import torch
from torch.nn import functional as F
from torch.utils.data import DataLoader

sys.path.insert(0, str(Path(__file__).resolve().parent))

from nano_data import NanoPreparedCollator, NanoPreparedDataset  # noqa: E402

from cua_s1.nano import (  # noqa: E402
    NanoScorer,
    load_nano_checkpoint,
    make_nano_system,
    parameter_count,
    save_nano_checkpoint,
    select_device,
)

DEFAULT_OUTPUT = Path("runs") / "cua-s1-nano" / "base"


def move(batch: dict, device: torch.device) -> dict:
    return {k: (v.to(device) if isinstance(v, torch.Tensor) else v) for k, v in batch.items()}


@torch.no_grad()
def evaluate(
    model: NanoScorer,
    dataset: NanoPreparedDataset,
    collator: NanoPreparedCollator,
    device: torch.device,
    batch_size: int = 256,
) -> dict:
    model.eval()
    loader = DataLoader(dataset, batch_size=batch_size, collate_fn=collator)
    per_action: dict[str, list[int]] = defaultdict(lambda: [0, 0])
    total_nll, total, correct = 0.0, 0, 0
    offset = 0
    for host in loader:
        batch = move(host, device)
        logits = model(batch)
        total_nll += float(F.cross_entropy(logits, batch["labels"], reduction="sum"))
        prediction = logits.argmax(-1)
        ok = prediction.eq(batch["labels"])
        correct += int(ok.sum())
        n = batch["labels"].numel()
        total += n
        for j in range(n):
            action = dataset.rows[offset + j]["meta"]["action"]
            per_action[action][0] += int(ok[j])
            per_action[action][1] += 1
        offset += n
    return {
        "top1": correct / total,
        "nll": total_nll / total,
        "examples": total,
        "per_action": {a: round(c / t, 4) for a, (c, t) in sorted(per_action.items())},
    }


def run_training(
    model: NanoScorer,
    train_set: NanoPreparedDataset,
    val_set: NanoPreparedDataset,
    collator: NanoPreparedCollator,
    device: torch.device,
    epochs: int,
    batch_size: int,
    learning_rate: float,
    output: str | Path,
    extra_metadata: dict,
) -> dict:
    train_loader = DataLoader(train_set, batch_size=batch_size, shuffle=True, collate_fn=collator)
    print(
        json.dumps(
            {
                "trainable_parameters": parameter_count(model),
                "train_rows": len(train_set),
                "val_rows": len(val_set),
            }
        )
    )
    optimizer = torch.optim.AdamW(model.parameters(), lr=learning_rate, weight_decay=1e-2)
    scheduler = torch.optim.lr_scheduler.CosineAnnealingLR(
        optimizer, T_max=max(1, epochs * len(train_loader))
    )

    before = evaluate(model, val_set, collator, device, batch_size)
    print("BEFORE:", json.dumps(before))
    best_nll = before["nll"]
    best_state = {k: v.detach().cpu().clone() for k, v in model.state_dict().items()}
    best_epoch = 0
    start = time.perf_counter()
    for epoch in range(epochs):
        model.train()
        total, count = 0.0, 0
        for host in train_loader:
            batch = move(host, device)
            loss = F.cross_entropy(model(batch), batch["labels"])
            optimizer.zero_grad(set_to_none=True)
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()
            scheduler.step()
            total += float(loss.detach()) * batch["labels"].numel()
            count += batch["labels"].numel()
        val = evaluate(model, val_set, collator, device, batch_size)
        record = {
            "epoch": epoch + 1,
            "train_nll": total / max(count, 1),
            "val_nll": val["nll"],
            "val_top1": val["top1"],
            "val_per_action": val["per_action"],
            "seconds": round(time.perf_counter() - start, 1),
        }
        print(json.dumps(record))
        if val["nll"] < best_nll:
            best_nll = val["nll"]
            best_state = {k: v.detach().cpu().clone() for k, v in model.state_dict().items()}
            best_epoch = epoch + 1
    model.load_state_dict(best_state)
    config = {
        "width": model.width,
        "rank": model.rank,
        "option_tokens": model.option_tokens,
        "context_tokens": model.context_tokens,
        "vision_dim": model.vision_dim,
    }
    weights_path, _ = save_nano_checkpoint(
        output,
        model,
        config,
        {
            "best_val_nll": best_nll,
            "best_epoch": best_epoch,
            "trainable_parameters": parameter_count(model),
            **extra_metadata,
        },
    )
    summary = {
        "checkpoint": str(weights_path),
        "best_val_nll": best_nll,
        "best_epoch": best_epoch,
        "train_seconds": round(time.perf_counter() - start, 1),
    }
    print(json.dumps(summary))
    return summary


def cmd_base(args: argparse.Namespace) -> None:
    device = select_device(args.device)
    torch.manual_seed(args.seed)
    config = {
        "width": args.width,
        "rank": args.rank,
        "option_tokens": args.option_tokens,
        "context_tokens": args.context_tokens,
    }
    model, _ = make_nano_system(config, device)
    collator = NanoPreparedCollator(args.modality, args.option_tokens, args.context_tokens)
    train_set = NanoPreparedDataset(args.data / "train.jsonl", args.modality)
    val_set = NanoPreparedDataset(args.data / "validation.jsonl", args.modality)
    run_training(
        model,
        train_set,
        val_set,
        collator,
        device,
        args.epochs,
        args.batch_size,
        args.learning_rate,
        args.output,
        {"stage": "base", "modality": args.modality},
    )


def cmd_finetune(args: argparse.Namespace) -> None:
    device = select_device(args.device)
    torch.manual_seed(args.seed)
    model, _collator, config = load_nano_checkpoint(args.checkpoint_in, device)
    collator = NanoPreparedCollator(
        args.modality, config["option_tokens"], config.get("context_tokens", 256)
    )
    train_set = NanoPreparedDataset(args.data / "train.jsonl", args.modality)
    val_set = NanoPreparedDataset(args.data / "validation.jsonl", args.modality)
    run_training(
        model,
        train_set,
        val_set,
        collator,
        device,
        args.epochs,
        args.batch_size,
        args.learning_rate,
        args.checkpoint_out,
        {"stage": "finetune", "modality": args.modality, "warm_start_from": str(args.checkpoint_in)},
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="stage", required=True)

    base = sub.add_parser("base", help="base training on a prepared bulk split")
    base.add_argument(
        "--data", type=Path, required=True, help="dir with train.jsonl/validation.jsonl (+ .features.pt)"
    )
    base.add_argument("--modality", choices=["text", "multimodal"], required=True)
    base.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    base.add_argument("--width", type=int, default=128)
    base.add_argument("--rank", type=int, default=128)
    base.add_argument("--option-tokens", type=int, default=96)
    base.add_argument("--context-tokens", type=int, default=256)
    base.add_argument("--epochs", type=int, default=25)
    base.add_argument("--batch-size", type=int, default=256)
    base.add_argument("--learning-rate", type=float, default=3e-3)
    base.add_argument("--device", default="auto")
    base.add_argument("--seed", type=int, default=7)
    base.set_defaults(func=cmd_base)

    finetune = sub.add_parser("finetune", help="warm-start fine-tune of a base checkpoint on a small split")
    finetune.add_argument("--data", type=Path, required=True)
    finetune.add_argument("--modality", choices=["text", "multimodal"], required=True)
    finetune.add_argument("--checkpoint-in", type=Path, required=True)
    finetune.add_argument("--checkpoint-out", type=Path, required=True)
    finetune.add_argument("--epochs", type=int, default=10)
    finetune.add_argument("--batch-size", type=int, default=64)
    finetune.add_argument("--learning-rate", type=float, default=3e-4)
    finetune.add_argument("--device", default="auto")
    finetune.add_argument("--seed", type=int, default=7)
    finetune.set_defaults(func=cmd_finetune)

    return parser


def main() -> None:
    parser = build_parser()
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
