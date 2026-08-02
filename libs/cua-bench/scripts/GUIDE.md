# Ohm-Bench: Packaging & Evaluation Guide

## Prerequisites

```bash
cd libs/cua-bench
uv sync
echo "ANTHROPIC_API_KEY=sk-ant-..." > .env
echo "OPENAI_API_KEY=sk-..." >> .env
```

Build the environment image (KiCad 9 pre-installed):
```bash
cd libs/xfce
docker build -t trycua/cua-xfce:latest .
```

---

## 1. Packaging a Submission

Converts a DaaS submission JSON into a self-contained `scripts/tasks/<id>/` directory.

```bash
uv run python scripts/package_kicad_submission.py <submission.json> --output scripts/tasks/
```

**Input JSON fields required:**
- `submission_id` — unique ID (becomes the task directory name)
- `circuit_prompt` — task description shown to the agent
- `netlist.s3Uri` — S3 URI of the reference netlist zip
- `circuit_pcb_file.s3Uri` — S3 URI of the initial KiCad project zip

**Output structure:**
```
scripts/tasks/<submission_id>/
  main.py          # cb task harness
  reference.net    # ground-truth KiCad netlist
  initial/         # initial KiCad project files shown to agent
    <project>/
      *.kicad_sch
      *.kicad_pcb
      ...
```

---

## 2. Running Evaluations

### Single task (quick test)

```bash
uv run cb run dataset scripts/tasks --task-filter "<submission_id>" \
  --agent cua-agent --model anthropic/claude-opus-4-6 --max-steps 150
```

### Multiple specific tasks

```bash
uv run cb run dataset scripts/tasks \
  --task-filter "154d0750,1625e97a,2c11fe94" \
  --agent cua-agent --model anthropic/claude-opus-4-6 \
  --max-parallel 5 --max-steps 150
```

### Full dataset

```bash
uv run cb run dataset scripts/tasks \
  --agent cua-agent --model anthropic/claude-opus-4-6 \
  --max-parallel 5 --max-steps 150
```

### Monitor a run

```bash
uv run cb run watch <run_id>
uv run cb run info <run_id>
```

---

## 3. Calibration (N attempts per task per model)

Runs each task N times with Claude and N times with OpenAI, then computes pass rates and difficulty tiers.

```bash
# Specific tasks, 5 attempts each, 1000 max steps
uv run python scripts/run_calibration.py \
  --tasks "154d0750,1625e97a,2c11fe94,34877be3,458f5f5c" \
  --attempts 5 \
  --max-steps 1000
```

```bash
# All tasks (runs sequentially: all claude attempts, then all openai attempts)
uv run python scripts/run_calibration.py \
  --tasks "$(ls scripts/tasks | grep -v '.json' | tr '\n' ',' | sed 's/,$//')" \
  --attempts 5 \
  --max-steps 1000
```

**What it does per task:**
1. Launches `--attempts` simultaneous `cb run dataset` calls with `anthropic/claude-opus-4-6`
2. Waits for all to finish
3. Launches `--attempts` simultaneous calls with `openai/computer-use-preview`
4. Waits for all to finish
5. Moves to next task

---

## 4. Cleaning Up

Kill all running containers between runs:
```bash
docker ps -a --format "{{.Names}}" | grep "cua-" | xargs -r docker rm -f
```

Stop all active cb runs:
```bash
uv run cb run list | grep "^[a-f0-9]" | awk '{print $1}' | xargs -I{} uv run cb run stop {}
```
