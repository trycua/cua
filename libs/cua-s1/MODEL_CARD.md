# Cua-S1 model card

## Model family

Cua-S1 is a research family of small, specialist computer-use models. Each
checkpoint is expected to have a narrow task contract and checkpoint-specific
evaluation evidence. Membership in the family does not imply general
computer-use capability.

## Initial checkpoint

**Name:** `cua-s1-form-v0`

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
