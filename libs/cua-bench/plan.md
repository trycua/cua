# Plan: Tinker RL Training Integration with cua-bench

## Overview

This plan describes how to integrate [Tinker](https://tinker-docs.thinkingmachines.ai/) as an RL training backend for the OpenCUA model using `cua-bench` as the environment. The result is a closed training loop: `cua-bench` generates rollout trajectories and rewards → Tinker updates model weights via PPO → updated weights reload into vLLM → next rollout round.

---

## 1. How Traces Are Collected Today

After running `cb run tasks --agent opencua`, each task execution produces a **HuggingFace Dataset** stored on disk via `cua_bench/tracing.py`.

### Trace event schema

| Field | Type | Description |
|---|---|---|
| `event_name` | `str` | One of `"reset"`, `"agent_step"`, `"solve"`, `"evaluate"` |
| `data_json` | `str` | JSON payload specific to event type |
| `data_images` | `Sequence[Image]` | Screenshots at that moment |
| `trajectory_id` | `str` | UUID identifying the full episode |
| `timestamp` | `str` | ISO-8601 UTC timestamp |

### Events recorded per task

| Event | When | Key `data_json` fields |
|---|---|---|
| `reset` | After env setup completes | `{task, task_index, elapsed}` |
| `agent_step` | After every agent action | `{step, agent, model, usage, output}` |
| `solve` | After oracle solution (non-eval runs) | `{completed: bool}` |
| `evaluate` | After evaluation function | `{result: <reward_value>}` |

The `agent_step` `output` field mirrors the raw message list from the ComputerAgent SDK, including typed actions (`click`, `type_text`, etc.) and the final `"DONE"` message.

### Trace storage paths

```
~/.local/share/cua-bench/runs/<run_id>/
├── task_0_trace/          # HF Dataset (arrow files)
│   ├── dataset_info.json
│   ├── state.json
│   └── data/
├── task_0_agent_logs/
│   └── trajectories/      # ComputerAgent internal trajectories
└── imgs/                  # Optional PNG dumps (--save-pngs flag)
```

### Loading traces programmatically

```python
from datasets import load_from_disk

ds = load_from_disk("~/.local/share/cua-bench/runs/<run_id>/task_0_trace/")

# Extract reward from evaluate event
reward_row = next(r for r in ds if r["event_name"] == "evaluate")
reward = json.loads(reward_row["data_json"])["result"]

# Extract agent steps in order
steps = [r for r in ds if r["event_name"] == "agent_step"]
for step in steps:
    data = json.loads(step["data_json"])
    screenshot = step["data_images"][0]  # PIL.Image
```

---

## 2. Tinker API Overview

Tinker is a cloud-hosted RL/SL fine-tuning platform for LLMs. It abstracts distributed GPU training while exposing a simple Python API.

### Core client hierarchy

```
ServiceClient  (entry point)
├── create_lora_training_client()  →  TrainingClient
│   ├── forward_backward(data, loss_fn)  →  Future
│   ├── optim_step(AdamParams)           →  Future
│   ├── save_state(name)                 →  Future[str]  (tinker:// path)
│   └── save_weights_and_get_sampling_client()  →  SamplingClient
├── create_sampling_client(checkpoint)   →  SamplingClient
│   ├── sample(prompt, params)           →  List[str]
│   └── compute_logprobs(prompt, target) →  List[float]
└── create_rest_client()                 →  RestClient
    ├── list_checkpoints(run_id)
    └── get_checkpoint_archive_url_from_tinker_path(path)
```

### Supported RL loss functions

| Loss | Use case |
|---|---|
| `"importance_sampling"` | Off-policy PPO without clipping |
| `"ppo"` | Clipped PPO (standard RLHF) |
| `"cispo"` | Clipped importance sampling coefficients |
| `"dro"` | Direct reward optimization with quadratic penalty |
| Custom | Via `forward_backward_custom()` with arbitrary differentiable loss |

### Checkpoint persistence

| Method | Saves | When to use |
|---|---|---|
| `save_weights_for_sampler(name)` | Weights only | Quick eval between epochs |
| `save_state(name)` | Weights + optimizer state | Full resume / multi-phase SL→RL |
| `save_weights_and_get_sampling_client()` | Weights → new SamplingClient | End of training step, for next rollout |

Checkpoint paths follow the scheme `tinker://<model_id>/<name>`. Default TTL is 7 days; use `RestClient` to extend.

---

## 3. Proposed Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                       RL Training Loop                       │
│                                                              │
│  ┌──────────────┐    rollouts    ┌───────────────────────┐   │
│  │  cua-bench   │ ─────────────▶│   Trace Collector     │   │
│  │  (env)       │               │  (HF Datasets)         │   │
│  └──────┬───────┘               └──────────┬────────────┘   │
│         │                                  │                 │
│  ┌──────▼───────┐               ┌──────────▼────────────┐   │
│  │  vLLM        │ ◀─ weights ── │   Tinker              │   │
│  │  (OpenCUA)   │               │   TrainingClient      │   │
│  └──────────────┘               │   (PPO / IS loss)     │   │
│                                 └───────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### Component roles

| Component | Role |
|---|---|
| `cua-bench` | Rollout environment; generates trajectories + reward signals |
| `cua_bench/tracing.py` | Serializes trajectories to HF Datasets on disk |
| `Trace Collector` | Reads traces, extracts `(prompt, action, reward)` tuples |
| `Datum Builder` | Converts tuples to Tinker `Datum` objects (token sequences + masks) |
| `Tinker TrainingClient` | Runs PPO gradient updates on remote GPU cluster |
| `Tinker SamplingClient` | Not used for generation (vLLM does that), but used for `compute_logprobs` to get reference policy log-probs for IS ratio |
| `vLLM` | Serves the current policy for cua-bench rollouts |
| `Weight Sync` | Downloads updated LoRA weights from Tinker checkpoint → hot-reloads into vLLM |

---

## 4. Implementation Plan

### Phase 1: Trace extraction pipeline

**New file**: `cua_bench/rl/trace_reader.py`

```python
"""
Read completed cua-bench run traces and convert them to RL experience tuples.
"""
import json
from pathlib import Path
from dataclasses import dataclass
from typing import List, Optional
from datasets import load_from_disk

@dataclass
class RLStep:
    step: int
    prompt_images: list          # screenshots before action
    action_text: str             # raw action output from model
    reward: float                # 0.0 except at terminal step

@dataclass
class Episode:
    trajectory_id: str
    task_description: str
    steps: List[RLStep]
    terminal_reward: float       # from "evaluate" event

def load_episode_from_trace(trace_dir: str) -> Episode:
    ds = load_from_disk(trace_dir)
    rows_by_event = {}
    for row in ds:
        rows_by_event.setdefault(row["event_name"], []).append(row)

    reward = float(json.loads(rows_by_event["evaluate"][0]["data_json"])["result"])
    trajectory_id = ds[0]["trajectory_id"]

    steps = []
    for row in rows_by_event.get("agent_step", []):
        data = json.loads(row["data_json"])
        steps.append(RLStep(
            step=data["step"],
            prompt_images=row["data_images"],
            action_text=str(data["output"]),
            reward=0.0,
        ))

    # Assign terminal reward to last step only (sparse reward)
    if steps:
        steps[-1].reward = reward

    return Episode(trajectory_id, task_description="", steps=steps, terminal_reward=reward)

def load_run_episodes(run_dir: str) -> List[Episode]:
    """Load all task traces from a cua-bench run directory."""
    run_path = Path(run_dir)
    episodes = []
    for trace_dir in sorted(run_path.glob("task_*_trace")):
        episodes.append(load_episode_from_trace(str(trace_dir)))
    return episodes
```

### Phase 2: Datum construction

**New file**: `cua_bench/rl/datum_builder.py`

The Tinker `Datum` format requires tokenized inputs with explicit weight masks. For a vision-language model like OpenCUA (based on Qwen-VL), each step's datum looks like:

```
[system_tokens] [image_tokens] [user_instruction_tokens] [action_tokens]
                                                          ^^^^^^^^^^^^^^^^
                                                          loss computed here
```

Key decisions:
- **Input sequence**: System prompt + screenshot(s) + task description (this is the `model_input`)
- **Target sequence**: The action the agent took (this is the `loss_fn_inputs` target)
- **Weight mask**: 1.0 on action tokens, 0.0 on prompt tokens
- **Advantage weighting**: For PPO, multiply step reward by a per-step advantage estimate; apply as weight multiplier on action tokens

```python
import tinker.types as tt
from transformers import AutoTokenizer

def build_datum(step: RLStep, tokenizer, advantage: float) -> tt.Datum:
    """Convert a single RL step to a Tinker Datum."""
    # Tokenize action (target)
    action_ids = tokenizer.encode(step.action_text, add_special_tokens=False)

    # Tokenize prompt context (images + instruction handled by rendering pipeline)
    # ... (use Tinker's ImageChunk + EncodedTextChunk for multimodal)

    model_input = tt.ModelInput(
        chunks=[
            tt.EncodedTextChunk(token_ids=prompt_ids),
            tt.ImageChunk(image=step.prompt_images[0]),  # latest screenshot
            tt.EncodedTextChunk(token_ids=action_ids),
        ]
    )

    # Shift targets: model predicts token[i+1] from token[i]
    target_ids = action_ids  # shifted internally by Tinker
    weights = [max(0.0, advantage)] * len(action_ids)  # zero-out negative advantages

    return tt.Datum(
        model_input=model_input,
        loss_fn_inputs=tt.LossFnInputs(
            target_token_ids=target_ids,
            weights=weights,
        )
    )

def build_batch(episode: Episode, tokenizer, gamma=0.99) -> list[tt.Datum]:
    """Convert episode to list of Datums with discounted advantage estimates."""
    # Compute discounted returns (simple Monte Carlo)
    G = 0.0
    returns = []
    for step in reversed(episode.steps):
        G = step.reward + gamma * G
        returns.insert(0, G)

    baseline = sum(returns) / len(returns) if returns else 0.0

    datums = []
    for step, G in zip(episode.steps, returns):
        advantage = G - baseline
        datums.append(build_datum(step, tokenizer, advantage))
    return datums
```

### Phase 3: RL training loop

**New file**: `cua_bench/rl/train.py`

```python
"""
Main RL training loop using Tinker.

Usage:
    python -m cua_bench.rl.train \
        --base-model Qwen/Qwen3-8B \
        --run-dir ~/.local/share/cua-bench/runs/<run_id> \
        --epochs 10 \
        --lr 1e-5
"""
import tinker
from tinker import ServiceClient
import tinker.types as tt

def train_epoch(training_client, episodes: list, tokenizer, args):
    """Run one PPO update epoch over a batch of episodes."""
    all_datums = []
    for ep in episodes:
        all_datums.extend(build_batch(ep, tokenizer, gamma=args.gamma))

    if not all_datums:
        return

    # Submit forward-backward and optim step without awaiting between them
    # (keeps operations in the same ~10s Tinker clock cycle)
    fwdbwd_future = training_client.forward_backward_async(
        data=all_datums,
        loss_fn="ppo",
        loss_fn_params=tt.PpoParams(
            clip_low_threshold=0.8,
            clip_high_threshold=1.2,
        ),
    )
    optim_future = training_client.optim_step_async(
        tt.AdamParams(
            learning_rate=args.lr,
            betas=(0.9, 0.999),
            weight_decay=0.01,
            grad_clip=1.0,
        )
    )

    # Collect results
    fwdbwd_result = fwdbwd_future.result()
    optim_future.result()
    return fwdbwd_result

def main(args):
    service_client = ServiceClient(api_key=args.tinker_api_key)

    training_client = service_client.create_lora_training_client(
        base_model=args.base_model,
        lora_config=tt.LoraConfig(rank=16, seed=42),
    )

    tokenizer = training_client.get_sampling_client().tokenizer

    for epoch in range(args.epochs):
        # 1. Run cua-bench rollouts with current policy
        run_id = run_cuabench_rollouts(args)

        # 2. Load traces
        run_dir = f"~/.local/share/cua-bench/runs/{run_id}"
        episodes = load_run_episodes(run_dir)

        # 3. Log stats
        avg_reward = sum(ep.terminal_reward for ep in episodes) / max(len(episodes), 1)
        print(f"Epoch {epoch}: {len(episodes)} episodes, avg_reward={avg_reward:.3f}")

        # 4. Train
        train_epoch(training_client, episodes, tokenizer, args)

        # 5. Save weights and get new sampling client
        sampling_client = training_client.save_weights_and_get_sampling_client()

        # 6. Checkpoint full state every N epochs
        if (epoch + 1) % args.checkpoint_every == 0:
            ckpt_path = training_client.save_state(name=f"epoch-{epoch+1}").result()
            print(f"Checkpoint saved: {ckpt_path}")

        # 7. Hot-reload updated weights into vLLM
        sync_weights_to_vllm(training_client, args.vllm_url)

def run_cuabench_rollouts(args) -> str:
    """Launch a cua-bench dataset run and return the run_id."""
    import subprocess, re
    result = subprocess.run([
        "cb", "run", "dataset", args.tasks_path,
        "--agent", "opencua",
        "--model", args.model,
        "--max-steps", str(args.max_steps),
    ], capture_output=True, text=True)
    run_id = re.search(r"run-[a-f0-9-]+", result.stdout).group(0)
    # Wait for all sessions to complete (cb run watch or poll cb run info)
    wait_for_run(run_id)
    return run_id
```

### Phase 4: Weight sync to vLLM

After each Tinker training step the updated LoRA weights must reload into the running vLLM server so that subsequent cua-bench rollouts use the new policy.

**New file**: `cua_bench/rl/weight_sync.py`

```python
"""
Download LoRA checkpoint from Tinker and hot-reload into a running vLLM instance.
"""
import requests
import tarfile
import tempfile
from pathlib import Path

def sync_weights_to_vllm(training_client, vllm_base_url: str):
    """
    1. Save weights to Tinker checkpoint
    2. Download tar archive
    3. POST to vLLM's /load_lora_adapter endpoint
    """
    # Save weights-only checkpoint (faster than save_state)
    ckpt_path = training_client.save_weights_for_sampler(name="current").result()

    # Get signed download URL via RestClient
    rest_client = training_client.service_client.create_rest_client()
    url_response = rest_client.get_checkpoint_archive_url_from_tinker_path(ckpt_path)

    # Download tar archive
    with tempfile.TemporaryDirectory() as tmpdir:
        archive_path = Path(tmpdir) / "lora.tar"
        r = requests.get(url_response.url, stream=True)
        r.raise_for_status()
        with open(archive_path, "wb") as f:
            for chunk in r.iter_content(chunk_size=8192):
                f.write(chunk)

        # Extract LoRA adapter files
        with tarfile.open(archive_path) as tar:
            tar.extractall(tmpdir)

        adapter_dir = Path(tmpdir) / "adapter"

        # Hot-reload via vLLM dynamic LoRA API
        requests.post(
            f"{vllm_base_url}/load_lora_adapter",
            json={
                "lora_name": "opencua-current",
                "lora_path": str(adapter_dir),
            }
        ).raise_for_status()
```

### Phase 5: Resume training from checkpoint

```python
# Resume full training state (weights + optimizer momentum)
training_client = service_client.create_training_client_from_state_with_optimizer(
    checkpoint_path="tinker://<model_id>/epoch-5"
)

# Resume weights only (fresh optimizer)
training_client = service_client.create_lora_training_client(
    base_model=args.base_model,
    lora_config=tt.LoraConfig(rank=16),
    initial_checkpoint="tinker://<model_id>/epoch-5",
)
```

---

## 5. File Structure

```
libs/cua-bench/
├── cua_bench/
│   └── rl/
│       ├── __init__.py
│       ├── trace_reader.py     # Load HF Dataset traces → Episode objects
│       ├── datum_builder.py    # Episode → Tinker Datum (tokenized, masked)
│       ├── train.py            # Main RL training loop (entry point)
│       └── weight_sync.py      # Download Tinker checkpoint → reload vLLM
├── plan.md                     # This file
└── pyproject.toml              # Add tinker to [project.optional-dependencies]
```

Add to `pyproject.toml`:
```toml
[project.optional-dependencies]
rl = [
    "tinker",
    "transformers>=4.57.0,<5.0",
]
```

---

## 6. End-to-End Training Script

```bash
# 1. Start vLLM with base model
vllm serve Qwen/Qwen3-8B \
    --served-model-name opencua \
    --enable-lora \
    --host 0.0.0.0 --port 30000

# 2. Set env vars
export OPENCUA_BASE_URL=http://localhost:30000/v1
export OPENCUA_API_KEY=EMPTY
export TINKER_API_KEY=<your_tinker_key>

# 3. Run RL training loop
python -m cua_bench.rl.train \
    --base-model Qwen/Qwen3-8B \
    --tasks-path tasks/slack_env \
    --model openai/opencua \
    --max-steps 50 \
    --epochs 20 \
    --lr 1e-5 \
    --gamma 0.99 \
    --checkpoint-every 5 \
    --vllm-url http://localhost:30000/v1
```

---

## 7. Key Design Decisions

| Decision | Rationale |
|---|---|
| **Sparse reward** | `evaluate_task` returns a single terminal reward per episode; intermediate steps get reward 0.0 and the terminal step gets the full reward |
| **Monte Carlo returns** with simple baseline | Simple, unbiased, works well with sparse rewards; GAE would need a value network |
| **PPO loss in Tinker** | Standard choice for LLM RL; clip thresholds prevent oversized updates when off-policy rollouts diverge |
| **LoRA fine-tuning** | Tinker only supports LoRA currently; keeps checkpoint size small for fast sync to vLLM |
| **Weights-only sync to vLLM** | `save_weights_for_sampler` is faster than `save_state`; vLLM's dynamic LoRA API avoids server restart |
| **Async Tinker calls** | Submit `forward_backward_async` and `optim_step_async` without awaiting between them to keep both in the same ~10s Tinker clock cycle |
| **Screenshot as model input** | Each `agent_step` trace row has a screenshot; use the most recent as the visual context for that step's datum |

---

## 8. Open Questions / Future Work

1. **Reference policy for IS ratio**: PPO needs log-probs from both current policy (from Tinker `compute_logprobs`) and sampling policy (from vLLM generation). Need to capture vLLM log-probs at rollout time and store them in trace metadata.

2. **Dense rewards**: Currently uses sparse evaluation reward. Could add partial rewards (e.g., intermediate task completion signals from `evaluate_task` called mid-episode) for faster learning.

3. **Multi-task batching**: Different task types may have very different reward scales. Per-task reward normalization or separate baselines may be needed.

4. **Rollout parallelism**: `cb run dataset` already parallelizes across tasks. Tinker's clock-cycle architecture means training throughput is bounded by how many rollout batches can be collected per ~10s cycle.

5. **Vision input**: OpenCUA is a vision-language model. Tinker supports Qwen3-VL via `ImageChunk`. The datum builder must encode screenshots as Tinker image chunks rather than base64 strings.

6. **SL warm-start before RL**: Use `save_state` after a supervised learning phase, then `create_training_client_from_state_with_optimizer` to continue into RL without losing optimizer momentum.
