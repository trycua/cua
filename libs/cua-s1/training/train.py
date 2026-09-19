"""Train and evaluate the Cua-S1 form-specialist choice scorer."""

from __future__ import annotations

import argparse
import json
import math
import random
import time
from collections import defaultdict
from collections.abc import Mapping
from pathlib import Path
from typing import Callable

import torch
from torch.nn import functional as F
from torch.utils.data import DataLoader, Dataset

from cua_s1.model import (
    ChoiceExample,
    load_checkpoint,
    make_system,
    parameter_count,
    save_checkpoint,
    select_device,
    trainable_state,
    validate_example,
)
from cua_s1.schema import FIXED_ACTIONS


class MetaDataset(Dataset):
    """Validated JSONL choices with action labels retained for metrics."""

    def __init__(self, path: str | Path) -> None:
        self.path = Path(path)
        self.examples: list[ChoiceExample] = []
        self.actions: list[str] = []
        with self.path.open(encoding="utf-8") as handle:
            for line_number, line in enumerate(handle, start=1):
                if not line.strip():
                    continue
                try:
                    payload = json.loads(line)
                    example = validate_example(payload)
                except (AttributeError, json.JSONDecodeError, TypeError, ValueError) as error:
                    raise ValueError(f"invalid example at {self.path}:{line_number}") from error
                self.examples.append(example)
                metadata = payload.get("meta")
                action = metadata.get("action") if isinstance(metadata, Mapping) else None
                self.actions.append(action or action_name(example))

    def __len__(self) -> int:
        return len(self.examples)

    def __getitem__(self, index: int) -> ChoiceExample:
        return self.examples[index]


def action_name(example: ChoiceExample) -> str:
    option = example.options[example.label]
    return option if option in FIXED_ACTIONS else "fill"


def move(batch: dict, device: torch.device) -> dict:
    return {key: value.to(device) for key, value in batch.items()}


def _seed_worker(_worker_id: int) -> None:
    random.seed(torch.initial_seed() % 2**32)


def configure_determinism(seed: int, deterministic: bool) -> torch.Generator:
    """Seed model and loader randomness and configure deterministic kernels."""

    random.seed(seed)
    torch.manual_seed(seed)
    if torch.cuda.is_available():
        torch.cuda.manual_seed_all(seed)
    torch.use_deterministic_algorithms(deterministic)
    if hasattr(torch.backends, "cudnn"):
        torch.backends.cudnn.benchmark = not deterministic
        torch.backends.cudnn.deterministic = deterministic
    generator = torch.Generator()
    generator.manual_seed(seed)
    return generator


@torch.no_grad()
def evaluate(
    model,
    dataset: MetaDataset,
    collator,
    device: torch.device,
    batch_size: int = 256,
    shuffle_context: bool = False,
) -> dict:
    """Report aggregate, per-action, calibration, and throughput metrics."""

    if not dataset:
        raise ValueError(f"evaluation dataset is empty: {dataset.path}")
    if batch_size <= 0:
        raise ValueError("batch_size must be positive")
    model.eval()
    loader = DataLoader(dataset, batch_size=batch_size, collate_fn=collator)
    per_action: dict[str, list[int]] = defaultdict(lambda: [0, 0])
    confidences: list[torch.Tensor] = []
    corrects: list[torch.Tensor] = []
    nll_total = 0.0
    offset = 0
    start = time.perf_counter()
    for host_batch in loader:
        batch = move(host_batch, device)
        logits = model(batch, shuffle_context=shuffle_context)
        nll_total += float(F.cross_entropy(logits, batch["labels"], reduction="sum"))
        probabilities = logits.softmax(-1)
        confidence, prediction = probabilities.max(-1)
        correct = prediction.eq(batch["labels"])
        for row_index in range(correct.shape[0]):
            action = dataset.actions[offset + row_index]
            per_action[action][0] += int(correct[row_index])
            per_action[action][1] += 1
        offset += correct.shape[0]
        confidences.append(confidence.cpu())
        corrects.append(correct.cpu())

    elapsed = max(time.perf_counter() - start, 1e-12)
    confidence = torch.cat(confidences)
    correct = torch.cat(corrects).float()
    expected_calibration_error = 0.0
    for lower_bound in torch.linspace(0, 0.9, 10):
        selected = (confidence >= lower_bound) & (confidence < lower_bound + 0.1)
        if selected.any():
            expected_calibration_error += float(
                selected.float().mean()
                * (correct[selected].mean() - confidence[selected].mean()).abs()
            )
    example_count = len(dataset)
    return {
        "top1": float(correct.mean()),
        "nll": nll_total / example_count,
        "ece": expected_calibration_error,
        "examples": example_count,
        "per_action": {
            action: {"acc": correct_count / total, "n": total}
            for action, (correct_count, total) in sorted(per_action.items())
        },
        "rows_per_second": example_count / elapsed,
    }


def _stable_metrics(metrics: dict) -> dict:
    return {key: value for key, value in metrics.items() if key != "rows_per_second"}


def _clone_trainable_state(model) -> dict[str, torch.Tensor]:
    return {key: value.detach().cpu().clone() for key, value in trainable_state(model).items()}


def train(
    config: dict,
    train_path: str | Path,
    validation_path: str | Path,
    output: str | Path,
    *,
    epochs: int = 10,
    batch_size: int = 128,
    learning_rate: float = 2e-3,
    seed: int = 7,
    device_name: str = "auto",
    warmup: float = 0.05,
    workers: int = 0,
    deterministic: bool = True,
    log: Callable[[str], None] = print,
) -> dict:
    """Train from scratch and save the best validation checkpoint safely."""

    if epochs <= 0 or batch_size <= 0:
        raise ValueError("epochs and batch_size must be positive")
    if learning_rate <= 0:
        raise ValueError("learning_rate must be positive")
    if warmup < 0 or warmup > 1:
        raise ValueError("warmup must be between 0 and 1")
    if workers < 0:
        raise ValueError("workers must be non-negative")

    loader_generator = configure_determinism(seed, deterministic)
    device = select_device(device_name)
    model, collator = make_system(config, device)
    train_set = MetaDataset(train_path)
    validation_set = MetaDataset(validation_path)
    if not train_set:
        raise ValueError(f"training dataset is empty: {train_set.path}")
    if not validation_set:
        raise ValueError(f"validation dataset is empty: {validation_set.path}")

    loader = DataLoader(
        train_set,
        batch_size=batch_size,
        shuffle=True,
        collate_fn=collator,
        drop_last=False,
        num_workers=workers,
        persistent_workers=workers > 0,
        generator=loader_generator,
        worker_init_fn=_seed_worker if workers else None,
    )
    parameters = [parameter for parameter in model.parameters() if parameter.requires_grad]
    if not parameters:
        raise ValueError("model has no trainable parameters")
    optimizer = torch.optim.AdamW(parameters, lr=learning_rate, weight_decay=1e-2)
    total_steps = epochs * len(loader)
    warmup_steps = int(total_steps * warmup)

    def learning_rate_multiplier(step: int) -> float:
        if warmup_steps and step < warmup_steps:
            return (step + 1) / warmup_steps
        progress = (step - warmup_steps) / max(1, total_steps - warmup_steps)
        return 0.5 * (1 + math.cos(math.pi * min(progress, 1.0)))

    scheduler = torch.optim.lr_scheduler.LambdaLR(optimizer, learning_rate_multiplier)
    best_validation: dict = {"nll": float("inf")}
    best_state: dict[str, torch.Tensor] | None = None
    history: list[dict] = []
    started = time.perf_counter()

    for epoch in range(epochs):
        model.train()
        total_loss = 0.0
        example_count = 0
        for host_batch in loader:
            batch = move(host_batch, device)
            loss = F.cross_entropy(model(batch), batch["labels"])
            optimizer.zero_grad(set_to_none=True)
            loss.backward()
            torch.nn.utils.clip_grad_norm_(parameters, 1.0)
            optimizer.step()
            scheduler.step()
            batch_examples = batch["labels"].numel()
            total_loss += float(loss.detach()) * batch_examples
            example_count += batch_examples

        validation = evaluate(model, validation_set, collator, device)
        stable_validation = _stable_metrics(validation)
        record = {
            "epoch": epoch + 1,
            "train_nll": total_loss / example_count,
            "validation": stable_validation,
        }
        history.append(record)
        log(json.dumps(record, sort_keys=True))
        if stable_validation["nll"] < best_validation["nll"]:
            best_validation = stable_validation
            best_state = _clone_trainable_state(model)

    if best_state is None:
        raise RuntimeError("training completed without a validation checkpoint")
    model.load_state_dict(best_state, strict=False)
    training_metadata = {
        "task": "form-specialist-choice-classification",
        "seed": seed,
        "deterministic": deterministic,
        "epochs": epochs,
        "batch_size": batch_size,
        "learning_rate": learning_rate,
        "warmup": warmup,
        "workers": workers,
        "history": history,
        "best_validation": best_validation,
    }
    output_path = Path(output)
    save_checkpoint(output_path, model, config, metadata=training_metadata)
    return {
        "checkpoint": str(output_path),
        "best_validation": best_validation,
        "parameters": parameter_count(model),
        "history": history,
        "train_seconds": round(time.perf_counter() - started, 3),
    }


def _model_config(args: argparse.Namespace) -> dict:
    return {
        "encoder": args.encoder,
        "width": args.width,
        "rank": args.rank,
        "layers": args.layers,
        "heads": args.heads,
        "context_tokens": args.context_tokens,
        "option_tokens": args.option_tokens,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    train_parser = commands.add_parser("train")
    train_parser.add_argument("train")
    train_parser.add_argument("--validation", required=True)
    train_parser.add_argument("--output", type=Path, default=Path("runs/cua-s1/model"))
    train_parser.add_argument("--encoder", choices=("tiny", "tinyx"), default="tinyx")
    train_parser.add_argument("--width", type=int, default=128)
    train_parser.add_argument("--rank", type=int, default=128)
    train_parser.add_argument("--layers", type=int, default=2)
    train_parser.add_argument("--heads", type=int, default=4)
    train_parser.add_argument("--context-tokens", type=int, default=224)
    train_parser.add_argument("--option-tokens", type=int, default=96)
    train_parser.add_argument("--epochs", type=int, default=10)
    train_parser.add_argument("--batch-size", type=int, default=128)
    train_parser.add_argument("--learning-rate", type=float, default=2e-3)
    train_parser.add_argument("--warmup", type=float, default=0.05)
    train_parser.add_argument("--workers", type=int, default=0)
    train_parser.add_argument("--device", default="auto")
    train_parser.add_argument("--seed", type=int, default=7)
    train_parser.add_argument("--non-deterministic", action="store_true")

    eval_parser = commands.add_parser("eval")
    eval_parser.add_argument("checkpoint", type=Path)
    eval_parser.add_argument("data", type=Path)
    eval_parser.add_argument("--batch-size", type=int, default=256)
    eval_parser.add_argument("--device", default="auto")
    args = parser.parse_args()

    if args.command == "train":
        result = train(
            _model_config(args),
            args.train,
            args.validation,
            args.output,
            epochs=args.epochs,
            batch_size=args.batch_size,
            learning_rate=args.learning_rate,
            seed=args.seed,
            device_name=args.device,
            warmup=args.warmup,
            workers=args.workers,
            deterministic=not args.non_deterministic,
        )
        print(
            json.dumps(
                {
                    key: value
                    for key, value in result.items()
                    if key not in {"history", "train_seconds"}
                },
                indent=2,
                sort_keys=True,
            )
        )
        return

    device = select_device(args.device)
    model, collator, checkpoint_config = load_checkpoint(args.checkpoint, device)
    dataset = MetaDataset(args.data)
    print(
        json.dumps(
            {
                "checkpoint_config": checkpoint_config,
                "model": evaluate(model, dataset, collator, device, batch_size=args.batch_size),
                "shuffled_context": evaluate(
                    model,
                    dataset,
                    collator,
                    device,
                    batch_size=args.batch_size,
                    shuffle_context=True,
                ),
            },
            indent=2,
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
