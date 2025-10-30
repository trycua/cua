#!/usr/bin/env python3
"""
ScreenSpot-Pro Benchmark Script

Evaluates models on the ScreenSpot-Pro dataset for click prediction accuracy.
Supports both ComputerAgent model strings and custom model classes.
"""

import argparse
import asyncio
import random
import statistics
import time
from typing import Optional

from datasets import load_dataset
from tqdm import tqdm
from utils import (
    ModelWrapper,
    get_available_models,
    get_gpu_memory,
    is_click_in_bbox,
    save_results_to_markdown,
    save_visualizations,
)


async def evaluate_model(
    model_wrapper: ModelWrapper, dataset, max_samples: Optional[int] = None
) -> dict:
    """
    Evaluate a model on the ScreenSpot-Pro dataset.

    Args:
        model_wrapper: ModelWrapper instance
        dataset: ScreenSpot-Pro dataset (list of samples)
        max_samples: Maximum number of samples to evaluate (None for all)

    Returns:
        Dictionary with evaluation results
    """
    print(f"\nEvaluating model: {model_wrapper.model_name}")

    # Load model
    await model_wrapper.load_model()

    total_samples = len(dataset)
    if max_samples is not None:
        total_samples = min(max_samples, total_samples)

    correct_predictions = 0
    error_predictions = 0
    results = []

    for i in tqdm(range(total_samples), desc=f"Evaluating {model_wrapper.model_name}"):
        sample = dataset[i]

        # Extract sample data
        image = sample["image"]
        instruction = sample["instruction"]
        bbox = sample["bbox"]  # [x1, y1, x2, y2]
        sample_id = sample["img_filename"]

        # Predict click coordinates with timing
        start_time = time.time()
        click_coords = await model_wrapper.predict_click(image, instruction)
        prediction_time = time.time() - start_time

        # Check if prediction is correct
        is_correct = is_click_in_bbox(click_coords, bbox)

        if is_correct:
            correct_predictions += 1

        results.append(
            {
                "id": sample_id,
                "instruction": instruction,
                "bbox": bbox,
                "predicted_coords": click_coords,
                "is_correct": is_correct,
                "failed": False,
                "prediction_time": prediction_time,
            }
        )

    # Unload model
    await model_wrapper.unload_model()

    # Calculate metrics
    accuracy = correct_predictions / total_samples if total_samples > 0 else 0.0
    error_rate = error_predictions / total_samples if total_samples > 0 else 0.0

    # Calculate timing statistics
    successful_times = [r["prediction_time"] for r in results if not r["failed"]]
    avg_prediction_time = sum(successful_times) / len(successful_times) if successful_times else 0.0
    median_prediction_time = statistics.median(successful_times) if successful_times else 0.0
    min_prediction_time = min(successful_times) if successful_times else 0.0
    max_prediction_time = max(successful_times) if successful_times else 0.0

    # Get VRAM statistics
    vram_stats = model_wrapper.get_vram_stats()

    return {
        "model_name": model_wrapper.model_name,
        "total_samples": total_samples,
        "correct_predictions": correct_predictions,
        "failed_predictions": error_predictions,
        "accuracy": accuracy,
        "failure_rate": error_rate,
        "avg_prediction_time": avg_prediction_time,
        "median_prediction_time": median_prediction_time,
        "min_prediction_time": min_prediction_time,
        "max_prediction_time": max_prediction_time,
        "vram_max_mb": vram_stats["max_mb"],
        "vram_avg_mb": vram_stats["avg_mb"],
        "results": results,
    }


async def main():
    """
    Main function to run the benchmark.
    """
    # Parse command line arguments
    parser = argparse.ArgumentParser(description="ScreenSpot-Pro Benchmark Script")
    parser.add_argument(
        "--samples", type=int, default=300, help="Number of samples to evaluate (default: 300)"
    )
    parser.add_argument(
        "--seed", type=int, default=42, help="Random seed for shuffling (default: 42)"
    )
    args = parser.parse_args()

    # Set random seed
    random.seed(args.seed)

    # Load dataset
    print("Loading ScreenSpot-Pro dataset...")
    ds = load_dataset("lmms-lab/ScreenSpot-Pro")
    dataset = ds["train"]  # type: ignore
    # Convert to list to support indexing
    dataset_list = list(dataset)
    print(f"Dataset loaded: {len(dataset_list)} samples")

    # Shuffle dataset with seed
    random.shuffle(dataset_list)
    print(f"Dataset shuffled with seed {args.seed}")

    # Get available models
    models = get_available_models()

    # Evaluation settings
    max_samples = args.samples  # Use command line argument

    # Run evaluations
    all_results = []

    for model in models:
        model_wrapper = ModelWrapper(model)
        result = await evaluate_model(model_wrapper, dataset_list, max_samples)
        all_results.append(result)

        # Print summary
        print(f"\n{result['model_name']} Results:")
        print(f"  Accuracy: {result['accuracy']*100:.2f}%")
        print(f"  Correct: {result['correct_predictions']}/{result['total_samples']}")
        print(f"  Errors: {result['failed_predictions']}")
        print(f"  Error Rate: {result['failure_rate']*100:.2f}%")
        print(f"  Avg Time: {result['avg_prediction_time']:.2f}s")
        print(f"  Median Time: {result['median_prediction_time']:.2f}s")
        print(
            f"  Time Range: {result['min_prediction_time']:.2f}s - {result['max_prediction_time']:.2f}s"
        )
        print(f"  VRAM Max: {result['vram_max_mb']:.1f}MB")
        print(f"  VRAM Avg: {result['vram_avg_mb']:.1f}MB")

        # Print GPU memory info
        gpu_memory = get_gpu_memory()
        if gpu_memory and gpu_memory[0] > 0:
            print(f"  GPU Free Memory: {gpu_memory[0]:.1f}MB")

    # Save results
    if all_results:
        save_results_to_markdown(all_results)
        save_visualizations(all_results, dataset_list)
        print("\nBenchmark completed successfully!")
    else:
        print("\nNo successful evaluations completed.")


if __name__ == "__main__":
    asyncio.run(main())
