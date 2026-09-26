# Cua-S1 model card

## Model family

Cua-S1 is a research family of small, specialist computer-use models. Each
checkpoint has a narrow task contract and checkpoint-specific evaluation
evidence; membership in the family does not imply general computer-use
capability. Four checkpoints, documented below: `cua-s1-form-v0`,
`cua-s1-nano-0.1`, `cua-s1-4b-0.1`, and `cua-s1-4b-0.2`. `cua-s1-4b-0.2` does
not supersede or replace `cua-s1-4b-0.1`; both are distributed and
documented independently.

## Checkpoint: cua-s1-form-v0

**Relationship to `cua-s1-nano-0.1`:** a finetuned, text-only variant of the
`cua-s1-nano-0.1` option-attention architecture, specialized to
form-oriented user-interface tasks.

**Intended scope:** research on form-oriented user-interface tasks.

**Status:** research profile. The public Hugging Face repository
[`cua-ai/cua-s1-forms`](https://huggingface.co/cua-ai/cua-s1-forms)
(revision `f54adbf447f4ca6ec259f529ee3f2e3e09f8cc71`, MIT) publishes a
related `tinyx` form checkpoint with 706,048 parameters. Its safetensors and
JSON pair loads with `cua_s1.model.load_checkpoint`; its pickle `.pt` copy is
refused by design (#3977). The results on that repository's card are its
own and are not reproduced in this component. No training dataset or
checkpoint artifact manifest is distributed here.

**Model design:** the reference `tinyx` configuration uses a byte-level
transformer encoder with an option-attention classification head. For each
interface element, the model selects one option from a fixed set: fill from
an extracted entity, check, click, or skip. The prototype scores interface
elements independently and does not generate field values; its document
parser only extracts values represented as `Label: value` pairs.

**Training data:** the included generator creates fictional form episodes
using reserved phone numbers, `.invalid` domains, invalid test identifiers,
and fictional brands. No real user submission dataset is included.

**Intended uses:** research on narrowly specified form-oriented
computer-use tasks; evaluation of specialist-model behavior in isolated,
controlled environments; study of task-specific failure modes,
verification, and human oversight.

**Out-of-scope uses:** general-purpose or open-ended computer operation;
unsupervised operation on production accounts or sensitive data; actions
with financial, legal, medical, employment, safety, or other high-impact
consequences; bypassing access controls, consent, rate limits, or service
policies; treating model output or apparent task completion as proof that
an action was correct or successful.

## Checkpoint: cua-s1-4b-0.1

**Base model:** a LoRA (low-rank adapter) fine-tune on top of the frozen,
openly licensed [`Qwen/Qwen3.5-4B`](https://huggingface.co/Qwen) checkpoint;
not the `tinyx` architecture. Only the LoRA adapter weights are a Cua-S1
artifact.

**Intended scope:** general computer-use decisions -- given a description of
the current screen state (an accessibility tree, or a screenshot) and a
fixed, closed set of candidate (element, action) options, select the single
best option. Text-only (accessibility tree) and multimodal (screenshot)
modes are two independently trained LoRA adapters, published as `text/` and
`multimodal/` subdirectories under the same HF repo;
`cua_s1.four_b.FourBModel` selects the right one for the requested
`modality`. The multimodal adapter was trained only on the 6 core GUI
families' synthetic/same-distribution data and does not generalize to
out-of-distribution families such as `chess` or `game_control` (see
Evaluation below).

**Status:** research profile. Inference-only Python implementation
(`cua_s1.four_b`). Adapter weights at
[`cua-ai/cua-s1-4b-0.1`](https://huggingface.co/cua-ai/cua-s1-4b-0.1),
pinned to revision `88d8b8a90c2da4470d005cc23ec8665a6442ebe1`. At that
revision the repository declares no license and has no model card.

**Decoding contract:** a chat-template prompt asks the model to answer with
a single letter identifying the chosen option; each option letter is a
verified single tokenizer token; one forward pass is run; final-position
logits at the option-letter token positions are softmaxed into per-option
probabilities. Closed-set selection, not open-ended generation.

**Evaluation:** real, measured results are published in
`libs/cua-bench-s1/README.md`'s Results section. As of that section: on the
held-out cross-dataset text split, task-level accuracy ranges 0.167-0.571
depending on family; on the same-distribution multimodal split for the 6
core families, 1.000; on `chess` (15-of-800 positions, this checkpoint's
26-option decoding cap), 0.000 task accuracy and 0.227-0.515 element
accuracy depending on modality; on `game_control`, 0.000 task accuracy and
0.333 element accuracy (neither survives chance-correction -- see the
calibrated table); on `safety_gate`, 0.071 zero-shot and 1.000 finetuned on
that family's own split; on the external `general_decision` benchmark,
0.563 zero-shot. See `libs/cua-bench-s1/docs/TASK_FAMILIES.md` for what each
family measures. The included tests exercise the prompt-building,
letter-assignment, and logit-decoding implementation, not checkpoint
quality.

**Out-of-scope uses:** the same as `cua-s1-form-v0` above. In addition,
because this checkpoint selects only among caller-enumerated options, it
must not be treated as evidence that the option set itself was safe,
complete, or correctly scoped.

## Checkpoint: cua-s1-nano-0.1

**Intended scope:** research on general closed-option computer-use GUI
decisions, not specific to form-filling.

**Status:** research profile. Source code (`cua_s1.nano`). Weights at
[`cua-ai/cua-s1-nano-0.1`](https://huggingface.co/cua-ai/cua-s1-nano-0.1),
pinned to revision `1f93fd0fdcbe33740334948f967dff9f6c8e9f34`. At that
revision the repository declares no license and has no model card.

**Model design:** a from-scratch, single-pass option-attention classifier
with approximately 855,000 trainable parameters. Scores every candidate
(element, action) option in one parallel forward pass and selects the
highest-scoring option per element. Two context modalities: text (a small
trainable byte-level transformer over a rendered accessibility-tree excerpt
per element) and multimodal (a frozen vision backbone, SmolVLM-256M or
SigLIP-base, over a screenshot crop per element, with a small trainable
projection layer). Candidate options are always encoded as text by a shared
option encoder. `cua_s1.nano` includes inference and checkpoint-loading
code only, not the training loop or data-generation code.

**Training data:** intended to be evaluated on a benchmark spanning
synthetic GUI task families, real-world GUI task families derived from
AndroidControl and GUI-360, and out-of-domain checks (chess,
ViZDoom-based game control). No dataset is distributed with this component.

**Intended uses:** research on general closed-option computer-use GUI
decision-making; evaluation of a small, from-scratch specialist
architecture against models in comparable size classes; study of
task-family generalization, including out-of-domain checks.

**Out-of-scope uses:** same categories as above (general/open-ended
operation, unsupervised production use, high-impact actions, bypassing
access controls, treating output as proof of correctness).

**Limitations:** requires a closed, pre-enumerated option set per element;
cannot propose an action outside that set. Its small parameter count trades
capacity for speed. Vision-modality behavior depends on the frozen
backbone's own limitations and screenshot-crop quality. Game-control and
board-game families are out-of-domain generalization checks only.

**Evaluation:** real, measured results are published in
`libs/cua-bench-s1/README.md`'s Results section. As of that section: on the
held-out cross-dataset text split, task-level accuracy ranges 0.000-0.286
depending on family; on the same-distribution multimodal split, 1.000; on
`chess` (15-of-800 positions), 0.000 task accuracy and 0.227 element
accuracy in both modalities; on `game_control`, 0.000 task accuracy and
0.333 element accuracy (neither survives chance-correction); on
`safety_gate`, 0.000 zero-shot and 1.000 finetuned. No text-modality shape
for the external `general_decision` benchmark. Qualitatively, this
architecture is fast (small-millisecond per-task latency on GPU, under
100ms on CPU) because scoring is a single parallel forward pass rather than
autoregressive generation.

## Checkpoint: cua-s1-4b-0.2

**Base model:** like `cua-s1-4b-0.1`, a LoRA fine-tune on the frozen,
openly licensed [`Qwen/Qwen3.5-4B`](https://huggingface.co/Qwen) checkpoint.
A separate artifact from `cua-s1-4b-0.1`, which remains published.

**Intended scope:** the same closed-option decision contract as
`cua-s1-4b-0.1`, in both text and multimodal modes. Unlike `cua-s1-4b-0.1`,
both modalities are trained and evaluated on real, held-out cross-dataset
GUI data, and both have a reinforcement-learning stage against live GUI
environments, so the checkpoint is also exercised as a multi-step agentic
policy.

**Adapter layout:** two fully independently trained LoRA adapters, each
with its own supervised stage and its own RL stage, published as `text/`
and `multimodal/` subdirectories under the same HF repo, exactly as
`cua-s1-4b-0.1` is.

**Status:** research profile. Inference-only Python implementation
(`cua_s1.four_b`). Adapter weights at
[`cua-ai/cua-s1-4b-0.2`](https://huggingface.co/cua-ai/cua-s1-4b-0.2),
pinned to revision `16818868b0cc7813808aae4e87b417657046ab79`, Apache-2.0
for the adapter only.

**Decoding contract:** identical to `cua-s1-4b-0.1`'s.

**Evaluation:** real, measured results are published in
`libs/cua-bench-s1/README.md`'s Results section. As of that section: on the
held-out cross-dataset text split, task-level accuracy is 0.833-1.000 across
five of six core GUI families and 0.429 on `pagination`, for 0.875 overall
(ECE 0.121); on the held-out cross-dataset multimodal split
(accessibility tree stripped), 0.750-1.000 per family for 0.929 overall
(ECE 0.069); on the real, live `cua-bench-basic` environments (agentic,
20-step cap, held-out task variants), episode success rate is 0.944 text /
0.722 multimodal.

Caveats: the `pagination` row (7 tasks text, 1 multimodal) is mislabeled
mid-episode scroll steps, not a genuine pagination measurement; text
`form_filling` at 0.939 ties the strongest baseline; the 4-task multimodal
`consent_checkbox` row is a tie, not a win; in the agentic multimodal
setting, `toggle-switch` is 0/3 (under-trained, not solved). This checkpoint
is not measured on `safety_gate`, `chess`, `game_control`,
`general_decision` or `osworld_next_action` in either modality --
unmeasured, not known-good and not known-bad.

**Out-of-scope uses:** the out-of-scope uses listed above for
`cua-s1-form-v0` and `cua-s1-4b-0.1` apply unchanged, including that
selecting among caller-enumerated options is not evidence the option set
was itself safe, complete or correctly scoped. The agentic results
additionally do not license unsupervised multi-step operation: they are
measured on 13 single-widget research environments under a hard step cap,
with 6 excluded because they are not reliably rewardable under the provider
used.

## Verification scope and known failure modes

This section records what has been checked outside the offline benchmark and
what has not. The download and setup commands are in the README's
[Get the weights and run inference](README.md#get-the-weights-and-run-inference)
section.

**Pinned inputs.** Every result below used `Qwen/Qwen3.5-4B` at
`851bf6e806efd8d0a36b00ddf55e13ccb7b8cd0a`, `cua-s1-4b-0.2` at
`16818868b0cc7813808aae4e87b417657046ab79`, and `cua-s1-4b-0.1` at
`88d8b8a90c2da4470d005cc23ec8665a6442ebe1`, loaded by `cua_s1.four_b` through
the jev-use closed-candidate chooser with the `four-b` lock (torch 2.14.0,
Transformers 5.17.0, PEFT 0.21.0).

**Local fixture runs (2026-09-25).** One Apple M1 Ultra host with 128 GB of
unified memory, macOS 26, Python 3.12, `mps`. Each row is one deterministic
decision per fixture; the value is the selected option's probability. The
positive and negative requests are the checked-in
`libs/cua-driver/examples/jev-use/fixtures/jev-choice-request-v1.json` and
`jev-choice-negative-v1.json`. The 12-region, 8-candidate request and the
synthetic screenshots used for the multimodal rows are not checked in.

| Model and modality                  | dtype      | Positive (expects `submit-form`) | Negative (expects `abstain`) | 8-candidate (expects `click-r11`) |
| ----------------------------------- | ---------- | -------------------------------- | ---------------------------- | --------------------------------- |
| `cua-s1-4b-0.2` text                | `float16`  | `submit-form` 0.975              | `abstain` 0.430              | `click-r11` 0.932                 |
| `cua-s1-4b-0.2` text                | `bfloat16` | `submit-form` 0.973              | `abstain` 0.455              | not run                           |
| `cua-s1-4b-0.2` multimodal          | `float16`  | `submit-form` 0.979              | `abstain` 0.857              | `click-r11` 0.984                 |
| `cua-s1-4b-0.1` text                | `float16`  | `abstain` 0.418 (wrong)          | `abstain` 0.437              | `click-r9` 0.159 (wrong)          |
| `cua-s1-4b-0.1` multimodal          | `float16`  | `abstain` 0.561 (wrong)          | `abstain` 0.569              | `click-r0` 0.283 (wrong)          |
| `Qwen/Qwen3.5-4B`, no adapter, text | `float16`  | `submit-form` 0.426              | `abstain` 0.745              | `click-r2` 0.523 (wrong)          |

These are runtime smokes on three synthetic requests, not an accuracy
estimate. The base model without an adapter passes the two checked-in
fixtures, so passing them does not by itself show adapter quality.

**Desktop counterexample.** In an earlier private primary-desktop probe,
packaged OmniParser observed `Send`, the only action candidate required an
exact `Save`, and `cua-s1-4b-0.2` text selected that action with probability
0.777 instead of abstaining. No click was dispatched. A caller must not treat
a high S1 score as proof that a candidate's condition holds; see
`libs/cua-driver/examples/jev-use/decision-models.md`.

**Not yet covered.**

- Cua-S1 is not yet covered by canonical Cua Driver desktop E2E on any
  platform. The fixture runs above involve no Cua Driver session, so no
  Driver version applies to them.
- No CI job loads real weights. `CI: cua-s1` runs the unit suite with fake
  models on Ubuntu with Python 3.11, 3.12, and 3.13, checks that the `four-b`
  dependencies import, and runs the unit suite with the `four-b` extra on
  macOS arm64.
- 4B inference on Linux, Windows, and CUDA has not been measured. On the
  macOS host above, `cpu` with `float32` took 19.5 to 50.2 s per warm
  decision while other workloads loaded the host (see the README's hardware
  table).
- `cua-s1-nano-0.1` and `cua-s1-forms` load with the package's safetensors
  loaders, but no chooser or Driver integration runs them, and they were not
  part of the fixture runs.
- Peak memory for 4B inference has not been measured. The base weights alone
  are 9.32 GB, so 8 GB hosts and guests cannot load the model.

**Known failure modes.**

- `cua-s1-4b-0.1` abstained on the positive fixture in both modalities and
  chose the wrong target on the 8-candidate request.
- `cua-s1-4b-0.2` can select an action whose stated condition does not match
  the observation, as in the desktop counterexample above.
- On Apple silicon with Transformers 5.17.0 and torch 2.14.0, `mps` with
  `float16` crashed or hung during weight loading with Transformers'
  concurrent loader (#4198). `cua_s1.four_b` now loads `mps` weights
  sequentially; the fixture runs above used sequential loading.
- The one-letter-per-option readout supports at most 26 options. The
  chooser returns `option_limit` instead of truncating.
- The chooser's `model` field names the adapter, its revision when it can be
  read locally, and the modality (#4204), but not the base model. Evidence
  logs must record the base revision separately.

## Limitations (all checkpoints)

Computer-use behavior can fail because of unfamiliar layouts, changed
interface state, ambiguous labels, localization, timing, occlusion,
accessibility settings, visual similarity, or unexpected dialogs. A
specialist checkpoint may overfit its evaluation distribution and may not
recognize when a task has moved outside that distribution. A model may
select the wrong target, enter incorrect information, expose sensitive
data, repeat an action, or report success without satisfying the intended
outcome. Interface content can also contain adversarial or misleading
instructions. No claim of robustness, broad transfer, autonomy, or general
capability is made here.

The included runtime refuses checkbox mutations when the role or checked
state is unknown, skips a checkbox that is already checked, and verifies
the checked postcondition. A future runtime or checkpoint integration must
preserve an equivalent fail-closed boundary rather than assume actions are
idempotent.

## Evaluation methodology notes

No model result is claimed by this component beyond what is published in
`libs/cua-bench-s1/README.md` and the runtime smokes recorded in
[Verification scope and known failure modes](#verification-scope-and-known-failure-modes). Offline evaluation utilities
report abstention, coverage, selective accuracy, wrong actions, wrong
targets, and unsafe actions. Synthetic splits are disjoint by form
signature, and model selection uses validation rather than test results. A
checkpoint release must report: the exact checkpoint and code revisions;
the task set, environment, applications, and OS configuration; the action
space, observation method, stopping rules, and retry policy; success
criteria and independent outcome verification; aggregate results together
with representative failure categories; and known exclusions and material
differences from real-world deployment.

## Deployment guidance

Use an isolated environment, least-privilege credentials, bounded actions, and
auditable logs that do not retain secrets unnecessarily. Validate state before
actions and verify outcomes afterward. Require a human confirmation gate for
consequential or irreversible actions, and provide a reliable way to stop the
system.

See [`SECURITY.md`](SECURITY.md) for threat-model and reporting guidance.

## Data, architecture, and licensing

This component does not grant rights to checkpoint weights, external
training data, or unlisted third-party materials. At the pinned revisions,
the Hugging Face repositories declare these licenses: `cua-s1-4b-0.2`
Apache-2.0 (adapter only), `cua-s1-forms` MIT, and `Qwen/Qwen3.5-4B`
Apache-2.0 under Qwen's own terms. `cua-s1-4b-0.1` and `cua-s1-nano-0.1`
declare no license. A checkpoint release must
document its exact artifact license, data provenance, and applicable
third-party notices before distribution or use decisions are made. The source
code is MIT-licensed, but an official checkpoint may use separate terms that
permit research and evaluation while requiring a commercial license for
production, hosted inference, resale, or commercial redistribution.
