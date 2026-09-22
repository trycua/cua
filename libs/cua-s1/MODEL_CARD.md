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

**Status:** research profile. Source code only; no weights, training
datasets, or checkpoint artifact manifest distributed.

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
[`cua-ai/cua-s1-4b-0.1`](https://huggingface.co/cua-ai/cua-s1-4b-0.1).

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
[`cua-ai/cua-s1-nano-0.1`](https://huggingface.co/cua-ai/cua-s1-nano-0.1).

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
[`cua-ai/cua-s1-4b-0.2`](https://huggingface.co/cua-ai/cua-s1-4b-0.2).

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

No model result is claimed by this source-only component beyond what is
published in `libs/cua-bench-s1/README.md`. Offline evaluation utilities
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

This component does not grant rights to future checkpoint weights, external
training data, or unlisted third-party materials. A checkpoint release must
document its exact artifact license, data provenance, and applicable
third-party notices before distribution or use decisions are made. The source
code is MIT-licensed, but an official checkpoint may use separate terms that
permit research and evaluation while requiring a commercial license for
production, hosted inference, resale, or commercial redistribution.
