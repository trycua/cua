#!/usr/bin/env python3
"""Run a repeatable MLX-LM prefill and generation throughput benchmark."""

from __future__ import annotations

import argparse
import json
import platform
import statistics
import sys
import time
from importlib.metadata import version
from pathlib import Path

import mlx.core as mx
from mlx_lm import load, stream_generate


PROMPT_SEED = (
    "Apple Silicon virtual machines expose a paravirtualized Metal device. "
    "Applications query its capabilities before choosing compute kernels. "
    "This benchmark repeats a fixed passage to measure prompt processing and "
    "token generation with identical model weights and prompt tokens. "
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="Local MLX model directory")
    parser.add_argument("--model-id", required=True, help="Published model identifier")
    parser.add_argument("--model-revision", required=True, help="Pinned model revision")
    parser.add_argument("--prompt-tokens", type=int, default=512)
    parser.add_argument("--generation-tokens", type=int, default=128)
    parser.add_argument("--repeats", type=int, default=10)
    parser.add_argument("--warmup-prompt-tokens", type=int, default=64)
    parser.add_argument("--warmup-generation-tokens", type=int, default=16)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def fixed_prompt(tokenizer, count: int) -> list[int]:
    text = PROMPT_SEED
    while len(tokenizer.encode(text, add_special_tokens=True)) < count:
        text += PROMPT_SEED
    return list(tokenizer.encode(text, add_special_tokens=True)[:count])


def run_once(model, tokenizer, prompt: list[int], max_tokens: int, seed: int) -> dict:
    mx.random.seed(seed)
    mx.clear_cache()
    if hasattr(mx, "reset_peak_memory"):
        mx.reset_peak_memory()

    started = time.perf_counter()
    response = None
    for response in stream_generate(
        model,
        tokenizer,
        prompt,
        max_tokens=max_tokens,
    ):
        pass
    elapsed = time.perf_counter() - started
    if response is None:
        raise RuntimeError("MLX-LM returned no generation response")

    return {
        "prompt_tokens": int(response.prompt_tokens),
        "prompt_tps": float(response.prompt_tps),
        "generation_tokens": int(response.generation_tokens),
        "generation_tps": float(response.generation_tps),
        "peak_memory_gb": float(response.peak_memory),
        "finish_reason": response.finish_reason,
        "wall_seconds": elapsed,
    }


def main() -> None:
    args = parse_args()
    model, tokenizer = load(args.model)

    # Match llama-bench semantics by measuring the requested token count even if
    # the generated stream happens to contain the model's end-of-sequence token.
    tokenizer._eos_token_ids = set()

    prompt = fixed_prompt(tokenizer, args.prompt_tokens)
    warmup_prompt = prompt[: args.warmup_prompt_tokens]
    warmup = run_once(
        model,
        tokenizer,
        warmup_prompt,
        args.warmup_generation_tokens,
        args.seed,
    )

    samples = [
        run_once(
            model,
            tokenizer,
            prompt,
            args.generation_tokens,
            args.seed + repeat,
        )
        for repeat in range(args.repeats)
    ]

    result = {
        "schema_version": 1,
        "runtime": {
            "python": platform.python_version(),
            "mlx": version("mlx"),
            "mlx_lm": version("mlx-lm"),
            "platform": platform.platform(),
        },
        "model": {
            "id": args.model_id,
            "revision": args.model_revision,
            "path": str(Path(args.model).resolve()),
        },
        "configuration": {
            "prompt_tokens": args.prompt_tokens,
            "generation_tokens": args.generation_tokens,
            "repeats": args.repeats,
            "warmup_prompt_tokens": args.warmup_prompt_tokens,
            "warmup_generation_tokens": args.warmup_generation_tokens,
            "seed": args.seed,
            "eos_stopping": False,
        },
        "warmup": warmup,
        "samples": samples,
        "median": {
            "prompt_tps": statistics.median(s["prompt_tps"] for s in samples),
            "generation_tps": statistics.median(
                s["generation_tps"] for s in samples
            ),
            "peak_memory_gb": statistics.median(
                s["peak_memory_gb"] for s in samples
            ),
        },
    }

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    json.dump(result, sys.stdout, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
