"""Run a configurable, deterministic Cua-S1 form-specialist experiment ladder."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import time
from pathlib import Path

from cua_s1.model import load_checkpoint, save_checkpoint, select_device

from train import MetaDataset, evaluate, train

BASE_MODEL_CONFIG = {
    "encoder": "tinyx",
    "width": 128,
    "rank": 128,
    "layers": 2,
    "heads": 4,
    "context_tokens": 224,
    "option_tokens": 96,
}

DEFAULT_LADDER = (
    {
        "name": "tiny-64",
        "model": {"encoder": "tiny", "width": 64, "rank": 64},
        "training": {"epochs": 6},
    },
    {
        "name": "tiny-256",
        "model": {"encoder": "tiny", "width": 256, "rank": 256},
        "training": {"epochs": 6},
    },
    {"name": "tinyx-128-l2", "model": {}, "training": {"epochs": 6}},
    {
        "name": "tinyx-192-l3",
        "model": {"width": 192, "layers": 3, "heads": 6},
        "training": {"epochs": 8},
    },
    {
        "name": "tinyx-256-l4",
        "model": {"width": 256, "rank": 256, "layers": 4, "heads": 8},
        "training": {"epochs": 8, "learning_rate": 1e-3},
    },
    {
        "name": "tinyx-128-l2-lr1e-3-e12",
        "model": {},
        "training": {"epochs": 12, "learning_rate": 1e-3},
    },
)

_SAFE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
_TRAINING_KEYS = {"epochs", "batch_size", "learning_rate", "warmup"}


def _validate_name(name: str) -> str:
    if not _SAFE_NAME.fullmatch(name) or name in {".", ".."}:
        raise ValueError(f"unsafe experiment name: {name!r}")
    return name


def _experiment_seed(base_seed: int, name: str) -> int:
    digest = hashlib.sha256(f"{base_seed}:{name}".encode("utf-8")).digest()
    return int.from_bytes(digest[:4], "big")


def load_ladder(path: str | Path | None) -> list[dict]:
    """Load an optional JSON ladder, validating the small supported schema."""

    if path is None:
        ladder = [
            {
                "name": experiment["name"],
                "model": dict(experiment["model"]),
                "training": dict(experiment["training"]),
            }
            for experiment in DEFAULT_LADDER
        ]
    else:
        payload = json.loads(Path(path).read_text(encoding="utf-8"))
        if not isinstance(payload, list):
            raise ValueError("ladder JSON must be a list of experiments")
        ladder = payload

    seen: set[str] = set()
    validated: list[dict] = []
    for experiment in ladder:
        if not isinstance(experiment, dict):
            raise ValueError("each ladder experiment must be an object")
        name = _validate_name(str(experiment.get("name", "")))
        if name in seen:
            raise ValueError(f"duplicate experiment name: {name}")
        seen.add(name)
        model = experiment.get("model", {})
        training = experiment.get("training", {})
        if not isinstance(model, dict) or not isinstance(training, dict):
            raise ValueError(f"experiment {name} model and training values must be objects")
        unknown_training_keys = set(training) - _TRAINING_KEYS
        if unknown_training_keys:
            unknown = ", ".join(sorted(unknown_training_keys))
            raise ValueError(f"experiment {name} has unsupported training keys: {unknown}")
        validated.append({"name": name, "model": dict(model), "training": dict(training)})
    return validated


def _write_json(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def _metric_summary(metrics: dict) -> dict:
    return {
        "top1": metrics["top1"],
        "nll": metrics["nll"],
        "ece": metrics["ece"],
        "examples": metrics["examples"],
        "per_action": metrics["per_action"],
    }


def run_experiment(
    name: str,
    config: dict,
    training: dict,
    data: Path,
    output: Path,
    device_name: str,
    seed: int,
    workers: int,
    deterministic: bool,
    record_timing: bool = False,
) -> dict:
    """Train and score one form-specialist experiment."""

    name = _validate_name(name)
    checkpoint = output / "checkpoints" / name
    started = time.perf_counter()
    training_options = {
        "seed": seed,
        "workers": workers,
        "deterministic": deterministic,
        **training,
    }
    result = train(
        config,
        data / "train.jsonl",
        data / "validation.jsonl",
        checkpoint,
        device_name=device_name,
        log=lambda message: print(f"[{name}] {message}"),
        **training_options,
    )
    device = select_device(device_name)
    model, collator, _ = load_checkpoint(checkpoint, device)
    test_dataset = MetaDataset(data / "test.jsonl")
    test = evaluate(model, test_dataset, collator, device)
    shuffled_context = evaluate(model, test_dataset, collator, device, shuffle_context=True)
    record = {
        "name": name,
        "config": config,
        "training": training_options,
        "parameters": result["parameters"],
        "best_validation": result["best_validation"],
        "test": _metric_summary(test),
        "shuffled_context": _metric_summary(shuffled_context),
        "checkpoint": str(Path("checkpoints") / name),
    }
    if record_timing:
        record["timing"] = {
            "train_seconds": result["train_seconds"],
            "wall_seconds": round(time.perf_counter() - started, 3),
            "test_rows_per_second": test["rows_per_second"],
        }
    print(
        json.dumps(
            {
                "name": name,
                "parameters": record["parameters"],
                "test_top1": record["test"]["top1"],
                "shuffled_context_top1": record["shuffled_context"]["top1"],
            },
            sort_keys=True,
        )
    )
    return record


def select_best(records: list[dict]) -> dict:
    if not records:
        raise ValueError("cannot select a best experiment from an empty result set")
    return max(
        records,
        key=lambda record: (
            record["best_validation"]["top1"],
            -record["best_validation"]["nll"],
            record["name"],
        ),
    )


def build_manifest(records: list[dict], base_seed: int, deterministic: bool) -> dict:
    """Build the final selection record from completed experiment results."""

    best = select_best(records)
    return {
        "base_seed": base_seed,
        "deterministic": deterministic,
        "best": best["name"],
        "best_checkpoint": "best",
        "experiments": records,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, default=Path("data/cua-s1"))
    parser.add_argument("--output", type=Path, default=Path("runs/cua-s1/autoresearch"))
    parser.add_argument("--ladder", type=Path, help="JSON experiment list")
    parser.add_argument("--device", default="auto")
    parser.add_argument("--only", nargs="*", help="run only these experiment names")
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--workers", type=int, default=0)
    parser.add_argument("--non-deterministic", action="store_true")
    parser.add_argument("--record-timing", action="store_true")
    args = parser.parse_args()

    ladder = load_ladder(args.ladder)
    selected = set(args.only or ())
    available = {experiment["name"] for experiment in ladder}
    unknown = selected - available
    if unknown:
        raise ValueError(f"unknown experiment names: {', '.join(sorted(unknown))}")

    args.output.mkdir(parents=True, exist_ok=True)
    records: list[dict] = []
    for experiment in ladder:
        name = experiment["name"]
        if selected and name not in selected:
            continue
        config = {**BASE_MODEL_CONFIG, **experiment["model"]}
        records.append(
            run_experiment(
                name,
                config,
                experiment["training"],
                args.data,
                args.output,
                args.device,
                _experiment_seed(args.seed, name),
                args.workers,
                not args.non_deterministic,
                args.record_timing,
            )
        )
        _write_json(args.output / "results.json", {"experiments": records})

    best = select_best(records)
    best_checkpoint = args.output / best["checkpoint"]
    device = select_device(args.device)
    model, _, source_config = load_checkpoint(best_checkpoint, device)
    best_metadata = {
        "task": "form-specialist-choice-classification",
        "selected_experiment": best["name"],
        "selection_metrics": {
            "validation": best["best_validation"],
        },
        "source_checkpoint": best["checkpoint"],
        "source_config": source_config,
    }
    save_checkpoint(
        args.output / "best",
        model,
        best["config"],
        metadata=best_metadata,
    )
    manifest = build_manifest(records, args.seed, not args.non_deterministic)
    _write_json(args.output / "results.json", manifest)
    print(
        json.dumps(
            {
                "best": best["name"],
                "test_top1": best["test"]["top1"],
                "checkpoint": "best",
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
