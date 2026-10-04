# About outcome, participation, certification, and comparison

Every protected trial produces four separate decisions because each answers a
different question.

## Outcome

Outcome asks whether the task reached the required final state. A task-owned
evaluator checks collected target state through a channel outside the agent's
control.

An outcome can pass even when the agent used an unexpected route. It can also
fail inside a sound execution apparatus.

## Driver participation

Participation asks whether the required computer-use interaction occurred.
The task declares semantic observation, action, and readback requirements. A
benchmark-owned observer binds those events to the target process, window,
surface, facts, and one trial.

Participation does not add points to the task score. It controls whether the
result can claim the required driver path.

## Apparatus certification

Certification asks whether the protected environment and evidence boundary
held. It covers clone state, account isolation, driver identity, network
enforcement, human input, fresh workspace, immutable inputs, stopped-disk
collection, cleanup, host evaluation, and signed receipts.

Successful participation cannot repair an apparatus failure. A sound
apparatus can still record a failed task outcome.

## Comparison eligibility

Comparison eligibility asks whether observed facts support the report's causal
claim. A model comparison needs trustworthy served-model and accounting facts.
A harness comparison needs identical observed model routes and tool contracts.

Opaque telemetry can leave a trial ineligible for comparison even when its
outcome and apparatus certification pass.

## Why the separation matters

Collapsing the decisions into one score would hide useful failures. A task
success with off-target driver activity is product evidence and a safety
finding. A provider authentication failure is an infrastructure condition and
contains no model-performance evidence. A missing cost count limits a cost
claim without changing the task result.

The runtime retains each decision and its reason so reports can state the
narrowest claim supported by the evidence.

## Further reading

- [Trial artifact reference](../reference/trial-artifacts.md)
- [Comparison view reference](../reference/comparison-views.md)
- [Public monorepo boundary decision](../decisions/0001-public-monorepo-boundary.md)
