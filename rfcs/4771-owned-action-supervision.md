---
title: Separate action supervision from application commitment
authors:
  - muness
created: 2026-10-06
last_updated: 2026-10-06
status: review
discussion: https://github.com/trycua/cua/issues/4771
rfc_pr: https://github.com/trycua/cua/pull/4772
implementation: []
supersedes:
superseded_by:
---

# RFC: Separate action supervision from application commitment

## Summary

Add an opt-in receipt lifecycle for exact-bound native text writes. Driver returns after dispatch and immediate readback, then independently owns the existing post-action supervision window. A qualified application adapter can establish value commitment through a bound transaction outcome and fresh observation while supervision is still pending. These remain separate states: committed values do not imply that supervision has finished. Existing synchronous actions retain their behavior.

## Motivation

Current macOS action invocation includes the roughly one-second window-change wait. The tool registry holds the global desktop-action coordinator through invocation, so independent text writes accumulate waits even when callers launch them concurrently. Moving observation into an independently owned task can release input locks after actual input and focus handling without shortening protection.

A private native experiment on Cua main `5227ad637590a15976413b1a33f8693fac0e9a7e` with the broad AX batching patch measured the following two-field task. Actual Cua backend was 0.34.0; Arc source was `74ffae1108b1cb4b1f6b161084af12646f544ba5`, actual server 0.1.1. Candidates were checked before each scored trial.

| Path | Median | Meaning |
|---|---:|---|
| Original serial observation waits | 2,639 ms | Full task decision |
| Independently owned, overlapping full waits | 1,519 ms | Full supervision fence plus final value read |
| Combined application evidence and supervision | 1,573 ms | Full fence plus application outcome proof |
| Combined in final alternating Arc comparison | 443 ms | Application commit and fresh AX proof; supervision still owned |
| Same combined comparison | 1,492 ms | Full supervision fence finishes |
| Arc in that comparison | 502 ms | Same application acknowledgment and independent value criterion |

The final comparison has four scored alternating pairs after warmup; all ten rows passed values, owned foreground and interference checks. The earlier four-arm matrix has three scored repetitions per arm after warmup. These small samples on one owned AppKit fixture establish feasibility, not general application or agent performance. No model or JEV calls were involved.

The [research packet](https://github.com/open-horizon-labs/computer-use/pull/110) records raw results, exact identities, the private patch and failed controls. It is not production implementation. The owned-sentinel foreground restoration helper is not qualified for ordinary user windows.

## Goals

- Remove repeated blocking observation waits from qualified text plans while preserving supervision ownership.
- Distinguish dispatch, application commitment, supervision completion and uncertainty in public results.
- Preserve fresh exact targets, record identity, dependent-input barriers and independent verification.
- Keep admitted input supervised after caller cancellation or transport disconnect.
- Preserve existing synchronous behavior and provide an explicit unsupported result on unqualified platforms or routes.

## Non-goals

- Shortening the generic protection window, changing defaults or making ordinary AX readback a transaction acknowledgment.
- Treating a quiet window sample as proof that no later application timer exists.
- General action programs, planner policy inside Driver, automatic mutation retries or rollback.
- Promoting the private research registry or sentinel restoration helper directly into production.

## Terminology

**Dispatched:** the exact-bound input was attempted and its delivery result recorded. Immediate echo may be included as evidence; it is not a committed outcome.

**Committed:** a qualified application provider reports a final transaction outcome, bound to the intended operation and context, and a fresh independent observation agrees. This claim covers the provider's declared value/transaction scope, not every future desktop effect.

**Supervision:** independently owned post-dispatch observation and protection, with explicit pending, finished, failed or interrupted state. A finished empty poll does not prove application commitment.

**Fence:** a bounded wait for a named set of receipt obligations. It returns their individual states; one successful later action cannot conceal an earlier failure.

## Current state

The global coordinator lives in `libs/cua-driver/rust/crates/cua-driver-core/src/tool.rs` and currently spans tool invocation. macOS `tools/set_value.rs` also holds a process mutation lease and awaits `Snapshot::detect_async()` from `window_change_detector.rs`. The snapshot owns the relevant protection lease. Releasing only the process mutation lease left the global coordinator bottleneck in the private pilot.

Related work: #3963 defines script-speed experiments; #4055 proposes narrowed reads and explicit waits; #4744 overlaps caller-side advisory reads with an unchanged action; #4751 develops explicit text plans; #4762 batches AX observations. This proposal changes Driver's ownership and public completion lifecycle, which those experiments do not by themselves authorize.

## Proposal

### Public surface

Start with an additive exact-bound `dispatch_set_value` operation, alongside the existing synchronous setter. It accepts the same fresh native target binding and returns an opaque receipt scoped to the current owner/session. A public receipt read operation returns its current disposition and supervision state. A fence takes explicit receipt IDs and a deadline and returns each receipt's state. SDK, CLI and MCP representations must share the same semantics and compatibility fixtures before public release; naming and projection details require the maintainer decision below.

The initial supported route is qualified native macOS text-field AXValue writes. Popups, renderer-untrusted values, coordinate delivery and other action kinds refuse this route. Unsupported platforms or unqualified routes return `unsupported` before input. No fallback silently disables supervision or retries through another route.

The dispatch result includes the receipt ID, admitted binding identity, input disposition, independent supervision status and immediate readback evidence. It cannot report `confirmed` merely because AX echoed the requested value. Application outcome is a separate field owned by the qualified evidence layer. Raw supplied text must not be included in persistent receipts by default.

### Admission and dispatch

Reserve a receipt and supervision capacity before mutation. Revalidate exact owner/process lifetime, window and target binding through the existing checks; reject invalid, expired or mismatched context before input. Serialize actual dispatch and focus handling through the existing global and process coordinators. Release those coordinators only after the mutation thread has ended and focus handling has completed. Cancellation must not drop input ownership while the blocking OS operation remains alive.

Transfer the snapshot and its protection lease to the durable receipt owner before returning. Failure during this transfer leaves the action uncertain and supervised through a synchronous fallback; it must not claim a successful unsupervised dispatch. Capacity exhaustion refuses before input. Admission does not assert an independent input lane or bypass a coordinator.

### Receipt owner and lifetime

The backend runtime owns receipts and observer tasks, independently of request futures. It retains every admitted receipt until its watcher terminates and the configured terminal-retention period expires. Cancellation of a waiter cancels only that wait; it cannot remove the receipt or abort its watcher. A dropped transport triggers orderly drain during runtime shutdown. Session expiration blocks further input but does not revoke protection for already admitted input.

Observer failure or an interrupted shutdown records an explicit uncertain state. A process crash cannot promise continued in-process protection; reconnect must report the lost owner generation and require a fresh observation before any further input. There is no automatic replay. Receipt lookup for a foreign session, expired owner generation or unknown ID refuses without accessing an unrelated desktop target. Terminal eviction produces an explicit expired state.

### Application evidence and dependent inputs

Driver owns deterministic dispatch and desktop supervision. A provider-qualified application adapter owns transaction evidence and the recipe/SDK executor owns the plan. An adapter must define which immutable application outcome it trusts, how that outcome binds transaction, action/field, value, process, exact window, record and generation, and which fresh independent observation verifies it. Generic AX value echo, renderer events and silence are not substitutes.

The initial executor accepts explicit text plans. Each supplied step is rebound from a fresh observation. Dependent steps require the earlier operation's qualified committed outcome and unchanged context. Any earlier rejection, missing or misbound outcome, changed record or observation failure stops the plan with visible partial results. Independent fields may overlap pending supervision only when the executor's selected provider contract and fresh binding justify that ordering. No provider capability means the existing synchronous fallback remains available.

Commitment may be reported while supervision is `pending_owned`. A caller requiring fully supervised completion must fence. Late observation findings are delivered through the receipt owner and become barriers to further dependent input. The API must not rewrite a prior commitment claim into “nothing could happen later”; its value scope and the separate supervisor state remain explicit.

### Foreground and interference

Supervision must retain the existing protection duration and qualify actual activation callback delivery. Ordinary foreground restoration must use current exact foreground identity and avoid overriding a legitimate user's foreground change. User/other-agent interference invalidates the affected automation run and stops new input; restoring a stale prior window is not permitted. The private known-sentinel helper is evidence for the owned test surface only. This production requirement must pass before the asynchronous route is enabled.

### Platform behavior

macOS is the first candidate, gated on real native qualification. Windows and Linux retain synchronous behavior and explicitly refuse the asynchronous route until they supply equivalent ownership, exact binding, cancellation and interference evidence. Do not project an empty window report as parity. No platform automatically substitutes the user's desktop for an unavailable isolated surface.

## Alternatives considered

- **Shorten or skip the detector:** lowers latency, but acknowledgment-only execution failed the delayed-activation control. Not selected as the generic route.
- **Return complete on matching AX echo:** falsely accepted a value rejected after 1.4 seconds and a missing application acknowledgment. Rejected.
- **Just launch concurrent calls:** left the global coordinator held through invocation; the native pilot stayed near serial latency. Insufficient.
- **Release both input locks early:** risks interleaving actual input or restoration. Reject; release only after those phases finish.
- **Always fence before reporting value commitment:** safe conservative option, preserved for synchronous callers; retains about a second of residual latency and cannot expose the measured prompt-commit benefit.
- **Require application evidence for every app:** unavailable for ordinary applications. Keep explicit provider qualification and synchronous fallback.
- **Use a private experiment-only task vector as the public owner:** insufficient cancellation, retention and cross-session semantics. Build the durable receipt owner instead.

## Compatibility and migration

All existing synchronous operations and their result schemas remain unchanged. The new operation is opt-in and capability-advertised only after qualification. Old clients never receive receipt-only results from existing setters. Initial release is experimental and disabled by default; no default performance claim follows from this fixture. Rollback disables admission to the new operation while draining already admitted receipts, then routes new plans through existing synchronous calls. Do not terminate protection to perform rollback.

## Security, privacy, and telemetry

Existing OS permissions and target/session authorization remain authoritative. Opaque receipt IDs confer no cross-session authority. The operation cannot broaden the native target, privilege, display or permission boundary. Apply bounded admission, receipt storage and terminal retention to prevent unbounded observation tasks. Persist only lifecycle/disposition and safe binding identifiers by default; no secrets, raw text values, screenshots, user app inventories or private transaction payloads. Opt-in diagnostics need explicit redaction and retention. The public research uses synthetic fields and owned applications.

## Implementation plan

1. After the maintainer decision, implement the owner/state machine and deterministic cancellation, admission, expiry, drain and fault-injection checks in core. No public asynchronous mutation enabled yet.
2. Add the exact-bound native text dispatch operation and transfer the unchanged observer/protection lease. Qualify main-loop callback delivery, ordinary foreground restoration and interference without the research helper.
3. Add SDK/CLI/MCP receipt read and fence projections with shared compatibility fixtures, explicit unsupported-platform behavior and unchanged synchronous parity.
4. Add a provider-qualified text-plan example that owns application outcomes, fresh binding, dependent barriers and independent proof. Keep unavailable providers fail-closed.
5. Run current-candidate alternating native trials and the supported desktop matrix before enabling an experimental release. Link each implementation PR and its local evidence here; do not mark this RFC completed from research timings.

## Test and acceptance plan

- Delayed rejection before and after the observation horizon; missing, forged/misbound and generation-mismatched outcomes must never produce committed success.
- First step rejected with the second still in the plan: exactly one write. Second step rejected: partial results preserved, no rollback/retry claim. Earlier failure cannot be concealed by later success.
- Admission exhaustion and invalid session/target/record refuse before input; no coordinator bypass or overlapping OS mutation threads.
- Cancellation before admission, during native dispatch, during a fence and after commitment; the owner survives wait cancellation, protection survives admitted dispatch, and states remain retrievable.
- EOF, session closure, orderly shutdown, observer faults, terminal eviction and simulated lost owner generation; uncertainty is explicit, reconnect never replays input.
- Actual delayed activation, user foreground changes and competing-agent activity; do not infer protection from registration or callbacks alone. Observe OS and owned app state independently.
- Shared SDK/CLI/MCP fixtures, synchronous result parity and explicit refusal on unsupported routes/platforms.
- Latest release/source preflight for every head-to-head, exact running backend identity, alternating trials, successful task/commit/fence durations reported separately, warmups excluded, and every failed/interfered run retained outside scored medians.
- Mutation controls must fail for trusting echo, dropping observer ownership, skipping dependency fences, cancelling observers with waiters and concealing partial failure.
- Run all feasible local native checks and the repository's supported desktop E2E requirements. Any unavailable canonical environment is a concrete release qualification gap, not an implicit waiver.

## Unresolved questions

- Accept the owner/receipt contract and the separation between committed value and completed supervision?
- Should the first public operation be a dedicated text dispatch operation or an explicitly opted-in variant of the existing setter? A dedicated operation is proposed to preserve old result semantics.
- Which runtime owns retention across SDK, CLI and MCP sessions, and what capacity/retention limits should be normative?
- Which application outcome provider can be maintained and qualified as the first real adapter?
- What foreground-restoration/interference evidence is sufficient to enable the asynchronous macOS route beyond owned fixtures?

## Decision record

Awaiting maintainer decision in #4771. Research does not authorize production implementation or change the status to accepted.
