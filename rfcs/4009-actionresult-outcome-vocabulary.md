---
title: ActionResult outcome vocabulary
authors:
  - will-bogusz
created: 2026-09-20
last_updated: 2026-09-29
status: accepted
discussion: https://github.com/trycua/cua/issues/4009
rfc_pr: https://github.com/trycua/cua/pull/4367
implementation:
  - https://github.com/trycua/cua/pull/3858
  - https://github.com/trycua/cua/pull/3923
  - https://github.com/trycua/cua/pull/3950
  - https://github.com/trycua/cua/issues/3971
supersedes:
superseded_by:
---

# RFC: ActionResult outcome vocabulary

## Summary

Extend `ActionResult` additively, in one increment on top of contract `0.8.0`,
so a consumer can choose its next step from typed fields instead of reply text:
whether the post-dispatch window observation ran, whether a value-setting
action's value was committed, and fixed codes plus a typed escalation on
pre-dispatch refusals. The contract moves from `0.8.0` to `0.9.0`. Nothing is
renamed or removed.

This document records the narrower scope the
[maintainer decision](https://github.com/trycua/cua/issues/4009#issuecomment-5896354993)
accepted on 2026-09-29. The rest of the proposal in #4009 is deferred and out
of scope here.

## Motivation

A consumer today receives verdicts it cannot act on:

- A skipped window poll, a lost poll and a completed poll that saw nothing all
  look the same on the wire. #3971 shows callers repeating non-idempotent
  actions because of it.
- A text write reports `confirmed` because the read-back matched, while the
  application can echo that read-back and discard the value at end of edit. A
  Save panel filename written that way reported `confirmed`, and the file
  landed as `Untitled.txt`.
- Pre-dispatch refusals carry per-tool codes and advice written for people, so
  every host keeps its own table of codes and sentences.

## Goals

- A consumer decides stop, observe, re-run or re-address from `ActionResult`
  fields, never from reply text.
- A value-setting action states whether the application kept the value,
  separately from the read-back.
- A pre-dispatch refusal carries a stable code and, where a rung exists, the
  same escalation shape as `ActionResult`.
- A raw-JSON `0.8.0` consumer keeps working unchanged.

## Non-goals

- `type_text` insertion semantics, partial counts and retry offsets (#3897).
- Snapshot identity, rebinding and typed window-change metadata (#3616,
  #3373).
- Driver-side automatic retries: escalation stays advice.
- The 800 ms text-field settle and browser action settles.
- Renaming or removing any `0.8.0` member.

## Terminology

- **Post-dispatch observation**: the window-change poll a producer runs after
  it dispatches an action, bounded at launch by the embedding host (#3929,
  #3946).
- **Commit**: the application keeping a written value, as opposed to the
  control reading it back.

## Current state

At contract `0.8.0`, `ActionResult` publishes `effect`, `route`, `delivery`,
`evidence[].kind` (`value_readback`, `window_change`) and
`escalation: {target, reason}`. On macOS, #3946 records internally whether the
post-dispatch poll ran (`Changes::polled`), but nothing publishes that fact.

Since #3882, Linux X11 foreground delivery emits `window_change` evidence rows.
macOS reports what its poll found only in prose, so on macOS a completed poll
without a `window_change` row does not yet mean that nothing opened.

## Proposal

Accepted as the first increment, additive to `0.8.0`:

1. **`post_dispatch_observation`** (`completed | skipped | unavailable`),
   optional on `ActionResult`, covering only the post-dispatch window-change
   observation. The enum and its projection live in `cua-driver-contract` and
   the core action record; each adapter reports which state happened.
   - macOS derives it from the #3946 provenance, split so that a zero bound is
     `skipped` and a lost poll is `unavailable`, and publishes any detected
     change as a `window_change` evidence row.
   - Linux X11 foreground delivery publishes the same three states in the same
     increment.
   - Windows, Wayland, X11 background delivery and the `browser_*` tools run no
     post-dispatch window observation and leave the field out. A missing field
     means "not observed", never "nothing changed"; the docs say so and
     contract tests pin it per platform.
   - The field never upgrades `effect`. `skipped` and `unavailable` never
     appear together with a `window_change` row from the poll.
2. **`committed`** (`committed | not_committed | unproven`) on `ActionResult`,
   set only by value-setting actions. A missing field means no verdict. This
   answers the RFC's third open question: `committed` stays on `ActionResult`.
3. **Refusal codes and escalation**: fixed codes `element_disabled`,
   `element_no_longer_exists` and `action_unsupported`, and a refusal
   `escalation: {target, reason}` that uses only existing enum values. The
   normalizer still reads `recommended`. Refusal detail such as `obscured_by`
   and the focus-holder fields stays documented structured content, marked per
   platform, not a typed contract. This answers the second open question for
   now.
4. **Version**: one contract bump for this increment, `0.8.0` to `0.9.0`.

Out of scope for this increment, and to be proposed as an amendment on #4009
with its own review period: `evidence[].signal`, the `element` and `snapshot`
escalation targets, a producer-observed `escalation.reason`, and
`menu_command` / `menu_path`. Each needs the first open question settled
(#3373) and a producer table for every platform.

## Alternatives considered

| Alternative | Why not |
| --- | --- |
| A per-call `detect_window_change` or other per-call skip | The supported path is the launch-time bound from #3929 plus the SDK allowlist in #3946 |
| Folding the observation into `effect` | `effect` states the action's outcome; whether a poll ran is a separate fact |
| A boolean `committed` | It collapses "the application took it" and "the control echoed it"; the echo case is the one that loses data |
| Facts that exist only in prose | Every host writes its own parsing, and a wording change becomes a silent contract break |

## Compatibility and migration

- A raw-JSON or MCP consumer on `0.8.0` sees unchanged replies until a producer
  emits a new field.
- A typed `0.8.0` SDK rejects the new fields when talking to a newer daemon, so
  daemon and SDK are upgraded together.
- A `0.9.0` SDK against a `0.8.0` daemon sees every new field absent.
- Rollback is a version pin: no new field is required.

## Security, privacy, and telemetry

No new permission, capture or retained state. The new fields publish verdicts
and codes, never written values or screenshot content.

## Implementation plan

- #3858: `committed` on macOS value-setting actions, and the `0.8.0` to `0.9.0`
  bump with the manifest and bindings regenerated.
- #3923: `element_disabled` and its escalation on macOS press and click
  refusals; refusal detail fields documented as macOS-only.
- #3950: rebased onto this document; its published detail fields documented as
  macOS-only.
- A follow-up pull request publishes `post_dispatch_observation` for macOS and
  Linux X11, with the documentation and contract tests above. #3971 closes when
  it lands.
- Producers of `element_no_longer_exists` and `action_unsupported` are linked
  here as their pull requests open.

## Test and acceptance plan

- Contract tests pin each new field's wire spellings and pin, per platform,
  which tools omit `post_dispatch_observation`.
- Action-record tests: `committed` and `post_dispatch_observation` never
  promote `effect`; `skipped` and `unavailable` never appear with a poll
  `window_change` row; refusal escalation uses only existing `target` and
  `reason` values; `recommended` still normalizes.
- `cua-contract-gen` and the binding generators run in `--check` mode.
- The canonical macOS Lume harness runs on #3858 before it is marked ready.

## Unresolved questions

- The deferred rows above.
- Whether refusal payloads become a typed contract type later; for now they
  stay documented structured content.

## Decision record

The [2026-09-29 maintainer decision](https://github.com/trycua/cua/issues/4009#issuecomment-5896354993)
accepts items 1 to 4 above, a narrower scope than the proposal, and defers the
other rows until #3373 settles the first open question and each row has a
producer table for every platform.

Material feedback: a skipped poll, a lost poll and a completed poll that saw
nothing looked the same on the wire, and #3971 shows callers repeating
non-idempotent actions because of it. Two statements in the proposal were out
of date: since #3882, Linux X11 foreground delivery emits `window_change`; and
because macOS reports its poll only in prose, "`completed` without a
`window_change` row means nothing opened" is not yet true on macOS. Several
deferred rows (`obscured_by.layer/subrole/ax_backed`, `menu_command`) used
macOS vocabulary in a type shared by all platforms without saying what Windows
or Linux would produce.

Rejected alternatives: a per-call window-observation skip, folding the
observation into `effect`, a boolean `committed`, and facts that exist only in
prose.

Remaining risks:

- A typed `0.8.0` SDK rejects the new fields from a newer daemon, so daemon and
  SDK are upgraded together.
- `post_dispatch_observation` must not be reused for other probes.
- The 800 ms text-field settle and browser action settles are out of scope.
- `post_dispatch_observation` was added to the proposal on 2026-09-25; because
  it is additive and drew no objection, it does not get a separate seven-day
  window.

Disposition: accepted with the narrower scope (items 1 to 4); the other rows
are deferred.
