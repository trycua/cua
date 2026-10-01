# Offline evaluation scaffold

`metrics.evaluate_predictions` compares normalized JSON-like records from any
model or provider. It never performs inference or network access.

Gold records require a stable `id`, `action`, and optional `target`. Ambiguous
cases may instead include an `acceptable` list:

```json
{"id":"city","acceptable":[{"action":"fill","target":2},{"action":"fill","target":5}]}
```

Prediction records use the same `id`, `action`, and optional `target` shape.
Missing predictions are scored as abstentions. The report separates exact
accuracy, coverage, selective accuracy, wrong actions, wrong targets, and
actions taken where the gold behavior required abstention.

## Metric definitions

Every rate in an `evaluate_predictions` report is computed over the gold records
in the run.

| Field | Definition |
| --- | --- |
| `examples` | Number of gold records scored. |
| `accuracy` | Share of records whose prediction exactly matches an acceptable gold `(action, target)` choice. |
| `coverage` | Share of records where the model acted instead of abstaining. |
| `abstention_rate` | Share of records scored as abstentions. Missing predictions count as abstentions. |
| `selective_accuracy` | Accuracy restricted to the records where the model acted. |
| `wrong_action_rate` | Share of records where the model acted with an action outside the acceptable set. |
| `wrong_target_rate` | Share of records where the action was acceptable but the target or entity index was not. |
| `unsafe_action_rate` | Share of records where the model acted even though the gold behavior required abstention. |
| `counts` | Raw counters behind the rates: `total`, `correct`, `abstained`, `acted`, `acted_correct`, `wrong_action`, `wrong_target`, `unsafe_action`. |
| `per_action` | `accuracy`, `abstention_rate`, and `examples` per gold action, plus an `ambiguous` bucket for golds with multiple acceptable choices. |

`train.py eval` reports a different view of a checkpoint: `top1`, `nll`, `ece`
(10-bin expected calibration error), per-action accuracy, and throughput
(`rows_per_second`, excluded from deterministic comparisons).

## Reproducing results

Run from the repository root:

```bash
# Install the development environment used by the component tests
uv sync --project libs/cua-s1/python --extra pdf --group test

# Generate the synthetic splits (deterministic for a fixed --seed)
uv run --project libs/cua-s1/python python -m cua_s1.synth --output data/cua-s1

# Train the default tinyx scorer on the generated train/validation splits
uv run --project libs/cua-s1/python python libs/cua-s1/training/train.py train \
  data/cua-s1/train.jsonl \
  --validation data/cua-s1/validation.jsonl \
  --output runs/cua-s1/model

# Evaluate the saved checkpoint on the held-out test split
uv run --project libs/cua-s1/python python libs/cua-s1/training/train.py eval \
  runs/cua-s1/model data/cua-s1/test.jsonl
```

Synthetic generation writes `train.jsonl`, `validation.jsonl`, `test.jsonl`, and
a `manifest.json` with artifact hashes and row counts under the output
directory. Training and evaluation are deterministic for a fixed `--seed`
unless `--non-deterministic` is passed; the test split is disjoint by form
signature and model selection uses validation results only.

To score arbitrary gold/prediction records with `evaluate_predictions`, run from
`libs/cua-s1` so the `evals` package is importable, for example:

```bash
cd libs/cua-s1
uv run --project python python - <<'PY'
import json

from evals.metrics import evaluate_predictions

with open("gold.jsonl", encoding="utf-8") as handle:
    gold = [json.loads(line) for line in handle if line.strip()]
with open("predictions.jsonl", encoding="utf-8") as handle:
    predictions = [json.loads(line) for line in handle if line.strip()]

print(json.dumps(evaluate_predictions(gold, predictions), indent=2, sort_keys=True))
PY
```

Checkpoints are safetensors weights plus a matching JSON configuration (legacy
pickle files are rejected). The published `cua-s1-forms` artifacts are on
Hugging Face at <https://huggingface.co/cua-ai/cua-s1-forms>; numbers are only
comparable when the task set, splits, seed, and checkpoint revision match.
