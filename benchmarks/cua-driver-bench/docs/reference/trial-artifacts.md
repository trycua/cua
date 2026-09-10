# Trial directory reference

A runtime trial directory contains frozen execution inputs, lifecycle evidence,
collected target state, evaluation output, receipts, and the final result.

## Top-level files

| Path | Contents |
| --- | --- |
| `config.json` | Resolved runtime inputs and adapter configuration |
| `inputs.manifest.json` | Digest inventory for captured inputs |
| `events.ndjson` | Canonical hash-chained lifecycle events |
| `result.json` | Final lifecycle, outcome, participation, certification, policy, and cleanup state |

## Input directories

| Path | Contents |
| --- | --- |
| `inputs/` | Captured task, system, policy, agent, and apparatus inputs |
| `harness-workspace/` | Fresh per-attempt harness home and configuration |

The harness workspace is separate from the target application workspace.

## Output directories

| Path | Contents |
| --- | --- |
| `artifacts/` | Agent metadata, collection manifests, protected evidence, receipts, and bounded exports |
| `evaluator/` | Host evaluator result and process streams |

Protected production adapters may add signed participation and apparatus
receipts, a sealed mediator chain, provider-proxy evidence, stopped-disk
collection manifests, and a bounded target-state export.

An explicit protected debug run also adds `artifacts/agent.debug.json`. This
content-free local diagnostic is verified by `cdb explain` but excluded from
signed benchmark evidence and trial export.

## Result fields

| Field | Meaning |
| --- | --- |
| `status` | Runtime lifecycle status |
| `evaluation` | Independent task outcome and score |
| `participation` | Ordered driver-participation receipt |
| `apparatus_certification` | Signed protected-boundary decision |
| `debug_mode` | Whether the trial used non-certifying protected diagnostics |
| `execution_policy` | Observed routing, accounting, autonomy, and policy decision |
| `certifying` | Composite certification result |
| `comparison_eligible` | Controlled-comparison eligibility |
| `cleanup_ok` | Cleanup completion |

## Integrity rules

`cdb explain` verifies the input-manifest binding, immutable config, event chain,
receipt bindings and signatures, evaluation digest, policy receipt, collection
manifests, and final decisions. A consumer must verify the directory before
using or exporting its result.

Debug-mode trials cannot be exported.

## Publication boundary

Agent-authored screenshots, logs, files, and self-reports cannot replace
evaluator or protected-observer evidence. Local observer directories may
contain screen content and are excluded from published result bundles.
