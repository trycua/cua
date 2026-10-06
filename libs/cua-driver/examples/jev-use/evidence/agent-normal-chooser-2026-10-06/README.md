# Local qualification against the normal Cua chooser

The 20 scored local trials all completed with independent fixture proof and zero false completion. This qualifies the narrow text-plan recipe's behavior, but does **not establish a consistent stacked speedup**. The existing chooser-only baseline was fastest. With a fresh frontier plan in both arms, removing JEV decisions saved actual requests and tokens but won only two of five wall-time pairs. Adding the experimental action-plus-read operation did not consistently improve the literal executor.

## Actual end-to-end results

Five alternating repetitions used four arms after a separate successful four-arm smoke. Every trial launched a new owned AppKit fixture and set Full name first, then Email, leaving Subscribe unchecked, Record A selected and submission count zero. Each model plan was generated anew; no plan was reused across arms. Task wall time includes initial observation, planning, decisions, input and independent verification, while fixture startup/readiness is excluded equally.

| Arm | Strict passes | Median task wall | Median outer MCP wall | CLI turns per trial | JEV decisions per trial | Visible MCP calls | Canonical dispatches |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Normal chooser, caller-authored NativeTask | 5/5 | 3.848 s | 3.550 s | 0 | 2 | 7 | 7 |
| Fresh frontier plan + normal chooser | 5/5 | 8.613 s | 3.550 s | 1 | 2 | 7 | 7 |
| Fresh frontier plan + literal executor | 5/5 | 8.551 s | 3.503 s | 1 | 0 | 7 | 7 |
| Fresh frontier plan + literal executor + composite | 5/5 | 8.224 s | 3.652 s | 1 | 0 | 7 | 9 |

Strict success requires `terminal == verified_complete`, the runner's pass flag, its final oracle flag, and a separately recomputed fixture oracle from the saved application state. All 20 met every condition; all 20 independently satisfied the desired fields and unchanged checkbox, record, dialog and submission state. The successful smoke's four rows are excluded from these statistics.

- Literal versus matched frontier-plus-normal: lower wall time in **2/5** pairs; median paired difference **+0.518 s**. Its ratio of group medians was only 0.7% lower, which does not establish a consistent improvement. It removes two real JEV requests per trial; the matched normal arm's median JEV time was 0.350 s, while fresh frontier planning and caching varied.
- Composite versus matched frontier-plus-normal: lower wall time in **3/5** pairs; median paired difference **−0.389 s**. Its group median was **4.5% lower**. Five pairs with mixed results support only this observation, not a reliable population speedup.
- Composite versus literal: lower wall time in **2/5** pairs; median paired difference **+0.003 s**. No consistent marginal benefit from stacking the composite operation was shown.
- Literal versus chooser-only: slower in **5/5** pairs; median paired difference **+4.596 s**. Adding a new frontier planning call to an existing declarative NativeTask is not an optimization.

Normal Cua does not inherently require a Codex planning call. The chooser-only arm is its existing caller-authored task baseline; the matched frontend arms address the narrower case where an ordinary agent supplies intent once. Every normal request used Cua's validated `cua.jev_choice_request_v2`, `DecisionRequest`, `TypeSafeDecisionModel` and TypeSafe SDK path, with all field/parameter alternatives plus reobserve/abstain: six candidates initially and five after the first setter. They were not singleton Arc-policy requests. All normal decisions were followed by the same exact caller-intent and fresh semantic-binding guard used by the literal path. Reobserve, abstain or a different authorized pairing stops this narrow recipe; this is not every recovery path of the general native runner.

## Actual model usage

The table reports totals across each arm's five scored trials. CLI input includes cached input; the cache column is its reported subset, not additional tokens. These are actual CLI usage receipts, not inferred frontier costs. Codex reports the requested `gpt-6-sol` identity but does not expose the resolved server model version. JEV returned `jev-1.13.0` throughout; all 20 SDK calls made one HTTP request each and returned HTTP 200.

| Arm | CLI completed turns | CLI input | CLI cached input | CLI output | JEV calls | JEV input | JEV output |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Chooser-only | 0 | 0 | 0 | 0 | 10 | 10,443 | 960 |
| Frontier + normal | 5 | 86,920 | 31,744 | 265 | 10 | 10,440 | 960 |
| Frontier + literal | 5 | 86,920 | 47,616 | 245 | 0 | 0 | 0 |
| Frontier + literal + composite | 5 | 86,920 | 31,744 | 245 | 0 | 0 | 0 |

The normal frontier arm reported 18 reasoning output tokens, included in its reported output count; the other scored frontier arms reported zero. The runner counts completed CLI turns, SDK decisions and HTTP requests separately. CLI internal provider HTTP requests are not exposed, and no such count or monetary saving is invented. Frontier cache differences and a slow literal planning invocation affect the small-sample task medians.

## Binding, composition and limitations

`validate_literal_text_plan` rejects extra fields, changed owners, altered literals and missing, duplicate or reordered steps before execution. Initial controls must already be uniquely eligible; each input still resolves its label twice from fresh observations, checks unchanged semantic/ancestry identity and the caller's same-record context, and consumes only the freshly bound token and exact literal. Independent application checks verify every step and the whole task. No uncertain input is retried. No checkbox, ambiguity, record-selection or submission capability is added.

The composite research adapter connects through PR 4745's opt-in SDK host to the same signed released daemon. It preserves both preinput observations, validates both child results plus the fresh same-owner postaction observation, and never reuses the postread as a binding. An error or refusal after input stops without replay. This uncached composition adds two canonical read dispatches; it reduces neither the seven measured outer transport calls nor the literal plan's one frontier turn. Six adversarial adapter checks pass. These outer calls belong to the runner's MCP transport; they are not seven LLM-visible tool calls made by the planning-only CLI invocation.

Current source/release/runtime preflight completed at **2026-10-06 15:18:53 UTC**: Cua upstream `aa7a31a5f3633249db69f766256b1749feb7a652`; installed/latest/running signed Driver **0.34.0**, release source `b0968e1b12834e485dda68789541a3cc57664a9f`; Arc fixture source `74ffae1108b1cb4b1f6b161084af12646f544ba5`, version 0.1.1; macOS 27.0.1; Python 3.12.12; Codex CLI 0.160.1. Actual SDK metadata confirms daemon PID 27624, embedded=false, contract 0.8.0; its returned host bundle ID is null and remains null. The experimental host's exact binary SHA and all check timestamps are retained. Arc provides only the owned fixture here, not the decision policy or runtime recipe.

The measured source commit was `d95fa463446750f0dbff122c4c2b1e67af3913cd`. `measured-run_agent.py` preserves that exact executed runner, and its hash matches the trace. Raw composite canonical counters remain null because that runner expected nested operation telemetry while this host returned `child_dispatches` at the response root. Summaries derive the nine dispatches from the retained exact lists rather than wrapper acknowledgement. The current reproducer fixes this count-only path and makes its pass assignment explicitly require `verified_complete`; inputs, model requests, clocks and raw outcomes were not rewritten. The independent analysis requires the stronger terminal-and-oracle conjunction regardless of that implementation detail.

Earlier refusals are retained and excluded: three locked-session smoke rows stopped before models/input, then four unlocked readiness rows stopped before models/input because a tree-only readiness probe omitted frame metadata required by the normal eligibility rules. The readiness probe was corrected to request the same supported screenshot/frame metadata as timed observations, without relaxing eligibility. The subsequent four-arm smoke and 20 scored tasks passed. All owned fixtures and client processes closed; no persistent daemon settings, permissions or user apps were changed.

## Reproduction and evidence

Lossless gzip traces and `manifest.json` retain uncompressed lengths/hashes and compressed hashes. `results-summary.json` contains every derived row and paired comparison; `analyze.py` recomputes it directly from the unchanged traces without desktop or provider access. Independent review recomputed all 20 strict outcomes, medians, paired results and child counts.

For new live runs use the editable Arc fixture environment, this Cua checkout, an OH checkout containing the shared fixture harness, the experimental SDK host from PR 4745, and a runtime credential command (JSON argv) or `TYPESAFE_API_KEY`. External configuration names are `OH_COMPARISON_SOURCE`, `ARC_EVAL_SOURCE`, `CUA_LITERAL_SOURCE`, `ACTION_OBSERVE_HOST` and `JEV_CREDENTIAL_COMMAND`. The runner checks the unlocked session, latest sources/releases and actual daemon metadata before fixtures or input; it discovers its own exact actionable window through supported `list_windows` and fresh observations. Set `SMOKE_ONLY=1` for the separate smoke, then omit it for five alternating quadruples. Credentials are never saved. Run `analyze.py` after measurement. Run the six adapter tests with the recipe's Python directory on `PYTHONPATH`.

The complete jev-use Python suite passed 242 tests with two existing skips; the focused executor/plan suite passed 13 and the composite suite passed six. This is a macOS two-field caller-authorized qualification, not general planning accuracy, cross-platform live evidence or a Driver-core performance improvement. The historical singleton-control result remains separate evidence and does not replace this normal-chooser comparison.
