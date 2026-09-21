# Cua-S1 model card

## Model family

Cua-S1 is a research family of small, specialist computer-use models. Each
checkpoint is expected to have a narrow task contract and checkpoint-specific
evaluation evidence. Membership in the family does not imply general
computer-use capability. The family currently has three checkpoints,
documented separately below: `cua-s1-form-v0` (a finetuned, text-only
variant of `cua-s1-nano-0.1` specialized for form-oriented tasks),
`cua-s1-nano-0.1`, and `cua-s1-4b-0.1`.

## Initial checkpoint

**Name:** `cua-s1-form-v0`

**Relationship to `cua-s1-nano-0.1`:** `cua-s1-form-v0` is a finetuned,
text-only variant of the `cua-s1-nano-0.1` option-attention architecture
(see below), specialized to form-oriented user-interface tasks rather than
the general closed-option GUI decision scope `cua-s1-nano-0.1` targets.

**Intended scope:** research on form-oriented user-interface tasks in the
environments and task distributions documented with the checkpoint release.

**Status:** research profile. This component includes source code but does not
distribute weights, training datasets, or a checkpoint artifact manifest.

## Model design

The reference `tinyx` configuration uses a byte-level transformer encoder with
an option-attention classification head. Its size depends on the checked-in
configuration used for a training run.
For each interface element, the model selects one option from a fixed set:

- fill the element with one of the entities extracted from the source document;
- check the element;
- click the element; or
- skip the element.

The prototype scores interface elements independently and does not generate
field values. Its document parser only extracts values represented as
`Label: value` pairs. Plain code is responsible for turning selected options
into an execution order.

## Training data

The included generator creates fictional form episodes using reserved phone
numbers, `.invalid` domains, invalid test identifiers, and fictional brands.
It can co-locate similar field concepts and vary form titles to reduce simple
label or title shortcuts. Generated data is not a substitute for evaluation on
real interface variation.

No real user submission dataset is included. This model card does not grant or
establish distribution rights for a future checkpoint artifact.

## Intended uses

- Research on narrowly specified form-oriented computer-use tasks.
- Evaluation of specialist-model behavior in isolated, controlled environments.
- Study of task-specific failure modes, verification, and human oversight.

## Out-of-scope uses

- General-purpose or open-ended computer operation.
- Unsupervised operation on production accounts or sensitive data.
- Actions with financial, legal, medical, employment, safety, or other
  high-impact consequences.
- Bypassing access controls, consent, rate limits, or service policies.
- Treating model output or apparent task completion as proof that an action was
  correct or successful.

## Second checkpoint

**Name:** `cua-s1-4b-0.1`

**Base model:** this checkpoint is a LoRA (low-rank adapter) fine-tune on top
of the frozen, openly licensed [`Qwen/Qwen3.5-4B`](https://huggingface.co/Qwen)
checkpoint. It is not a model trained from scratch, and it is not the
`tinyx` architecture described above for `cua-s1-form-v0`. Only the LoRA
adapter weights are a Cua-S1 artifact; the base model's own weights and
license govern the base model itself and are not redistributed by this
component.

**Intended scope:** general computer-use decisions -- given a description of
the current screen state (an accessibility tree, or a screenshot) and a
fixed, closed set of candidate (element, action) options, select the single
best option. Both a text-only mode (accessibility tree) and a multimodal mode
(screenshot, using the base model's own vision-language input) are supported,
because `Qwen/Qwen3.5-4B` is natively vision-language.

**Status:** research profile. This component includes the inference-only
Python implementation (`cua_s1.four_b`). LoRA adapter weights are published at
[`cua-ai/cua-s1-4b-0.1`](https://huggingface.co/cua-ai/cua-s1-4b-0.1).

**Decoding contract:** the model is prompted with a chat-template message
asking it to answer with a single letter identifying the chosen option; each
option letter is verified to map to exactly one tokenizer token for the
loaded base model; one forward pass is run over the full prompt; and the
final-position logits at just the option-letter token positions are
softmaxed into per-option probabilities. This is a closed-set selection
readout, not open-ended text generation, and it does not produce free-form
output.

**Evaluation:** real, measured results for this checkpoint are published in
`libs/cua-bench-s1/README.md`'s Results section, not here (this file
describes the code/checkpoint's scope and limitations, not its benchmark
history, which changes independently of this document). As of that
package's current Results section: on a genuinely held-out cross-dataset
text split, task-level accuracy ranges 0.167-0.571 depending on family; on
`chess` (a real, Stockfish-backed move-selection task where every legal move
is its own scored option), 0.000 task accuracy and 0.409-0.515 element
(per-move) accuracy, on the 15-of-800 positions that fit this checkpoint's
26-option letter-decoding cap; on `safety_gate`, 0.071 zero-shot and 1.000
once finetuned on that family's own training split; on the external,
out-of-domain `general_decision` benchmark, 0.563 zero-shot. These are not general
computer-use capability figures -- see `libs/cua-bench-s1/docs/TASK_FAMILIES.md`
for exactly what each family measures before generalizing from any single
number. The tests included with this component exercise the
prompt-building, letter-assignment, and logit-decoding implementation, not
checkpoint quality.

**Out-of-scope uses:** the same out-of-scope uses listed above for
`cua-s1-form-v0` apply to `cua-s1-4b-0.1`. In addition, because this
checkpoint selects only among options the caller already enumerated, it must
not be treated as evidence that the enumerated option set itself was safe,
complete, or correctly scoped -- that scoping remains the caller's
responsibility.

## Limitations

Computer-use behavior can fail because of unfamiliar layouts, changed interface
state, ambiguous labels, localization, timing, occlusion, accessibility
settings, visual similarity, or unexpected dialogs. A specialist checkpoint
may also overfit its evaluation distribution and may not recognize when a task
has moved outside that distribution.

The model may select the wrong target, enter incorrect information, expose
sensitive data, repeat an action, or report success without satisfying the
intended outcome. Interface content can also contain adversarial or misleading
instructions. No claim of robustness, broad transfer, autonomy, or general
capability is made here.

## Evaluation

No model result is claimed by this source-only component. The included tests
exercise implementation behavior, not checkpoint quality. Offline evaluation
utilities report abstention, coverage, selective accuracy, wrong actions,
wrong targets, and unsafe actions. Synthetic splits are disjoint by form
signature, and model selection uses validation rather than test results.

A checkpoint release must report:

- the exact checkpoint and code revisions;
- the task set, environment, applications, and operating-system configuration;
- the action space, observation method, stopping rules, and retry policy;
- success criteria and independent outcome verification;
- aggregate results together with representative failure categories; and
- known exclusions and material differences from real-world deployment.

Comparisons are meaningful only when task definitions, environments, scoring
methods, and model-selection procedures are compatible.

## Observed failure modes

Expected failures include window-title changes and similar concepts such as
email versus street address or state versus an organization name. Synthetic
title variation and hard-negative concepts do not cover every ambiguity.

The executor also depends on the interface accessibility state. If a control's
current state is unavailable, an apparently valid action might be unsafe. The
included runtime refuses checkbox mutations when the role or checked state is
unknown, skips a checkbox that is already checked, and verifies the checked
postcondition. A future runtime or checkpoint integration must preserve an
equivalent fail-closed boundary rather than assume that actions are idempotent.

## Second checkpoint: cua-s1-nano-0.1

**Name:** `cua-s1-nano-0.1`

**Intended scope:** research on general closed-option computer-use GUI
decisions -- given a screen state and a fixed, closed set of candidate
(element, action) options, selecting which option each element should take --
in the environments and task distributions documented with the checkpoint
release. Unlike `cua-s1-form-v0`, this checkpoint is not specific to
form-filling.

**Status:** research profile. This component includes source code
(`cua_s1.nano`). Weights are published at
[`cua-ai/cua-s1-nano-0.1`](https://huggingface.co/cua-ai/cua-s1-nano-0.1).

### Model design

`cua-s1-nano-0.1` is a from-scratch, single-pass option-attention classifier
with approximately 855,000 trainable parameters. For a given screen state it
scores every candidate (element, action) option in one parallel forward pass
and selects the highest-scoring option per element, rather than generating
actions autoregressively.

Two context modalities are supported:

- a text modality, where context comes from a small trainable byte-level
  transformer over a rendered accessibility-tree excerpt per element; and
- a multimodal modality, where context comes from a frozen vision backbone
  (either SmolVLM-256M or SigLIP-base, selected explicitly, never
  auto-detected) over a screenshot crop per element, with only a small
  trainable projection layer on top of the frozen backbone.

Candidate options are always encoded as text by a shared option encoder.
`cua_s1.nano` includes inference and checkpoint-loading code only; it does
not include the training loop or data-generation code used to produce a
`cua-s1-nano-0.1` checkpoint artifact.

### Training data

<!-- TODO: describe the exact training data mixture and its provenance once
a checkpoint release is prepared. -->

The checkpoint is intended to be evaluated on a benchmark spanning synthetic
GUI task families, real-world GUI task families derived from AndroidControl
and GUI-360, and out-of-domain generalization checks based on chess and a
ViZDoom-based game-control task family. No real user submission dataset is
included, and no dataset is distributed with this component.

### Intended uses

- Research on general closed-option computer-use GUI decision-making.
- Evaluation of a small, from-scratch specialist architecture against other
  models in comparable size classes.
- Study of task-family generalization, including out-of-domain checks.

### Out-of-scope uses

- General-purpose or open-ended computer operation.
- Unsupervised operation on production accounts or sensitive data.
- Actions with financial, legal, medical, employment, safety, or other
  high-impact consequences.
- Bypassing access controls, consent, rate limits, or service policies.
- Treating model output as proof that a selected action was correct, safe, or
  successful.

### Limitations

The option-attention design requires a closed, pre-enumerated set of
candidate options per element; it cannot propose an action outside that set,
and the quality of its output depends on how well that option set was
constructed for a given task. Its very small parameter count trades some
capacity for speed, and it may not capture context that a larger
general-purpose model would. Vision-modality behavior additionally depends on
the frozen backbone's own limitations and on screenshot-crop quality.

Game-control and board-game task families are included as out-of-domain
generalization checks, not as evidence of any real-world game-playing
capability; results on these families do not establish general reasoning or
planning ability.

### Evaluation

Real, measured results for this checkpoint are published in
`libs/cua-bench-s1/README.md`'s Results section, not here. As of that
package's current Results section: on a genuinely held-out cross-dataset
text split, task-level accuracy ranges 0.000-0.286 depending on family; on
`chess` (a real, Stockfish-backed move-selection task, 800 positions), 0.000
task accuracy and 0.035 element (per-move) accuracy; on `safety_gate`, 0.000
zero-shot and 1.000 once finetuned on that family's own training split. This
checkpoint has no text-modality shape for the external `general_decision`
benchmark (its adapter expects real GUI elements/frames). See
`libs/cua-bench-s1/docs/TASK_FAMILIES.md` for exactly what each family
measures before generalizing from any single number.

Qualitatively, this architecture is intended to be fast (small-millisecond
per-task latency on GPU, and still well under 100ms on CPU) because scoring
is a single parallel forward pass over the option set rather than an
autoregressive generation. Any performance claim beyond that should point to
a benchmark report rather than this model card.

## Deployment guidance

Use an isolated environment, least-privilege credentials, bounded actions, and
auditable logs that do not retain secrets unnecessarily. Validate state before
actions and verify outcomes afterward. Require a human confirmation gate for
consequential or irreversible actions, and provide a reliable way to stop the
system.

See [`SECURITY.md`](SECURITY.md) for threat-model and reporting guidance.

## Data, architecture, and licensing

This component does not grant rights to future checkpoint weights, external
training data, or unlisted third-party materials. A checkpoint release must
document its exact artifact license, data provenance, and applicable
third-party notices before distribution or use decisions are made. The source
code is MIT-licensed, but an official checkpoint may use separate terms that
permit research and evaluation while requiring a commercial license for
production, hosted inference, resale, or commercial redistribution.
