# Computer Agent Benchmarks

This directory contains benchmarks designed to test agent providers in the Computer Agent SDK against reference agent implementations.

## Overview

The benchmark system evaluates models on GUI grounding tasks, specifically click prediction accuracy. It supports both:

- **Computer Agent SDK providers** (using model strings like `"huggingface-local/HelloKKMe/GTA1-7B"`)
- **Reference agent implementations** (custom model classes implementing the `ModelProtocol`)

## Available Benchmarks

### 1. ScreenSpot-v2 (`ss-v2.py`)

- **Dataset**: ScreenSpot-v2 (click-only GUI grounding)
- **Format**: Standard resolution screenshots
- **Task**: Predict click coordinates given an instruction and image
- **Metrics**: Accuracy, Error Rate, Timing, VRAM usage

### 2. ScreenSpot-Pro (`ss-pro.py`)

- **Dataset**: ScreenSpot-Pro (high-resolution click-only GUI grounding)
- **Format**: High-resolution screenshots
- **Task**: Predict click coordinates given an instruction and image
- **Metrics**: Accuracy, Error Rate, Timing, VRAM usage

### 3. Interactive Testing (`interactive.py`)

- **Real-time testing**: Take screenshots and visualize model predictions
- **Commands**:
  - Type instruction → test all models on last screenshot
  - `screenshot` → take screenshot
  - `models` → list available models
  - `quit`/`exit` → exit tool
- **Output**: Visual predictions with crosshairs for each model

## Running Benchmarks

### 1. Configure Models

Edit `utils.py` to specify which models you want to test in `get_available_models()`.

### 2. Run Benchmark

```bash
# ScreenSpot-v2 benchmark
python ss-v2.py --samples 50

# ScreenSpot-Pro benchmark
python ss-pro.py --samples 50

# Interactive testing
python interactive.py
```

## Output

### Console Output

```
Model Results:
  Accuracy: 85.50% (171/200)
  Avg Time: 1.23s (0.89s - 2.45s)
  VRAM Usage: 4.5GB (max) / 3.4GB (avg)
```

### Generated Files

- **Markdown Report**: `*_results.md` with detailed results tables
- **Visualizations**: `output/` directory with prediction visualizations
- **Interactive Output**: `interactive_output/` for interactive session results

## Contributing

To add a new reference model, follow the instructions in [contrib.md](contrib.md).
