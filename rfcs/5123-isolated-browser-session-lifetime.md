---
title: Discoverable session lifetime for isolated browsers
authors:
  - Iflaqbhat
created: 2026-10-14
last_updated: 2026-10-14
status: review
discussion: https://github.com/trycua/cua/issues/5123
rfc_pr:
implementation:
supersedes:
superseded_by:
---

# RFC: Discoverable session lifetime for isolated browsers

## Summary

Expose the preparing session's remaining idle lifetime in successful isolated
`browser_prepare` responses. Reuse the session lifecycle authority that powers
`get_session`; do not calculate a separate browser deadline. For longer QA
runs, propose an explicit authorized session renewal rather than treating an
open DevTools socket as evidence of agent activity.

This is a proposed addition to [the accepted lifecycle RFC](3007-cua-driver-lifecycle-sessions.md).
It needs a recorded decision before implementation and does not change the
current cleanup policy.

## Motivation

In #5123, the driver terminates an isolated browser during a QA run after its
owning session idles. Raw DevTools traffic does not refresh that session. The
agent loses the browser process, and an `isolated_new` profile loses its state.
The preparation response does not explain this lifetime.

Clients need to distinguish profile persistence from process persistence and
know whether their session is close to expiry. They also need a deliberate way
to continue a long run without weakening cleanup on revocation or disconnect.

## Current state

- [ManagedBrowser and session cleanup](../libs/cua-driver/rust/crates/cua-driver-core/src/browser/prepare.rs)
  associate an isolated process with preparing sessions. Session cleanup removes
  matching managed browser entries; dropping an entry terminates the process
  and removes its profile when `delete_profile` is set.
- `isolated_named` retains profile files. It does not make the process
  independent of its session or preserve open tabs after process termination.
- [The browser preparation result](../libs/cua-driver/rust/crates/cua-driver-core/src/browser/tools.rs)
  publishes the prepared PID, ownership evidence, effects, and attachment
  information, but no session lifetime.
- [The session tools](../libs/cua-driver/rust/crates/cua-driver-core/src/session_tools.rs)
  already expose `idle_seconds` and `expires_in_seconds` through `get_session`.
- [The lifecycle implementation](../libs/cua-driver/rust/crates/cua-driver-core/src/session.rs)
  defaults to five minutes idle and accepts a trusted runtime TTL override.
  The accepted RFC excludes health reads and transport keepalives from renewal;
  in-flight admitted calls are protected from idle eviction.

## Goals

- Publish session ownership and remaining idle lifetime when preparing a
  driver-owned isolated browser.
- Make named-profile persistence and process cleanup discoverable.
- Define a bounded renewal path that does not extend authorization.
- Preserve cleanup on session end, disconnect, revocation, and shutdown.

## Non-goals

- Detaching a browser from its owning lifecycle session.
- Treating raw DevTools traffic as trusted activity.
- An unconditional browser keepalive or an agent-controlled infinite TTL.
- Changes to existing-profile attachment grants and their separate deadlines.
- Claiming a remaining idle duration guarantees the browser stays alive.

## Proposal: publish lifetime first

Add an optional `lifecycle` object to successful `browser_prepare` output only
for a driver-owned isolated process associated with the current session. Its
proposed fields are:

| Field                            | Meaning                                                                             |
| -------------------------------- | ----------------------------------------------------------------------------------- |
| `owner`                          | The constant `session`; no runtime-private identifier.                              |
| `idle_expires_in_seconds`        | The current session's remaining idle duration, sampled after preparation completes. |
| `process_ends_with_session`      | `true`; the prepared process remains subject to session cleanup.                    |
| `profile_persists_after_session` | `false` for `isolated_new`, `true` for `isolated_named`.                            |

Use the same owner visibility and runtime TTL authority as `get_session`.
Account for the registry's activity refresh at admitted dispatch completion;
the response must not report a stale countdown from before the prepare call.
The value is a sample of remaining idle time, not a new lease or an absolute
authorization deadline. A session can end earlier through explicit end,
disconnect, revocation, or shutdown. Document these events in the tool text
and browser guide as well as the structured response.

Do not expose the new object on an existing-profile attachment or a merely
discovered endpoint that the driver does not own. Do not report the
existing-profile grant's 30-minute idle duration as this session's TTL.

The publication must respect preparation racing session end. If the session
has ended before success can be published, use the existing session-ended
refusal and cleanup path rather than returning a fresh lease or inventing a
replacement session. Profile reuse must not transfer ownership silently.

Clients can use `get_session` on the same transport and public session label
to inspect remaining time later. That health read does not keep the browser
alive. A named profile can be prepared again after cleanup, but the old process
and its in-memory page state do not survive.

## Proposal: explicit renewal in a separate implementation

Subject to maintainer approval, add a lifecycle tool named `renew_session`.
It refreshes an already-live session's activity timestamp under its existing
effective TTL and returns the existing session status output. It does not
accept a new duration, browser PID, raw endpoint, or different transport owner.

Authorization must complete before renewal: the tool must be allowed by the
runtime mode and any bounded manifest; absolute authorization deadlines,
revocation, and expired manifest idle authorization still win. It cannot
revive an ended session, reset a consent grant, or extend an absolute lifetime.

This tool is an explicit exception to the accepted rule that health reads and
transport keepalives do not refresh idle time. Keep `get_session`, socket
heartbeats, and raw CDP traffic under that existing rule. Document that clients
must stop renewing when their task finishes and call `end_session` normally.

If maintainers prefer only trusted launch-time configuration of a longer TTL,
ship the lifetime publication first and defer this tool. The initial response
improvement is independently reviewable and does not depend on renewal.

## Alternatives considered

**Keep the browser alive while its endpoint has clients:** an idle or abandoned
socket does not establish authorized work. It also adds an ownership authority
outside the accepted session cleanup coordinator.

**Extend every session's default TTL:** changes cleanup for all callers and
does not explain lifetime in the preparation result.

**Use `isolated_named` as a keepalive:** it retains files, not the process or
in-memory browser state.

**Document `get_session` only:** a useful interim workaround, but callers
still cannot discover ownership and persistence from the preparation result.

## Compatibility and platform behavior

The optional output object is additive. Older clients can ignore it. Existing
profile modes, input arguments, default TTL, and cleanup events stay as they
are in the first implementation.

Shared lifecycle and browser code own the behavior on macOS, Windows, X11, and
Wayland. Native adapters continue to launch and reap their owned process tree.
Do not expose private runtime session keys or change platform process identity.

Removing the optional metadata is the first phase's rollback. Renewal, if
approved, needs its own contract, policy classification, generated surfaces,
and rollback plan.

## Security, privacy, and telemetry

Remaining idle time is advisory. It does not replace any permission check or
authorize more browser access. Renewal must not cross transport namespaces,
modify consent, or revive revoked or ended sessions. Record outcome and timing
under existing policy without adding browser URLs or profile contents to logs.

## Implementation and acceptance plan

After the decision is recorded:

1. Add the optional preparation metadata from the existing session snapshot
   authority and document both profile modes and cleanup events.
2. Add hermetic public-boundary tests for default and trusted overridden TTL,
   named versus new profiles, missing metadata for unowned endpoints, and
   preparation racing session end. Use owned fake browser processes and an
   isolated test home; no real browser belongs in plain unit tests.
3. Mutation-check the lifetime source and ownership condition. Hard-coding a
   duration or publishing ownership for an unowned endpoint must fail tests.
4. If renewal is accepted, test a live owner, a foreign transport, an ended
   session, revoked authority, absolute expiry, and denied policy. Verify health
   reads and raw endpoint activity still do not refresh the session.
5. Add canonical desktop evidence that an owned process is cleaned up at
   session end, new-profile files are removed, and named-profile files remain.
   Record the final candidate and affected platform lanes before readiness.

## Decisions requested

- Approve the optional isolated-browser `lifecycle` fields and their advisory
  meaning, or choose different names consistent with `get_session`.
- Approve explicit renewal under the existing TTL, or restrict the first scope
  to discoverability and trusted launch-time TTL configuration.
- Confirm that endpoint activity must not become another ownership authority.

## Decision record

Pending maintainer review in #5123. No lifecycle exception is accepted yet.
