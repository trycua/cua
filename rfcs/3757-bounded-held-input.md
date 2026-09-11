---
title: Bounded held desktop input
authors:
  - steipete
created: 2026-09-11
last_updated: 2026-09-11
status: review
discussion: https://github.com/trycua/cua/issues/3757
rfc_pr:
implementation: []
supersedes:
superseded_by:
---

# RFC: Bounded held desktop input

## Summary

Add one native `hold_inputs` operation that presses a bounded set of keys and
mouse buttons, holds them for a requested duration, and releases every acquired
input before successful completion. The native driver owns timing, cancellation,
and release across local SDK and MCP calls. Existing tap operations remain
unchanged. This RFC proposes the contract; implementation starts after a recorded
maintainer decision.

## Motivation

Applications that sample maintained input state can miss a short press/release
pair even when event delivery succeeds. A game may respond to menu taps but miss
movement or firing. An agent also needs overlapping inputs, such as moving while
firing, without coordinating independent long-lived down/up calls across model
or network round trips.

## Goals

- Deliver simultaneous, bounded keyboard and mouse holds through the normal
  authorization and target-resolution paths.
- Release acquired input after completion, cancellation, or partial failure.
- Report delivery and cleanup separately from observed application success.
- Expose the same contract through generated native SDKs and MCP.

## Non-goals

- Indefinite key-down/up APIs, macros, autoplay, or input recording/replay.
- Relative mouse motion or pointer-lock support; see [#3298](https://github.com/trycua/cua/issues/3298).
- Changing tap pacing, existing mouse-drag behavior, or background focus policy.
- Claiming that releasing synthetic input can recover from every native-process
  crash or operating-system failure.

## Terminology

An **acquired input** is a key or button whose down transition was emitted by
this operation. A **hold** is one native operation, not a lease retained between
calls. **Release confirmed** means the backend's release delivery/completion
barrier succeeded; it does not prove the application acted on the input.

## Current state

The native [PressKeyInput and HotkeyInput](../libs/cua-driver/rust/crates/cua-driver-contract/src/inputs.rs)
carry no duration. Platform implementations own short press/release sequences;
Windows foreground delivery can queue both transitions in one `SendInput` call.
Linux also has legacy background window `mouse_button_down`/`mouse_button_up`
tools, but these do not provide a bounded, cross-platform keyboard/mouse hold.

[PR #3489](https://github.com/trycua/cua/pull/3489) changes macOS process-level
keyboard pacing. A pacing override is not a per-operation duration or release
lifecycle. [RFC #2794](https://github.com/trycua/cua/issues/2794) proposes sequential
batches and visual observation, and excludes raw held-button operations. This
proposal supplies a separate, self-contained input primitive; it does not expand
that RFC's batch allowlist.

## Proposal

### Public operation

Add `hold_inputs` to the canonical generated contract and tool registry:

| Field         | Proposed contract                                                                                           |
| ------------- | ----------------------------------------------------------------------------------------------------------- |
| `target`      | Required existing desktop action target; window and element targets are unsupported initially.              |
| `keys`        | Optional array of 1–8 existing key names. Names are individual keys, including modifiers; no chord strings. |
| `buttons`     | Optional array containing any of `left`, `middle`, `right`, at most once each.                              |
| `duration_ms` | Required integer from 1 through 5000; reject values outside the range.                                      |
| `session`     | Existing optional lifecycle session label and authenticated owner semantics.                                |

At least one key or button is required. Resolve every key before emitting input;
reject unknown names and duplicate native keys, including aliases. Reject unknown
fields. Mouse buttons act at the current pointer position; there is no implicit
pointer move. Desktop authorization covers input directed by current focus and
pointer position, including changes during the hold. Window and element targets
and background delivery are rejected before side effects. A future window mode
requires a backend that confines delivery throughout the hold; an initial focus
or pointer check alone does not establish that contract.

Emit requested key downs in array order, then button downs. Start the requested
hold interval after the final successful down delivery barrier. Release in reverse
order. Acquisition has a separate 1000 ms deadline starting before the first
down attempt. From that attempt, the overall normal-operation deadline is
`1000 + duration_ms` ms; expiry starts cleanup even when acquisition has stalled.
Adapters must use bounded native calls, fence late down delivery after cancellation,
and retain a release path that does not depend on a stalled acquisition waiter.
Report acquisition time separately from the measured hold interval.
A hold emits no synthetic repeat-down loop; applications or the OS may
apply their normal repeat behavior. Scheduling can extend the measured interval;
`duration_ms` is not a real-time guarantee. Return actual elapsed hold time.

Use the existing action-result effect conventions. Delivery alone remains
`unverifiable`; interruption after input begins is a partial effect. Add bounded
structured fields for requested duration, measured duration, and release outcome.
A successful result requires release confirmation. A cleanup failure is a
structured error, never `ok:true`. Exact field names and refusal codes become part
of the accepted generated contract rather than transport-specific additions.

### Ownership and cancellation

Shared native code owns validation, deadlines, input acquisition bookkeeping, and
settlement. Platform adapters resolve and deliver native transitions. A client
must not implement this operation as down/sleep/up calls.

1. Resolve current owner authority, target identity, and backend capability before
   any down transition. Acquire an input-domain guard shared with other synthetic
   input operations on the same desktop/seat; concurrent callers cannot interleave
   conflicting input. Revalidate after waiting for that guard.
2. Reject a request when requested inputs are already down, or when the backend
   cannot establish that it can safely own and release them. Release only inputs
   acquired by this operation. The contract does not promise isolation from a
   person pressing the same key during a hold.
3. Record each acquired input immediately after its down is accepted. Keep native
   acquisition and release in one guarded owner, including partial acquisition
   failures. Never release a guessed full key set.
4. Use an interruptible native wait with a monotonic deadline. Cancellation,
   session termination, or loss of target validity starts release immediately;
   it does not transfer cleanup responsibility to the caller.
5. Async future cancellation alone is insufficient: Rust `spawn_blocking` work
   survives cancellation of its waiter. A blocking adapter must receive an
   explicit cancellation signal and retain its release guard until settled.
   Local SDK cancellation and remote MCP transport/session teardown must reach
   that same owner. Losing a client connection must not extend the hold deadline.
6. Hold the input-domain guard through release completion. If release cannot be
   confirmed, report the partial effect and cleanup failure, and block new input
   on that domain until a backend state check, compensating release, or seat/device
   reset proves reconciliation. Replacing the runtime alone does not clear this
   condition: preserve it across restart or fail closed until reconciliation is
   established. Do not silently retry the original hold.

A platform must preserve a release path even if the focused application closes
or focus moves. Do not change focus to perform cleanup. Revoking the
original action's authority prevents further downs but must still permit cleanup
of that action's already-acquired inputs.

### Platform support

Support is declared per backend and target mode, not inferred from OS name or
package version. SDK and MCP capability discovery must agree. Unsupported
combinations return a structured refusal before input starts.

| Backend                                   | Required implementation evidence                                                                                                                                                                                                                |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Linux X11 desktop                         | XTest downs flushed before timing; matching releases and server completion barrier; desktop input-state ownership.                                                                                                                              |
| Linux Wayland                             | Each supported compositor/backend must demonstrate device lifetime, state queries or equivalent safe ownership, cancellation, and matching release. Generic wtype availability does not establish support. Unproven backends explicitly refuse. |
| macOS desktop                             | Existing authorized CGEvent delivery with paired transitions, cleanup, and target checks; evidence from a signed, TCC-authorized interactive process.                                                                                           |
| Windows desktop                           | Separate `SendInput` down/up batches around the native wait, partial insertion cleanup, desktop input-state verification, and an active interactive session.                                                                                    |
| Any window, element, or background target | Explicitly unsupported in this first proposal.                                                                                                                                                                                                  |

If reliable state inspection or release is unavailable on a backend, shipping an
explicit refusal is preferable to presenting taps as holds. The initial supported
matrix must be recorded in the implementation PR and generated capabilities.

## Alternatives considered

**Add duration to `press_key` and `click`.** Smaller individually, but overlapping
keys and buttons would still require concurrent calls or additional stateful APIs.
One bounded set gives release ownership a single transaction boundary.

**Expose raw down/up calls.** Flexible, but a missed follow-up, lost client, or
model delay leaves input held. Existing legacy mouse tools need not be expanded
to implement this contract.

**Repeat taps or change global pacing.** Repeated taps are not maintained input
state; a process-wide gap also changes unrelated typing and shortcut behavior.

## Compatibility and migration

This is additive. Existing taps, hotkeys, drags, and their defaults remain intact.
Older native drivers must refuse the unknown operation; clients adopt it only
when runtime capability discovery confirms support for the requested target.
Generated bindings, schemas, fixtures, and documentation must change together.
A dependency pinned to an older driver cannot acquire this capability through a
wrapper change alone. Release/version changes follow the normal component process
and are not part of this RFC. Rollback removes client use of the new operation.

## Security, privacy, and telemetry

Reuse existing input authorization, lifecycle ownership, and platform permissions.
The operation creates no permission bypass or new grant. Do not log input names,
target identifiers, screenshots, or user content in new telemetry. Prefer existing
bounded effect/error reporting without adding telemetry. Synthetic fixtures must
contain no private desktop content.

## Implementation plan

1. Record the RFC decision, including exact result/refusal fields and the initial
   backend matrix; merge the accepted RFC before or with implementation.
2. Implement common ownership and contract generation, then platform adapters and
   refusal paths. Keep the feature unavailable where its release invariant is not
   proven; document every remaining backend gap.
3. Certify the exact candidate with native fixture observations and the canonical
   desktop harnesses. Integrations can then expose it using runtime capabilities.

## Test and acceptance plan

- Invalid bounds, keys, duplicate aliases, targets, permissions, and unsupported
  backends produce no input. SDK and MCP expose equivalent schemas and results.
- A native event fixture measures holds at 10, 50, 100, 500, and 1000 ms, including
  two keys plus a button. Stall acquisition after an early down and prove the
  acquisition deadline starts cleanup without a client cancellation. Observe one
  down/up pair per input, overlap, reverse
  release order, and no requested key/button remaining held after settlement.
- Cancel before dispatch, during acquisition, and during the hold. Also terminate
  the session, disconnect the client, close the target, change focus, race another
  synthetic caller, and inject down/release failures. Prove bounded cleanup and
  truthful partial/cleanup-failure results; an aborted waiter is not proof.
- A controlled SDL fixture samples maintained key/button state each game tick.
  Prove motion, firing, and simultaneous movement/firing using observed fixture
  state, not successful tool return values.
- On an interactive Doom-compatible free-data game, verify position changes,
  ammunition decreases when firing, and a reachable door opens. Retain before/after
  observations. A menu response or elapsed play time is insufficient evidence.
- Run the canonical affected-platform harnesses on the stable candidate SHA. Record
  unsupported Wayland backends and any native platform proof gaps explicitly.
- Confirm existing taps and hotkeys retain their behavior and that a subsequent
  unrelated action receives no leaked modifier or button state.

## Unresolved questions

- Accept `hold_inputs`, the 8-key/3-button limit, and the 5000 ms maximum?
- Which backends can establish safe acquisition and release for the first release?
- Which existing capability and action-result fields can encode support and release
  outcome without creating a second discovery or result vocabulary?
- How does native startup preserve or reconcile a domain after an unconfirmed
  release, and what crash
  guarantees can dedicated virtual input devices actually provide?

## Decision record

Pending maintainer review. No API implementation is authorized by this document
until the required decision is recorded in the linked discussion issue.
