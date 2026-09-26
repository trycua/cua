# Cua-S1

Cua-S1 is a research project for studying small, specialist computer-use
models, scoped around a defined class of interface tasks rather than a
generally capable computer-use agent.

The project family currently has four checkpoints:

- `cua-s1-form-v0`, a finetuned, text-only variant of `cua-s1-nano-0.1`
  specialized for form-oriented user-interface tasks;
- `cua-s1-nano-0.1`, a from-scratch, ~855K-parameter option-attention
  classifier that scores every candidate (element, action) option for a
  screen state in a single forward pass;
- `cua-s1-4b-0.1`, a LoRA fine-tune on top of the frozen, open-weight
  `Qwen/Qwen3.5-4B` model for general computer-use element/action decisions;
  and
- `cua-s1-4b-0.2`, a separately trained pair of LoRA adapters on the same
  frozen `Qwen/Qwen3.5-4B` base, covering both the text and the multimodal
  modality, each with its own supervised stage and its own reinforcement
  learning stage against live GUI environments.

`cua-s1-4b-0.2` does not replace `cua-s1-4b-0.1`; both remain published.

None should be treated as a general-purpose assistant or as evidence of
reliable performance outside its evaluated task and environment boundaries.

## Project status

Cua-S1 is at an early research stage. This component includes Python model,
synthetic-data, training, evaluation, and optional Cua Driver integration code.
It does not include or download model weights, datasets, demo binaries, or
recordings.

Before evaluating or using a checkpoint, read [`MODEL_CARD.md`](MODEL_CARD.md)
for its intended scope and limitations and [`SECURITY.md`](SECURITY.md) for
deployment guidance.

The source code in this component is available under the repository's MIT
license. That license does not apply to future official model weights,
datasets, hosted services, or Cua trademarks.

## Python package

The Python distribution is named `cua-s1`, and its import name is `cua_s1`.
This source-only change does not publish the distribution to a package index.

Install the standalone development environment from this component:

```bash
uv sync --project libs/cua-s1/python --extra pdf --group test
uv run --project libs/cua-s1/python pytest libs/cua-s1/python/tests
```

The package exposes research primitives without downloading a model:

```python
import cua_s1

print(cua_s1.__version__)
```

Loading a `tiny`/`tinyx` checkpoint (e.g. `cua-s1-form-v0`) requires a local
`safetensors` file and matching JSON configuration. Pickle-based PyTorch
checkpoints are rejected. `cua-s1-4b-0.1` and `cua-s1-4b-0.2` are LoRA
adapters and use the standard PEFT on-disk layout instead
(`adapter_config.json` plus `adapter_model.safetensors`, one pair per
modality under a `text/` and a `multimodal/` subdirectory).

For `cua-s1-4b-0.1` or `cua-s1-4b-0.2` inference, install the `four-b`
extra. It supplies Transformers 5 and torchvision, which the Qwen3.5
processor needs for text and multimodal model loading:

```bash
uv sync --project libs/cua-s1/python --extra four-b
```

The `four-b` inference extra cannot be combined in one environment with
`nano-vision`, `four-b-train`, `four-b-rl`, or `all`. Those extras retain
their Transformers 4 dependency. In particular, the training and RL recipes
default to Qwen3.5, which that dependency cannot load; the recipes need a
separate compatibility update before they can train this base model. Use
separate environments for other extras, and do not treat the training extras
as a working Qwen3.5 setup.

`cua_s1.nano` (the `cua-s1-nano-0.1` architecture) supports a text-only
context modality with no extra dependencies, and an optional multimodal
context modality backed by a frozen vision backbone (`smolvlm` or `siglip`,
selected explicitly via a config field). Install the `nano-vision` extra to
use a vision backbone:

```bash
uv sync --project libs/cua-s1/python --extra nano-vision
```

## Get the weights and run inference

This section is the reproducible path from a clean checkout to one Cua-S1-4B
decision on a checked-in fixture. All published Cua-S1 artifacts are public on
Hugging Face and download without a token.

### Pinned artifacts

Pin every download to the commit SHA below. A branch name such as `main` can
move; these revisions are the ones the commands in this section were verified
against. The `cua-s1-4b-0.1`, `cua-s1-nano-0.1`, and `cua-s1-forms` pins were
updated on 2026-09-26 to revisions that add model cards and licenses (and, for
`cua-s1-forms`, remove the pickle checkpoint); their weight files are
byte-identical to the revisions measured on 2026-09-25.

| Artifact          | Hugging Face repository                                                   | Revision (commit SHA)                      | Pairs with                              | Download size                             | Declared license                             |
| ----------------- | ------------------------------------------------------------------------- | ------------------------------------------ | --------------------------------------- | ----------------------------------------- | -------------------------------------------- |
| Base model        | [`Qwen/Qwen3.5-4B`](https://huggingface.co/Qwen/Qwen3.5-4B)               | `851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a` | Base for both 4B adapters               | 9.34 GB (two safetensors shards, 9.32 GB) | Apache-2.0 (Qwen's model card and `LICENSE`) |
| `cua-s1-4b-0.2`   | [`cua-ai/cua-s1-4b-0.2`](https://huggingface.co/cua-ai/cua-s1-4b-0.2)     | `16818868b0cc7813808aae4e87b417657046ab79` | `Qwen/Qwen3.5-4B` at the revision above | 187 MB                                    | Apache-2.0 (adapter only)                    |
| `cua-s1-4b-0.1`   | [`cua-ai/cua-s1-4b-0.1`](https://huggingface.co/cua-ai/cua-s1-4b-0.1)     | `3ebffb9868f31a1d54140948cb2434d35f35b281` | `Qwen/Qwen3.5-4B` at the revision above | 272 MB                                    | Apache-2.0 (adapter only)                    |
| `cua-s1-nano-0.1` | [`cua-ai/cua-s1-nano-0.1`](https://huggingface.co/cua-ai/cua-s1-nano-0.1) | `abbd98492307dc20373f79f7141725412df63c42` | Standalone (no base model)              | 6.9 MB                                    | Apache-2.0                                   |
| `cua-s1-forms`    | [`cua-ai/cua-s1-forms`](https://huggingface.co/cua-ai/cua-s1-forms)       | `4a7a9f42a3d42e6dfbd111c0c843e37ac50f1332` | Standalone (no base model)              | 2.8 MB                                    | MIT                                          |

Layouts and loaders:

- Both 4B repositories contain two PEFT adapters, `text/` and `multimodal/`,
  each with `adapter_config.json` and `adapter_model.safetensors`. Both
  adapter configurations name `Qwen/Qwen3.5-4B` as the base (LoRA rank 16).
  Pass the repository root as the adapter path; `cua_s1.four_b.FourBModel`
  selects the subdirectory for its `modality`. `cua-s1-4b-0.1` also carries a
  root-level `adapter_model.safetensors` that is byte-identical to its
  `text/` adapter.
- `cua-s1-nano-0.1` contains `text/` and `multimodal/` checkpoints
  (`model.safetensors` plus `config.json`, 855,296 parameters each). Load a
  subdirectory with `cua_s1.nano.load_nano_checkpoint`. The multimodal
  checkpoint also needs a frozen vision backbone from the `nano-vision` extra.
- `cua-s1-forms` contains a `tinyx` checkpoint (`cua-s1-forms.safetensors`
  plus `cua-s1-forms.json`, 706,048 parameters). Load it with
  `cua_s1.model.load_checkpoint`. At the pinned revision the repository
  contains only that pair, a model card, and `.gitattributes`. Older
  revisions, including `f54adbf447f4ca6ec259f529ee3f2e3e09f8cc71`, also ship
  a pickle `cua-s1-forms.pt`, which `cua_s1` refuses to load by design (see
  #3977). If you pin an older revision, pass `--exclude "*.pt"`. Unpickling a
  checkpoint can run arbitrary code, so never open a `.pt` copy with
  `torch.load` or `pickle`.
- The Apache-2.0 licenses on `cua-s1-4b-0.2` and `cua-s1-4b-0.1` cover only
  the adapters. The base model is governed by its own license and is
  downloaded from Qwen's repository, not redistributed by Cua.
  `cua-s1-nano-0.1` is trained from scratch and its weights are Apache-2.0;
  the frozen vision backbone its multimodal checkpoint downloads at first use
  is not redistributed and is governed by its own license.

### Hardware

The 4B base weights are stored in 16-bit precision and occupy 9.32 GB, so the
loaded model needs more than that in RAM, unified memory, or GPU memory before
activations. An 8 GB host or guest cannot run Cua-S1-4B inference, which
includes the default 8 GB macOS guests used for desktop E2E. Plan for at least
16 GB of free memory. That figure is an estimate from the weight size; peak
memory has not been measured.

The only measured 4B runs so far used one Apple M1 Ultra host with 128 GB of
unified memory, macOS 26, Python 3.12, torch 2.14.0, and Transformers 5.17.0
from the `four-b` lock, with sequential weight loading on `mps` (see
[Known local issues](#known-local-issues)). Each decision is one forward pass;
times are medians of five warm calls:

| Adapter and modality       | Device and dtype  | Load time | 3-candidate fixture | 12-region, 8-candidate request |
| -------------------------- | ----------------- | --------- | ------------------- | ------------------------------ |
| `cua-s1-4b-0.2` text       | `mps`, `float16`  | 16.3 s    | 1.07 s              | 2.83 s                         |
| `cua-s1-4b-0.2` text       | `mps`, `bfloat16` | 6.8 s     | 1.23 s              | 3.23 s                         |
| `cua-s1-4b-0.2` multimodal | `mps`, `float16`  | 4.8 s     | 5.55 s              | 5.81 s                         |
| `cua-s1-4b-0.2` multimodal | `mps`, `bfloat16` | 3.7 s     | 6.70 s              | 6.66 s                         |
| `cua-s1-4b-0.2` text       | `cpu`, `float32`  | 18.3 s    | 19.5 s              | 32.1 s                         |
| `cua-s1-4b-0.2` multimodal | `cpu`, `float32`  | 7.8 s     | 34.2 s              | 50.2 s                         |

Multimodal rows used 1280x800 screenshots; a 2560x1600 screenshot took 22 to
30 s per decision on `mps`, so downscale large captures before scoring. The
CPU rows ran while other workloads kept the host's load average near 16, so
treat them as an upper bound rather than a CPU benchmark. Load times depend
on whether the weights were already in the operating-system file cache; rows
measured after the first load were faster. CUDA latency and Linux or Windows
hosts have not been measured. Qwen3.5 logs that it falls back to reference
PyTorch kernels because `causal_conv1d` and `flash-linear-attention` are not
installed; both are CUDA-oriented packages.

Plan live loops around the Cua Driver's 60-second capture lifetime: the
decision must return, and the action must be dispatched with the same
`capture_id`, before the capture expires. A cold `choose_decision.py`
invocation, which loads the model for one decision, took 10 to 35 s of wall
time on `mps` and 30 to 43 s on `cpu`. Keep one process with a loaded
`FourBModel` resident (for example, a small local service) and send each
decision to it; warm `mps` decisions then take about 1 to 3 s for text and 6 s
for multimodal. On CPU, a single warm decision can take 20 to 50 s, which leaves
little or no margin inside the capture lifetime.

### 1. Create the pinned environment

From the repository root, create the inference environment from the
checked-in lock on Python 3.11, 3.12, or 3.13 (change `--python` as needed):

```bash
uv sync --frozen --project libs/cua-s1/python --python 3.12 \
  --extra four-b --extra pdf --group test
```

The lock resolves torch 2.14.0, Transformers 5.17.0, PEFT 0.21.0, and
torchvision 0.29.0. The `pdf` extra is optional for inference; it adds PDF
entity extraction and its tests. The `four-b` extra cannot share an
environment with `nano-vision`, `four-b-train`, `four-b-rl`, or `all`.

Confirm the environment without loading any model weights:

```bash
uv run --frozen --project libs/cua-s1/python --extra four-b --extra pdf --group test \
  pytest libs/cua-s1/python/tests
```

### 2. Download the weights

The `hf` command-line tool is installed with `huggingface_hub`, which the
`four-b` environment already contains. Choose a models directory with at
least 10 GB free:

```bash
HF=libs/cua-s1/python/.venv/bin/hf
S1_MODELS="$HOME/cua-s1-models"

"$HF" download Qwen/Qwen3.5-4B \
  --revision 851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a \
  --local-dir "$S1_MODELS/Qwen3.5-4B"
"$HF" download cua-ai/cua-s1-4b-0.2 \
  --revision 16818868b0cc7813808aae4e87b417657046ab79 \
  --local-dir "$S1_MODELS/cua-s1-4b-0.2"
```

Download the other checkpoints the same way as needed:

```bash
"$HF" download cua-ai/cua-s1-4b-0.1 \
  --revision 3ebffb9868f31a1d54140948cb2434d35f35b281 \
  --local-dir "$S1_MODELS/cua-s1-4b-0.1"
"$HF" download cua-ai/cua-s1-nano-0.1 \
  --revision abbd98492307dc20373f79f7141725412df63c42 \
  --local-dir "$S1_MODELS/cua-s1-nano-0.1"
"$HF" download cua-ai/cua-s1-forms \
  --revision 4a7a9f42a3d42e6dfbd111c0c843e37ac50f1332 \
  --local-dir "$S1_MODELS/cua-s1-forms"
```

Add `--dry-run` to any command to list the files and total size without
downloading. The equivalent Python call is
`huggingface_hub.snapshot_download(repo_id, revision=<sha>, local_dir=<dir>)`.

### 3. Run the smoke decision

The [jev-use closed-candidate chooser](../cua-driver/examples/jev-use/decision-models.md)
loads Cua-S1-4B from local directories. It reads these environment variables:

| Variable              | Required | Default   | Meaning                                                                 |
| --------------------- | -------- | --------- | ----------------------------------------------------------------------- |
| `S1_BASE_MODEL_PATH`  | Yes      | None      | Local directory containing `Qwen/Qwen3.5-4B`                            |
| `S1_ADAPTER_PATH`     | Yes      | None      | Local adapter root, such as `cua-s1-4b-0.2`                             |
| `S1_DEVICE`           | No       | `cpu`     | Torch device passed to Transformers, such as `cpu`, `cuda`, or `mps`    |
| `S1_DTYPE`            | No       | `float16` | Torch dtype name, such as `float16`, `bfloat16`, or `float32`           |
| `S1_MODALITY`         | No       | `text`    | `text` or `multimodal`; `--s1-modality` overrides it                    |
| `S1_ADAPTER_ID`       | No       | Detected  | Adapter name reported in `model`, such as `cua-ai/cua-s1-4b-0.2`        |
| `S1_ADAPTER_REVISION` | No       | Detected  | Adapter revision reported in `model` when it cannot be detected locally |

The multimodal adapter also needs `--screenshot <png>` and
`--screenshot-capture-id <capture_id>` from the same capture as the request.
The chooser supports only the 4B adapters; `cua-s1-nano-0.1` and
`cua-s1-forms` fail setup. It rejects paths that are not existing directories
and never downloads weights itself.

From the repository root, run the verifier. It pipes the checked-in
`fixtures/jev-choice-request-v1.json` request to `choose_decision.py` with the
same Python interpreter, validates the response, and prints a summary:

```bash
export S1_BASE_MODEL_PATH="$S1_MODELS/Qwen3.5-4B"
export S1_ADAPTER_PATH="$S1_MODELS/cua-s1-4b-0.2"
export S1_DEVICE=cpu S1_DTYPE=float16   # on Apple silicon: S1_DEVICE=mps

libs/cua-s1/python/.venv/bin/python \
  libs/cua-driver/examples/jev-use/verify_decision_cli.py --model s1
```

Expected output on success:

```text
{"model": "cua-s1-4b-0.2@16818868b0cc7813808aae4e87b417657046ab79:text", "kind": "selected", "selected_id": "submit-form"}
```

The verifier exits nonzero if the response is not a `cua.decision_choice_v1`
object, names a different `capture_id`, omits or adds a candidate, or selects
anything other than `submit-form`. To check the checked-in negative fixture,
add `--fixture negative --expected-id abstain`. To run the multimodal
adapter, add `--screenshot <png>` with an image of the fixture's form; the
verifier binds it to the fixture's capture ID.

To see the full response, run the chooser directly:

```bash
libs/cua-s1/python/.venv/bin/python \
  libs/cua-driver/examples/jev-use/python/choose_decision.py --model s1 \
  < libs/cua-driver/examples/jev-use/fixtures/jev-choice-request-v1.json
```

It prints one JSON object with this shape; the probabilities are
model-dependent:

```text
{"schema":"cua.decision_choice_v1","kind":"selected","capture_id":"capture-fixture-1","selected_id":"submit-form","model":"cua-s1-4b-0.2@16818868b0cc7813808aae4e87b417657046ab79:text","confidence":0.97,"probabilities":{"submit-form":0.97,"reobserve":0.02,"abstain":0.01},"reason":null}
```

`kind` is one of `selected`, `reobserve`, `abstain`, or `error`. The
`probabilities` keys are exactly the request's candidate IDs. `model` is
`<adapter>[@<revision>]:<modality>`; the revision comes from the metadata that
`hf download --local-dir` writes or from a Hugging Face cache snapshot path,
and is omitted when neither is present. The base model is not part of it, so
record the base revision with your evidence. Replace `--model s1` with `--model mock` in either command to
check the plumbing without weights.

This smoke proves that the environment, pinned weights, and chooser load and
produce a valid decision. It is not a quality measurement: on the same
positive fixture, the base model without an adapter also selected
`submit-form`, and `cua-s1-4b-0.1` abstained. See the
[model card](MODEL_CARD.md#verification-scope-and-known-failure-modes) for
the recorded results and limits.

### Known local issues

- On Apple silicon, Transformers 5's concurrent weight loader crashed with
  `SIGSEGV` or hung when loading the 4B base with `mps` and `float16`
  (#4198). `FourBModel` now sets `HF_DEACTIVATE_ASYNC_LOAD=1` for `mps`
  loads, so `S1_DEVICE=mps` works with the default `float16` and with
  `bfloat16`. If you set `HF_DEACTIVATE_ASYNC_LOAD` yourself, your value is
  kept; do not set it to a false value on `mps`.
- The chooser supports at most 26 candidates, including `reobserve` and
  `abstain`. A larger request returns `kind: "error"` with
  `reason: "option_limit"`.

A checkpoint distributed as a pickle archive must be converted once before it
can be loaded. `cua-s1-convert` reads the archive with `weights_only=True`, so
no pickled code object is executed, and writes the checked pair:

```bash
uv run --project libs/cua-s1/python cua-s1-convert model.pt ./checkpoint
```

Convert only archives you trust, then load the resulting directory.

## Safety boundary

Planning and execution are separate. The optional runtime defaults to a dry
run, requires one unambiguous target window, uses snapshot-bound element
tokens, and reobserves the window after each mutation. `execute` and `submit`
are independent opt-ins. PDF access is confined to configured allowed roots.
Without explicit configuration, the library and MCP server use their current
working directory as the allowed root. Production deployments should use a
dedicated, least-privilege directory.

Submission is deliberately narrow: `submit=true` permits at most one
high-confidence `Button` or `AXButton` whose normalized label is exactly
`Submit` or `Submit Form`.

## Optional MCP server

The `cua-s1-mcp` command is an advanced integration surface, not a configured
model service. Install the optional dependencies before running it:

```bash
uv sync --project libs/cua-s1/python --extra mcp --extra pdf
```

The server uses the MCP stdio transport and requires these host settings:

- `CUA_S1_PLANNER_FACTORY` identifies trusted Python code in
  `module:attribute` form. Importing the factory executes code with the server
  process's privileges, so do not point it at untrusted modules.
- `CUA_S1_ALLOWED_PDF_ROOTS` is an operating-system path-separated list of
  directories that the server may read. If it is unset, the server uses its
  current working directory.
- `CUA_S1_DRIVER_BINARY`, `CUA_S1_DRIVER_TRANSPORT`, and `CUA_S1_SESSION` can
  override the Cua Driver executable, transport, and session.

The connected Cua Driver must provide exact-window snapshots, snapshot-bound
element tokens, and confirmed action effects. The portable Cua Driver contract
does not currently expose `set_value`, so fill execution fails closed unless
the connected runtime explicitly advertises compatible token-based value
mutation. Planning remains available without executing mutations.

Treat MCP tool results and stdio logs as sensitive. They can contain values
extracted from PDFs, form labels, window metadata, and values selected for
entry.

## Checkpoints

| Checkpoint        | Scope                                                                                                                                                                               | Status                                                                                                                                                                             |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cua-s1-form-v0`  | Form-oriented computer-use research (finetuned, text-only variant of `cua-s1-nano-0.1`)                                                                                             | Profile defined; a related `tinyx` form checkpoint is published at [`cua-ai/cua-s1-forms`](https://huggingface.co/cua-ai/cua-s1-forms) (see [Pinned artifacts](#pinned-artifacts)) |
| `cua-s1-nano-0.1` | General closed-option GUI decision research (element + action selection)                                                                                                            | Profile defined; weights at [`cua-ai/cua-s1-nano-0.1`](https://huggingface.co/cua-ai/cua-s1-nano-0.1)                                                                              |
| `cua-s1-4b-0.1`   | General computer-use element/action decisions (LoRA on frozen `Qwen/Qwen3.5-4B`), text and multimodal (screenshot) input                                                            | Profile defined; adapter weights at [`cua-ai/cua-s1-4b-0.1`](https://huggingface.co/cua-ai/cua-s1-4b-0.1)                                                                          |
| `cua-s1-4b-0.2`   | General computer-use element/action decisions (LoRA on frozen `Qwen/Qwen3.5-4B`), text and multimodal (screenshot) input, plus agentic multi-step rollouts in live GUI environments | Profile defined; adapter weights at [`cua-ai/cua-s1-4b-0.2`](https://huggingface.co/cua-ai/cua-s1-4b-0.2)                                                                          |

Do not assume that results transfer across applications, operating systems,
languages, layouts, accessibility settings, or task distributions.

## Evaluation

The included offline metrics distinguish accuracy, abstention, coverage, wrong
actions, wrong targets, and actions taken when the expected behavior was to
abstain. Synthetic train, validation, and test splits are separated by form
signature.

## Responsible use

Run computer-use models in isolated environments with least-privilege
credentials, explicit action boundaries, and independent verification of
important outcomes. Require human review before consequential, irreversible,
financial, legal, medical, account, permission, or external-communication
actions.

Report suspected vulnerabilities through the process in
[`SECURITY.md`](SECURITY.md).
