# Cua × HUD - Evaluate Any Computer-Use Agent

_Published on August 27, 2025 by Dillon DuPont_

You can now benchmark any GUI-capable agent on real computer-use tasks through our new integration with [HUD](https://hud.so), the evaluation platform for computer-use agents.

If [yesterday's 0.4 release](composite-agents.md) made it easy to compose planning and grounding models, today's update makes it easy to measure them. Configure your model, run evaluations at scale, and watch traces live in HUD.

<img src="./assets/hud-agent-evals.png" alt="Cua × HUD">

## What you get

- One-line evals on OSWorld (and more) for OpenAI, Anthropic, Hugging Face, and composed GUI models.
- Live traces at [app.hud.so](https://app.hud.so) to see every click, type, and screenshot.
- Zero glue code needed - we wrapped the interface for you.
- With Cua's Agent SDK, you can benchmark any configurations of models, by just changing the `model` string.

## What is OSWorld?

[OSWorld](https://os-world.github.io) is a comprehensive evaluation benchmark comprising 369 real-world computer-use tasks spanning diverse desktop environments (Chrome, LibreOffice, GIMP, VS Code, etc.) developed by XLang Labs. This benchmark has emerged as the de facto standard for evaluating multimodal agents in realistic computing environments, with adoption by leading AI research teams at OpenAI, Anthropic, and other major institutions for systematic agent assessment. The benchmark was recently enhanced to [OSWorld-Verified](https://xlang.ai/blog/osworld-verified), incorporating rigorous validation improvements that address over 300 community-identified issues to ensure evaluation reliability and reproducibility.

## Environment Setup

First, set up your environment variables:

```bash
export HUD_API_KEY="your_hud_api_key"       # Required for HUD access
export ANTHROPIC_API_KEY="your_anthropic_key" # For Claude models
export OPENAI_API_KEY="your_openai_key"       # For OpenAI models
```

## Try it

### Quick Start - Single Task

```python
from agent.integrations.hud import run_single_task

await run_single_task(
    dataset="hud-evals/OSWorld-Verified-XLang",
    model="openai/computer-use-preview+openai/gpt-5-nano",  # or any supported model string
    task_id=155  # open last tab task (easy)
)
```

### Run a dataset (parallel execution)

```python
from agent.integrations.hud import run_full_dataset

# Test on OSWorld (367 computer-use tasks)
await run_full_dataset(
    dataset="hud-evals/OSWorld-Verified-XLang",
    model="openai/computer-use-preview+openai/gpt-5-nano",  # any supported model string
    split="train[:3]"  # try a few tasks to start
)

# Or test on SheetBench (50 spreadsheet tasks)
await run_full_dataset(
    dataset="hud-evals/SheetBench-V2",
    model="anthropic/claude-3-5-sonnet-20241022",
    split="train[:2]"
)
```

### Live Environment Streaming

Watch your agent work in real-time. Example output:

```md
Starting full dataset run...
╔═════════════════════════════════════════════════════════════════╗
║ 🚀 See your agent live at: ║
╟─────────────────────────────────────────────────────────────────╢
║ https://app.hud.so/jobs/fe05805d-4da9-4fc6-84b5-5c518528fd3c ║
╚═════════════════════════════════════════════════════════════════╝
```

## Configuration Options

Customize your evaluation with these options:

- **Environment types**: `environment="linux"` (OSWorld) or `environment="browser"` (SheetBench)
- **Model composition**: Mix planning and grounding models with `+` (e.g., `"gpt-4+gpt-5-nano"`)
- **Parallel scaling**: Set `max_concurrent_tasks` for throughput
- **Local trajectories**: Save with `trajectory_dir` for offline analysis
- **Live monitoring**: Every run gets a unique trace URL at app.hud.so

## Learn more

- Notebook with end‑to‑end examples: https://github.com/trycua/cua/blob/main/notebooks/eval_osworld.ipynb
- Docs: https://docs.trycua.com/docs/agent-sdk/integrations/hud
- Live traces: https://app.hud.so
