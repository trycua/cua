# Qualification against the normal Cua chooser

This follow-up replaces the historical singleton Arc-policy control with Cua's normal `cua.jev_choice_request_v2` request and TypeSafe SDK decision model. It keeps every eligible text-field/parameter pairing and the normal reobserve/abstain alternatives; it does not trim the request to the desired action. Both paths retain the same fresh binding, caller intent guard and independent application oracles.

Three arms distinguish the cost of the frontend from the cost of redundant per-step decisions:

- `chooser_only`: the caller supplies the declarative NativeTask; the normal JEV chooser selects each next action.
- `frontier_normal`: a fresh Codex `gpt-6-sol` invocation transcribes the authorized task into fields and values, then the normal JEV chooser selects each next action.
- `frontier_literal`: a fresh Codex invocation supplies the plan once, then the guarded literal executor runs it without further model requests.

The matched frontend arms do not imply that normal Cua requires a Codex planning invocation. The chooser-only arm is the existing declarative-task baseline. All arms use an explicitly ordered two-field task: set Full name first, then Email, leave Subscribe unchecked and Record A selected, and do not submit. The chooser's selected complete action must match the authorized step; other alternatives cause handoff. This comparison exercises the normal chooser schema within the recipe's narrow guarded text executor, rather than reproducing every recovery path of the general native runner. No checkbox, ambiguous record selection or submission is added.

The plan validator rejects unknown fields, altered owners, missing/duplicate steps and any changed label or literal before execution. Labels must already be uniquely eligible in an initial fresh fixture observation and are resolved again before each input. The independent fixture oracle verifies the desired fields and the unchanged record, checkbox and submission state.

The requested frontend model is recorded, but Codex CLI does not expose its resolved server model identity. Actual CLI turns and token usage are retained. JEV SDK calls, HTTP requests, server model identity and available input/output token usage are recorded separately; absent provider usage remains null. Task wall time includes fresh planning, observations, decisions, input and independent verification, and excludes fixture startup/readiness equally across arms. No cached plan is reused across arms. The scored protocol is five alternating triples after a separate smoke, with failures retained and excluded from successful-task speed claims.

## Current live qualification status

The initial three-arm smoke stopped before model calls or input: all three owned fixtures returned an empty AX tree and capture failure. A read-only session check then confirmed the desktop was locked with no active displays. These are retained refusal outcomes, not scored task timings. The updated runner now refuses a locked-session preflight and discovers its own actionable window through supported `list_windows`, exact title/on-screen matching and fresh nonempty text controls. It does not unlock the desktop or manipulate display state. Live qualification is pending an available unlocked session; PR 4751 remains draft until that evidence and independent review are complete.

Run with the editable Arc fixture environment and this Cua checkout, an OH checkout containing the shared fixture harness, and a runtime credential command (JSON argv) or TYPESAFE_API_KEY. Credentials are never included in saved evidence. Use `SMOKE_ONLY=1` for the smoke; omit it for five alternating triples. Example configuration names are `OH_COMPARISON_SOURCE`, `ARC_EVAL_SOURCE`, `CUA_LITERAL_SOURCE`, and `JEV_CREDENTIAL_COMMAND`.
